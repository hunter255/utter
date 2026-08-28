//! Release-only Tauri updater orchestration.
//!
//! The plugin is feature-gated so ordinary developer builds cannot install
//! releases. The public verification key is embedded at compile time; the
//! matching private key exists only in the protected release environment.

use serde::Serialize;

#[cfg(any(feature = "updater", test))]
pub(crate) const UPDATE_ENDPOINT: &str =
    "https://github.com/hunter255/utter/releases/latest/download/latest.json";
#[cfg(any(feature = "updater", test))]
const MAX_NOTES_CHARS: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub update: Option<UpdateInfo>,
}

#[cfg(feature = "updater")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UpdateProgress {
    Started { total: Option<u64> },
    Progress { downloaded: u64, total: Option<u64> },
    Finished,
}

#[cfg(any(feature = "updater", test))]
fn bounded_notes(notes: Option<&str>) -> Option<String> {
    notes.and_then(|notes| {
        let notes = notes.trim();
        if notes.is_empty() {
            return None;
        }

        let mut chars = notes.chars();
        let mut bounded = chars.by_ref().take(MAX_NOTES_CHARS).collect::<String>();
        if chars.next().is_some() {
            bounded.push('…');
        }
        Some(bounded)
    })
}

#[cfg(feature = "updater")]
mod enabled {
    use std::sync::Mutex;

    use tauri::{AppHandle, Emitter};
    use tauri_plugin_updater::{Update, UpdaterExt};

    use super::{bounded_notes, UpdateCheck, UpdateInfo, UpdateProgress, UPDATE_ENDPOINT};

