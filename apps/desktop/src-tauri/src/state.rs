//! Shared application state, managed by Tauri and reached from every
//! command through `tauri::State<AppState>`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};

use utter_store::{DownloadCancellation, HistoryRepo, ModelManager, Settings};

use crate::events::{
    ModelOperationKind, ModelOperationPhase, ModelOperationSnapshot, ModelOperationState, Notice,
};
use crate::runtime::RuntimeHandle;
use crate::sink::{parse_kind, NoticeThrottle};

/// The name of the history database file under the app's XDG data directory.
const HISTORY_DB_FILE: &str = "history.sqlite3";

/// Notices reported before any window existed to hear them, kept until one
/// asks for them.
///
/// `runtime_boot::boot` runs synchronously inside Tauri's `setup` — before
/// the webview is loaded, and long before the settings window subscribes to
/// the `notice` event. Tauri's `emit` has no replay, so every notice boot
/// reports lands on zero listeners and is gone: exactly the degradations the
/// app most needs to explain (no model downloaded, an unavailable preview, a
/// config that would not migrate) are the ones reported at the one moment
/// nothing is listening.
///
/// The desktop notification is not a substitute. It is deliberately rate
/// limited (see [`NoticeThrottle`]), and boot reports its notices in a tight
/// loop, so a startup with two conditions to explain shows the first and
/// drops the second — which is a real configuration, not a corner case: a
/// missing transcription model and an unavailable preview arrive together.
///
/// So boot parks a copy here as well, and the frontend drains it on mount
/// (`take_pending_notices`). That is what makes the settings window the
/// backstop the rest of the app documents it as: nothing reported at startup
/// is lost, however many conditions there were.
#[derive(Default)]
pub struct PendingNotices {
    queued: Mutex<Vec<Notice>>,
}

impl PendingNotices {
    /// Parks one notice, `kind` using the same vocabulary as
    /// [`crate::runtime::EventSink::notify`] (`"info"`, `"warning"`,
    /// `"error"`).
    pub(crate) fn push(&self, kind: &str, message: &str) {
        // A poisoned lock means some earlier caller panicked mid-update; the
        // queue itself is a plain `Vec` and still intact, and losing the
        // startup notices is precisely the failure this type exists to
        // prevent, so recover rather than propagate.
        self.queued
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Notice::from_message(parse_kind(kind), message));
    }

    /// Hands over everything parked so far, leaving the queue empty.
    ///
    /// Draining (rather than copying) is what keeps a second reader — a
    /// window reopened later in the same run — from replaying startup
    /// conditions the user has already read and dismissed.
    pub(crate) fn take(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.queued.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl ModelOperationKind {
    fn present_participle(self) -> &'static str {
        match self {
            Self::Download => "downloading",
            Self::Remove => "removing",
        }
    }
}

#[derive(Debug)]
struct ActiveModelOperation {
    state: ModelOperationState,
    cancellation: Option<DownloadCancellation>,
}

/// Serializes model-file mutations and owns the cancellation handle for the
/// one active download. A lease lives on the blocking worker and clears the
/// slot on every return path, including a panic unwind or a dropped command
/// future.
#[derive(Debug, Default)]
pub(crate) struct ModelOperations {
    active: Mutex<Option<ActiveModelOperation>>,
    next_generation: AtomicU64,
}

