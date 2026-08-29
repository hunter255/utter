//! The one system tray/menu-bar status item: toggle dictation, temporarily
//! pause refinement, open settings, and quit cleanly. Its compact title also
//! mirrors the dictation phase so a hidden HUD does not make recording state
//! invisible: `●` while listening, `…` while the transcript is processed,
//! and no suffix while idle.

use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::events::DictationPhase;
use crate::state::AppState;

/// Stable id for the sole status item owned by this module. The declarative
/// `app.trayIcon` entry must stay absent from `tauri.conf.json`; otherwise
/// Tauri creates a second icon before [`build`] runs.
const TRAY_ID: &str = "utter-main";

const MENU_TOGGLE: &str = "toggle-dictation";
const MENU_REFINE: &str = "toggle-refinement";
const MENU_REFINE_LABEL: &str = "Pause transcript refinement";
const MENU_SETTINGS: &str = "open-settings";
const MENU_QUIT: &str = "quit";

fn refinement_is_paused(enabled: bool) -> bool {
    !enabled
}

/// The three appearances the five runtime phases collapse into. The
/// difference is deliberately shape/text as well as tooltip, not colour:
/// macOS owns status-item tint in light/dark mode and a colour-only signal
/// would be inaccessible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum TrayActivity {
    #[default]
    Idle,
    Recording,
    Processing,
}

impl TrayActivity {
    fn for_phase(phase: DictationPhase) -> Self {
        match phase {
            DictationPhase::Idle => Self::Idle,
            DictationPhase::Recording => Self::Recording,
            DictationPhase::Transcribing | DictationPhase::Refining | DictationPhase::Injecting => {
                Self::Processing
            }
        }
    }

    fn title(self) -> &'static str {
        match self {
            // `set_title(None)` does not clear the title on every tray-icon
            // backend; an explicit empty string does.
            Self::Idle => "",
            Self::Recording => "●",
            Self::Processing => "…",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "Utter — Ready",
            Self::Recording => "Utter — Recording",
            Self::Processing => "Utter — Processing speech",
        }
    }
}

/// App-managed state used to suppress redundant native status-item updates.
/// Recording level events arrive many times per second; only a phase edge
/// should call into AppKit.
#[derive(Default)]
pub(crate) struct TrayIndicator {
    activity: Mutex<TrayActivity>,
}

/// Builds the tray icon and its menu, wiring every item to its handler.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let refinement_paused = {
        let state = app.state::<AppState>();
        state
            .settings
            .read()
            .map(|s| refinement_is_paused(s.refine.enabled))
            .unwrap_or(true)
    };

    let toggle = MenuItem::with_id(app, MENU_TOGGLE, "Toggle dictation", true, None::<&str>)?;
    let refine = CheckMenuItem::with_id(
        app,
        MENU_REFINE,
        MENU_REFINE_LABEL,
        true,
        refinement_paused,
        None::<&str>,
    )?;
    let settings_item = MenuItem::with_id(app, MENU_SETTINGS, "Open settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&toggle, &refine, &settings_item, &quit])?;

    let refine_for_handler = refine.clone();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(TrayActivity::Idle.tooltip())
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu_event(app, event, &refine_for_handler));

    match app.default_window_icon().cloned() {
        Some(icon) => builder = builder.icon(icon),
        None => tracing::warn!("no default window icon configured; tray icon may not render"),
    }

    builder.build(app)?;

    Ok(())
}

/// Reflects `phase` in the menu bar. Failures only degrade this accessory
/// indicator; recording, HUD updates, and injection continue unchanged.
pub(crate) fn set_phase(app: &AppHandle, phase: DictationPhase) {
    let next = TrayActivity::for_phase(phase);
    let indicator = app.state::<TrayIndicator>();
    let mut current = indicator
        .activity
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *current == next {
        return;
    }

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        tracing::warn!("tray icon not found; cannot show dictation state");
        return;
    };

    let mut updated = true;
    if let Err(error) = tray.set_title(Some(next.title())) {
        tracing::warn!("failed to update tray state title: {error}");
        updated = false;
    }
    if let Err(error) = tray.set_tooltip(Some(next.tooltip())) {
        tracing::warn!("failed to update tray state tooltip: {error}");
        updated = false;
    }
    if updated {
        *current = next;
    }
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent, refine_item: &CheckMenuItem<tauri::Wry>) {
    match event.id().as_ref() {
        MENU_TOGGLE => toggle_dictation(app),
        MENU_REFINE => toggle_refinement(app, refine_item),
        MENU_SETTINGS => open_settings(app),
        MENU_QUIT => quit(app),
        _ => {}
    }
}

fn toggle_dictation(app: &AppHandle) {
    let state = app.state::<AppState>();
    let guard = state
        .session_ctl
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match guard.as_ref() {
        Some(handle) => handle.toggle(),
        None => {
            drop(guard);
            crate::sink::notify_no_session(app);
        }
    }
}

/// Flips `settings.refine.enabled`, persists it through the same save path
/// `save_settings` uses, and presents the inverse as a temporary "paused"
/// checkbox. Individual profile policies remain untouched.
fn toggle_refinement(app: &AppHandle, refine_item: &CheckMenuItem<tauri::Wry>) {
    let state = app.state::<AppState>();

    let mut settings = {
        let guard = state
            .settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clone()
    };
    settings.refine.enabled = !settings.refine.enabled;
    let paused = refinement_is_paused(settings.refine.enabled);

    if let Err(e) = crate::commands::persist_and_apply(app, &state, settings) {
        tracing::warn!("failed to toggle refinement: {e}");
        return;
    }

    if let Err(e) = refine_item.set_checked(paused) {
        tracing::warn!("failed to update refinement menu checkbox: {e}");
    }
}

fn open_settings(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(window) => {
            if let Err(e) = window.show() {
                tracing::warn!("failed to show main window: {e}");
            }
            if let Err(e) = window.set_focus() {
                tracing::warn!("failed to focus main window: {e}");
            }
        }
        None => tracing::warn!("main window not found; cannot open settings"),
    }
}

pub(crate) fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    crate::runtime_boot::shutdown(&state);
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_phases_map_to_three_stable_menu_bar_states() {
        assert_eq!(
            TrayActivity::for_phase(DictationPhase::Idle),
            TrayActivity::Idle
        );
        assert_eq!(
            TrayActivity::for_phase(DictationPhase::Recording),
            TrayActivity::Recording
        );
        for phase in [
            DictationPhase::Transcribing,
            DictationPhase::Refining,
            DictationPhase::Injecting,
        ] {
            assert_eq!(TrayActivity::for_phase(phase), TrayActivity::Processing);
        }
    }

    #[test]
    fn every_state_has_a_textual_cue_and_idle_clears_the_suffix() {
        assert_eq!(TrayActivity::Idle.title(), "");
        assert_eq!(TrayActivity::Recording.title(), "●");
        assert_eq!(TrayActivity::Processing.title(), "…");
        for activity in [
            TrayActivity::Idle,
            TrayActivity::Recording,
            TrayActivity::Processing,
        ] {
            assert!(activity.tooltip().starts_with("Utter — "));
        }
    }

    #[test]
    fn refinement_menu_describes_its_checked_state_as_a_pause() {
        assert_eq!(MENU_REFINE_LABEL, "Pause transcript refinement");
        assert!(refinement_is_paused(false));
        assert!(!refinement_is_paused(true));
    }
}