    const PROGRESS_EVENT: &str = "update-progress";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Idle,
        Checking,
        Ready,
        Installing,
    }

    struct Inner {
        phase: Phase,
        pending: Option<Update>,
    }

    impl Default for Inner {
        fn default() -> Self {
            Self {
                phase: Phase::Idle,
                pending: None,
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct UpdaterState {
        inner: Mutex<Inner>,
    }

    impl UpdaterState {
        fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, String> {
            self.inner
                .lock()
                .map_err(|_| "updater state lock poisoned".to_string())
        }

        /// `Some` returns the already-verified pending release. `None`
        /// reserves the only network-check slot for the caller.
        fn begin_check(&self) -> Result<Option<UpdateCheck>, String> {
            let mut inner = self.lock()?;
            match inner.phase {
                Phase::Idle => {
                    inner.phase = Phase::Checking;
                    Ok(None)
                }
                Phase::Ready => inner
                    .pending
                    .as_ref()
                    .map(|update| Some(check_from_update(update)))
                    .ok_or_else(|| "updater state lost its pending release".to_string()),
                Phase::Checking => Err("an update check is already in progress".to_string()),
                Phase::Installing => Err("an update is already being installed".to_string()),
            }
        }

        fn finish_check(&self, update: Option<Update>) -> Result<UpdateCheck, String> {
            let mut inner = self.lock()?;
            inner.pending = update;
            inner.phase = if inner.pending.is_some() {
                Phase::Ready
            } else {
                Phase::Idle
            };
            Ok(inner
                .pending
                .as_ref()
                .map(check_from_update)
                .unwrap_or_else(no_update))
        }

        fn fail_check(&self) {
            if let Ok(mut inner) = self.lock() {
                inner.pending = None;
                inner.phase = Phase::Idle;
            }
        }

        fn begin_install(&self) -> Result<Update, String> {
            let mut inner = self.lock()?;
            match inner.phase {
                Phase::Ready => {
                    let update = inner
                        .pending
                        .take()
                        .ok_or_else(|| "there is no pending update".to_string())?;
                    inner.phase = Phase::Installing;
                    Ok(update)
                }
                Phase::Idle => Err("check for an update first".to_string()),
                Phase::Checking => Err("the update check has not finished".to_string()),
                Phase::Installing => Err("an update is already being installed".to_string()),
            }
        }

        fn finish_install(&self, retry: Option<Update>) {
            if let Ok(mut inner) = self.lock() {
                inner.pending = retry;
                inner.phase = if inner.pending.is_some() {
                    Phase::Ready
                } else {
                    Phase::Idle
                };
            }
        }
    }

    fn no_update() -> UpdateCheck {
        UpdateCheck {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            update: None,
        }
    }

    fn check_from_update(update: &Update) -> UpdateCheck {
        UpdateCheck {
            current_version: update.current_version.clone(),
            update: Some(UpdateInfo {
                version: update.version.clone(),
                notes: bounded_notes(update.body.as_deref()),
            }),
        }
    }

    pub(crate) async fn check(app: AppHandle, state: &UpdaterState) -> Result<UpdateCheck, String> {
        if let Some(cached) = state.begin_check()? {
            return Ok(cached);
        }

        let result = async {
            let endpoint: tauri::Url = UPDATE_ENDPOINT
                .parse()
                .map_err(|error| format!("invalid update endpoint: {error}"))?;
            app.updater_builder()
                .endpoints(vec![endpoint])
                .map_err(|error| error.to_string())?
                .build()
                .map_err(|error| error.to_string())?
                .check()
                .await
                .map_err(|error| error.to_string())
        }
        .await;

        match result {
            Ok(update) => {
                let report = state.finish_check(update)?;
                if let Some(update) = &report.update {
                    tracing::info!(version = %update.version, "signed update is available");
                } else {
                    tracing::info!("application is up to date");
                }
                Ok(report)
            }
            Err(error) => {
                state.fail_check();
                tracing::warn!(error = %error, "update check failed");
                Err(format!("update check failed: {error}"))
            }
        }
    }

    pub(crate) async fn install_and_restart(
        app: AppHandle,
        state: &UpdaterState,
    ) -> Result<(), String> {
        let update = state.begin_install()?;
        let retry = update.clone();
        let progress_app = app.clone();
        let finished_app = app.clone();
        let mut started = false;
        let mut downloaded = 0_u64;

        let result = update
            .download_and_install(
                move |chunk_length, total| {
                    let chunk = u64::try_from(chunk_length).unwrap_or(u64::MAX);
                    downloaded = downloaded.saturating_add(chunk);
                    if !started {
                        let _ =
                            progress_app.emit(PROGRESS_EVENT, UpdateProgress::Started { total });
                        started = true;
                    }
                    let _ = progress_app.emit(
                        PROGRESS_EVENT,
                        UpdateProgress::Progress { downloaded, total },
                    );
                },
                move || {
                    let _ = finished_app.emit(PROGRESS_EVENT, UpdateProgress::Finished);
                },
            )
            .await;

        if let Err(error) = result {
            state.finish_install(Some(retry));
            tracing::warn!(error = %error, "signed update installation failed");
            return Err(format!("update installation failed: {error}"));
        }

        state.finish_install(None);
        tracing::info!(version = %update.version, "signed update installed; restarting");
        app.restart();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn updater_operations_are_single_flight_and_recover_after_failure() {
            let state = UpdaterState::default();

            assert_eq!(state.begin_check().unwrap(), None);
            assert_eq!(
                state.begin_check().unwrap_err(),
                "an update check is already in progress"
            );
            assert_eq!(
                state.begin_install().err().unwrap(),
                "the update check has not finished"
            );

            state.fail_check();
            assert_eq!(state.begin_check().unwrap(), None);
        }
    }
}

#[cfg(feature = "updater")]
pub(crate) use enabled::{check, install_and_restart, UpdaterState};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_are_trimmed_bounded_and_unicode_safe() {
        assert_eq!(bounded_notes(Some("  hello  ")), Some("hello".to_string()));
        assert_eq!(bounded_notes(Some("   ")), None);

        let long = "ю".repeat(MAX_NOTES_CHARS + 1);
        let bounded = bounded_notes(Some(&long)).unwrap();
        assert_eq!(bounded.chars().count(), MAX_NOTES_CHARS + 1);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn updater_uses_the_forks_https_release_manifest() {
        assert_eq!(
            UPDATE_ENDPOINT,
            "https://github.com/hunter255/utter/releases/latest/download/latest.json"
        );
    }
}