impl ModelOperations {
    pub(crate) fn begin_download(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<(ModelOperationLease, DownloadCancellation), String> {
        let cancellation = DownloadCancellation::default();
        let lease = self.begin(id, ModelOperationKind::Download, Some(cancellation.clone()))?;
        Ok((lease, cancellation))
    }

    pub(crate) fn begin_remove(self: &Arc<Self>, id: &str) -> Result<ModelOperationLease, String> {
        self.begin(id, ModelOperationKind::Remove, None)
    }

    fn begin(
        self: &Arc<Self>,
        id: &str,
        kind: ModelOperationKind,
        cancellation: Option<DownloadCancellation>,
    ) -> Result<ModelOperationLease, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "model operation lock poisoned".to_string())?;
        if let Some(existing) = active.as_ref() {
            return Err(format!(
                "model '{}' is already {}; wait for it to finish or cancel its download first",
                existing.state.id,
                existing.state.kind.present_participle()
            ));
        }

        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let phase = match kind {
            ModelOperationKind::Download => ModelOperationPhase::Preparing,
            ModelOperationKind::Remove => ModelOperationPhase::Removing,
        };
        *active = Some(ActiveModelOperation {
            state: ModelOperationState {
                generation,
                id: id.to_string(),
                kind,
                phase,
                done: 0,
                total: 0,
            },
            cancellation,
        });
        Ok(ModelOperationLease {
            owner: Arc::clone(self),
            generation,
        })
    }

    /// Requests cancellation of `id`. A download that completed between the
    /// UI click and this command is a successful no-op; a different active
    /// operation remains an explicit conflict.
    pub(crate) fn cancel_download(&self, id: &str) -> Result<Option<ModelOperationState>, String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "model operation lock poisoned".to_string())?;
        let Some(operation) = active.as_mut() else {
            return Ok(None);
        };
        if operation.state.id != id {
            return Err(format!(
                "model '{}' is currently {}; cannot cancel download of '{}'",
                operation.state.id,
                operation.state.kind.present_participle(),
                id
            ));
        }
        let Some(cancellation) = operation.cancellation.as_ref() else {
            return Err(format!("model '{}' is being removed, not downloaded", id));
        };
        cancellation.cancel();
        operation.state.phase = ModelOperationPhase::Cancelling;
        Ok(Some(operation.state.clone()))
    }

    /// Returns the authoritative state for a newly opened or remounted UI.
    pub(crate) fn snapshot(&self) -> Result<ModelOperationSnapshot, String> {
        let active = self
            .active
            .lock()
            .map_err(|_| "model operation lock poisoned".to_string())?;
        Ok(ModelOperationSnapshot {
            generation: self.next_generation.load(Ordering::Relaxed),
            operation: active.as_ref().map(|operation| operation.state.clone()),
        })
    }
}

/// Keeps a model operation registered for exactly as long as its blocking
/// task exists.
#[derive(Debug)]
pub(crate) struct ModelOperationLease {
    owner: Arc<ModelOperations>,
    generation: u64,
}

impl ModelOperationLease {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Updates the operation snapshot on every network callback. Event
    /// throttling happens separately, so a UI that reconnects between events
    /// still receives the latest byte counts from `snapshot()`.
    pub(crate) fn update_progress(
        &self,
        done: u64,
        total: u64,
    ) -> Result<Option<ModelOperationState>, String> {
        let mut active = self
            .owner
            .active
            .lock()
            .map_err(|_| "model operation lock poisoned".to_string())?;
        let Some(operation) = active.as_mut() else {
            return Ok(None);
        };
        if operation.state.generation != self.generation {
            return Ok(None);
        }
        if operation.state.phase != ModelOperationPhase::Cancelling {
            operation.state.phase = ModelOperationPhase::Downloading;
        }
        operation.state.done = done;
        operation.state.total = total;
        Ok(Some(operation.state.clone()))
    }
}

impl Drop for ModelOperationLease {
    fn drop(&mut self) {
        let mut active = self
            .owner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|operation| operation.state.generation == self.generation)
        {
            *active = None;
        }
    }
}

