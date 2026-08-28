//! Keeps `general.autostart` and the operating system's login registration
//! in sync through Tauri's official autostart plugin.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationChange {
    None,
    Enable,
    Disable,
}

/// Decides the only mutation the OS registration needs. Returning `None`
/// when the state already matches is what prevents duplicate Login Items.
fn required_change(desired: bool, actual: bool) -> RegistrationChange {
    match (desired, actual) {
        (true, false) => RegistrationChange::Enable,
        (false, true) => RegistrationChange::Disable,
        _ => RegistrationChange::None,
    }
}

/// Whether a normal settings save should touch the OS registration at all.
/// Startup always calls [`reconcile`] to repair external drift; save only
/// calls it on the preference edge so unrelated settings edits cannot churn
/// a healthy Login Item.
pub(crate) fn preference_changed(previous: bool, desired: bool) -> bool {
    previous != desired
}

/// Makes the OS registration agree with `desired`, without mutating it when
/// it already agrees. This function performs blocking platform I/O and is
/// called either from Tauri setup or `save_settings`' blocking task.
pub(crate) fn reconcile(app: &AppHandle, desired: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let actual = manager
        .is_enabled()
        .map_err(|error| format!("failed to inspect the Login Item: {error}"))?;

    match required_change(desired, actual) {
        RegistrationChange::None => Ok(()),
        RegistrationChange::Enable => manager
            .enable()
            .map_err(|error| format!("failed to enable the Login Item: {error}")),
        RegistrationChange::Disable => manager
            .disable()
            .map_err(|error| format!("failed to disable the Login Item: {error}")),
    }
}

/// User-facing wording shared by startup repair and a failed settings toggle.
/// The preference remains saved, so the recovery is an explicit off/on edge.
pub(crate) fn failure_notice(error: &str) -> String {
    format!(
        "Utter saved your Launch at login preference, but could not synchronize it with the \
         operating system: {error}. Open Settings > General and toggle Launch at login off and \
         on again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_matrix_only_mutates_a_mismatched_registration() {
        assert_eq!(required_change(false, false), RegistrationChange::None);
        assert_eq!(required_change(true, true), RegistrationChange::None);
        assert_eq!(required_change(true, false), RegistrationChange::Enable);
        assert_eq!(required_change(false, true), RegistrationChange::Disable);
    }

    #[test]
    fn settings_save_reconciles_only_on_the_preference_edge() {
        assert!(!preference_changed(false, false));
        assert!(!preference_changed(true, true));
        assert!(preference_changed(false, true));
        assert!(preference_changed(true, false));
    }

    #[test]
    fn failure_notice_explains_that_the_preference_was_not_lost() {
        let notice = failure_notice("permission denied");
        assert!(notice.contains("saved"));
        assert!(notice.contains("permission denied"));
        assert!(notice.contains("toggle Launch at login off and on again"));
    }
}