/// Application state shared across all Tauri commands.
///
/// `models` is wrapped in an `Arc` (rather than owned directly) so the async
/// `download_model` command can clone a handle to it into a `spawn_blocking`
/// closure without borrowing from a short-lived `tauri::State` guard.
pub struct AppState {
    pub settings: RwLock<Settings>,
    pub models: Arc<ModelManager>,
    /// At most one download/remove mutates model files at a time; downloads
    /// expose their cancellation flag through this registry.
    pub(crate) model_operations: Arc<ModelOperations>,
    pub history: Mutex<HistoryRepo>,
    /// The running dictation runtime's control handle. `None` only if boot
    /// (`runtime_boot::boot`) itself failed outright (an unexpected I/O
    /// error, not a degraded-but-booted condition like a missing model or
    /// hotkey permissions) — every command that reaches into this treats
    /// `None` as "no session control available yet" rather than panicking;
    /// the next successful `save_settings` spins one up (see
    /// `runtime_boot::rebuild`).
    pub session_ctl: Mutex<Option<RuntimeHandle>>,
    /// User-facing warnings discovered before the window and runtime exist,
    /// including storage-identity and settings-schema migration failures.
    pub startup_notices: Vec<String>,
    /// Live mirror of `settings.dictation.hud`, shared with every
    /// `TauriEventSink` (see `crate::sink`). A plain `RwLock<Settings>` read
    /// isn't enough on its own: the sink used by an already-running
    /// dictation `Runtime` is constructed once (at boot or the next
    /// `rebuild`) and kept for that runtime's whole lifetime, so this needs
    /// to be a shared, in-place-updatable cell rather than a value read
    /// fresh at sink-construction time, or a settings change wouldn't reach
    /// a sink that already exists.
    pub hud_enabled: Arc<AtomicBool>,
    /// Shared with every `TauriEventSink` (see [`crate::sink::NoticeThrottle`]),
    /// for the same reason `hud_enabled` is: sinks are built fresh per emit,
    /// so the rate limit has to outlive them or it limits nothing.
    pub notice_throttle: Arc<NoticeThrottle>,
    /// Everything `runtime_boot::boot` reported before any window could hear
    /// it, kept until the settings window asks (see [`PendingNotices`] and
    /// the `take_pending_notices` command).
    ///
    /// Distinct from `startup_notices` above, which are messages travelling
    /// from `AppState::new` (which has no `AppHandle`, so cannot report
    /// anything) to `boot`, the one place notices are reported. This is
    /// reported notices travelling from `boot` onward to the frontend. Boot
    /// therefore still reports the migration message through the sink like
    /// every other notice — and this parks it there like every other notice —
    /// rather than the two mechanisms being one, which would take the
    /// migration message out of the desktop-notification channel it currently
    /// reaches the user through.
    pub pending_notices: PendingNotices,
    /// Main-event-loop global shortcut registrations and their current
    /// runtime channel. macOS only; Linux keeps using `HotkeySource`.
    #[cfg(target_os = "macos")]
    pub macos_hotkeys: crate::macos_hotkeys::MacosHotkeys,
}

impl AppState {
    /// Builds application state: loads settings from disk (defaulting if
    /// absent), and opens the history database, creating both the on-disk
    /// config and data directories as needed.
    ///
    /// A config that fails to migrate degrades rather than aborting startup:
    /// the original file is left exactly as `utter_store::load` left it (see
    /// its doc comment), this run boots with `Settings::default()`, and
    /// `startup_notices` carries a message for `runtime_boot::boot` to queue.
    /// Any other load failure (unreadable file, genuinely malformed TOML
    /// unrelated to migration) still aborts startup, as it did before.
    pub fn new() -> Result<Self> {
        let storage_migration = utter_store::migrate_legacy_storage();
        let mut startup_notices = storage_migration.warnings;

        let config_path = utter_store::config_path();
        let settings = match utter_store::load(&config_path) {
            Ok(settings) => settings,
            Err(err) => match err.downcast_ref::<utter_store::MigrationFailed>() {
                Some(failed) => {
                    tracing::warn!("{err:#}");
                    startup_notices.push(migration_notice(failed));
                    Settings::default()
                }
                None => return Err(err).context("failed to load settings"),
            },
        };
        let hud_enabled = Arc::new(AtomicBool::new(settings.dictation.hud));

        let models = Arc::new(ModelManager::new(data_dir()?));
        let history =
            HistoryRepo::open(&history_db_path()?).context("failed to open history database")?;

        Ok(Self {
            settings: RwLock::new(settings),
            models,
            model_operations: Arc::new(ModelOperations::default()),
            history: Mutex::new(history),
            session_ctl: Mutex::new(None),
            startup_notices,
            hud_enabled,
            notice_throttle: Arc::new(NoticeThrottle::default()),
            pending_notices: PendingNotices::default(),
            #[cfg(target_os = "macos")]
            macos_hotkeys: crate::macos_hotkeys::MacosHotkeys::default(),
        })
    }
}

/// Builds the notice `AppState::new` queues for a config that could not be
/// migrated. Names the backup only when `failed.backup` is `Some` — a
/// `None` means the backup step itself is what failed, so the file at
/// `failed.path` (left untouched) is the user's only copy, and the message
/// must not claim a safety net that was never written.
fn migration_notice(failed: &utter_store::MigrationFailed) -> String {
    match &failed.backup {
        Some(backup) => format!(
            "Your settings at {} could not be upgraded to the new format and \
             were left unchanged; a backup was saved at {}. Utter is running \
             with default settings for now.",
            failed.path.display(),
            backup.display()
        ),
        None => format!(
            "Your settings at {} could not be upgraded to the new format and \
             were left unchanged. Utter is running with default settings for now.",
            failed.path.display()
        ),
    }
}

/// The per-user data directory for the app, under its final application
/// identifier (matching [`utter_store::config_path`]).
fn data_dir() -> Result<PathBuf> {
    utter_store::data_dir()
}

/// The history database's on-disk path. Shared by [`AppState::new`] (which
/// opens the command-facing connection kept for the app's lifetime) and
/// `runtime_boot`, which opens its own separate connection for the dictation
/// worker thread whenever `Settings.history.enabled` is true.
pub(crate) fn history_db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join(HISTORY_DB_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::events::{NoticeCode, NoticeKind};

    #[test]
    fn model_operation_lease_serializes_downloads_and_removes_then_releases() {
        let operations = Arc::new(ModelOperations::default());
        let (download, _) = operations.begin_download("large").expect("first download");

        let conflict = operations
            .begin_remove("small")
            .expect_err("remove must not race a download");
        assert!(conflict.contains("large"));
        assert!(conflict.contains("downloading"));

        drop(download);
        let remove = operations.begin_remove("small").expect("slot released");
        drop(remove);
        operations
            .begin_download("small")
            .expect("remove lease released");
    }

    #[test]
    fn cancellation_targets_only_the_matching_active_download() {
        let operations = Arc::new(ModelOperations::default());
        let (_lease, cancellation) = operations.begin_download("giga").expect("download");

        let mismatch = operations
            .cancel_download("parakeet")
            .expect_err("another model must not be cancelled");
        assert!(mismatch.contains("giga"));
        assert!(!cancellation.is_cancelled());

        let cancelling = operations
            .cancel_download("giga")
            .expect("cancel")
            .expect("active operation");
        assert_eq!(cancelling.phase, ModelOperationPhase::Cancelling);
        assert!(cancellation.is_cancelled());
        assert!(operations
            .cancel_download("giga")
            .expect("idempotent")
            .is_some());
    }

    #[test]
    fn cancellation_after_completion_is_a_successful_no_op() {
        let operations = Arc::new(ModelOperations::default());
        let (lease, _) = operations.begin_download("small").expect("download");
        drop(lease);

        assert!(operations
            .cancel_download("small")
            .expect("late cancel")
            .is_none());
    }

    #[test]
    fn model_operation_snapshot_tracks_progress_and_completion() {
        let operations = Arc::new(ModelOperations::default());
        assert_eq!(
            operations.snapshot().expect("empty snapshot"),
            ModelOperationSnapshot {
                generation: 0,
                operation: None,
            }
        );

        let (lease, _) = operations.begin_download("giga").expect("download");
        let preparing = operations
            .snapshot()
            .expect("preparing snapshot")
            .operation
            .expect("active operation");
        assert_eq!(preparing.generation, 1);
        assert_eq!(preparing.phase, ModelOperationPhase::Preparing);
        assert_eq!((preparing.done, preparing.total), (0, 0));

        lease
            .update_progress(25, 100)
            .expect("progress update")
            .expect("active operation");
        let downloading = operations
            .snapshot()
            .expect("progress snapshot")
            .operation
            .expect("active operation");
        assert_eq!(downloading.phase, ModelOperationPhase::Downloading);
        assert_eq!((downloading.done, downloading.total), (25, 100));

        drop(lease);
        assert_eq!(
            operations.snapshot().expect("completed snapshot"),
            ModelOperationSnapshot {
                generation: 1,
                operation: None,
            }
        );
    }

    #[test]
    fn stale_lease_cannot_overwrite_a_new_operation() {
        let operations = Arc::new(ModelOperations::default());
        let (first, _) = operations.begin_download("first").expect("first");
        let stale_generation = first.generation();
        drop(first);

        let (second, _) = operations.begin_download("second").expect("second");
        assert!(second.generation() > stale_generation);

        let stale = ModelOperationLease {
            owner: Arc::clone(&operations),
            generation: stale_generation,
        };
        assert!(stale
            .update_progress(99, 100)
            .expect("stale update")
            .is_none());
        let snapshot = operations
            .snapshot()
            .expect("snapshot")
            .operation
            .expect("second operation");
        assert_eq!(snapshot.id, "second");
        assert_eq!(snapshot.done, 0);

        // Dropping the stale lease must not release the current one.
        drop(stale);
        assert!(operations.begin_remove("third").is_err());
        drop(second);
    }

    #[test]
    fn a_notice_without_a_backup_does_not_claim_one_was_saved() {
        // `backup: None` is what a real `MigrationFailed` carries when the
        // backup step itself is what failed — see
        // `utter_store::settings::migrate_and_persist`. The notice built
        // from it must not tell the user a backup exists.
        let failed = utter_store::MigrationFailed {
            path: PathBuf::from("/home/user/.config/utter/config.toml"),
            backup: None,
        };

        let notice = migration_notice(&failed);

        assert!(
            !notice.to_lowercase().contains("backup"),
            "no backup was written, so the notice must not mention one: {notice}"
        );
        assert!(notice.contains("config.toml"), "must still name the file");
    }

    #[test]
    fn parked_notices_come_back_in_the_order_they_were_reported() {
        let parked = PendingNotices::default();
        parked.push("warning", "no transcription model");
        parked.push("info", "live preview unavailable");

        let taken = parked.take();

        assert_eq!(
            taken
                .iter()
                .map(|n| (n.kind, n.message.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (NoticeKind::Warning, "no transcription model"),
                (NoticeKind::Info, "live preview unavailable"),
            ],
            "both conditions of the same startup must survive, in order -- the throttled \
             desktop-notification channel is what only carries the first"
        );
    }

    #[test]
    fn parked_notice_uses_the_same_stable_payload_as_a_live_notice() {
        let message = "failed to start transcription: native decoder exploded";
        let live = Notice::from_message(NoticeKind::Error, message);
        let parked = PendingNotices::default();
        parked.push("error", message);
        let queued = parked.take().pop().expect("parked notice");

        assert_eq!(queued.code, Some(NoticeCode::TranscriptionStartFailed));
        assert_eq!(queued.code, live.code);
        assert_eq!(queued.args, live.args);
        assert_eq!(queued.detail, live.detail);
        assert_eq!(queued.message, live.message);
    }

    /// Draining, not copying: the settings window can be closed and reopened
    /// any number of times in one run, and a startup condition the user has
    /// already read must not come back as if it were news.
    #[test]
    fn taking_parked_notices_empties_the_queue() {
        let parked = PendingNotices::default();
        parked.push("error", "failed to start hotkey capture");

        assert_eq!(parked.take().len(), 1);
        assert!(parked.take().is_empty());
    }

    #[test]
    fn a_notice_with_a_backup_names_it() {
        let failed = utter_store::MigrationFailed {
            path: PathBuf::from("/home/user/.config/utter/config.toml"),
            backup: Some(PathBuf::from("/home/user/.config/utter/config.toml.v1.bak")),
        };

        let notice = migration_notice(&failed);

        assert!(notice.contains("config.toml.v1.bak"));
    }
}
