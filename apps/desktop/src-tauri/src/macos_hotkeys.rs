//! macOS global-shortcut adapter.
//!
//! The underlying manager must be driven by Tauri's main event loop, so this
//! lives in the desktop shell rather than implementing `HotkeySource`, whose
//! contract runs on a dedicated background thread. Events still enter the
//! existing runtime through its ordinary `HotkeyEvent` channel.

use std::collections::HashMap;
use std::sync::Mutex;

use crossbeam_channel::{unbounded, Receiver, Sender};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};
use utter_inject::{BindingId, HotkeyEvent, HotkeySpec};

use crate::state::AppState;

#[derive(Default)]
struct Route {
    sender: Option<Sender<HotkeyEvent>>,
    bindings: HashMap<u32, BindingId>,
    shortcuts: Vec<Shortcut>,
}

/// Owns the registrations and the currently active runtime channel.
#[derive(Default)]
pub(crate) struct MacosHotkeys {
    route: Mutex<Route>,
}

impl MacosHotkeys {
    /// Atomically replaces the event route after registering a complete new
    /// shortcut set. The new sender is installed empty before OS mutation so
    /// an event can never be delivered to the previous runtime receiver.
    pub(crate) fn replace(
        &self,
        app: &AppHandle,
        specs: &[HotkeySpec],
        profile_ids: &[String],
    ) -> (Receiver<HotkeyEvent>, Option<String>) {
        let (tx, rx) = unbounded();

        let prepared = match prepare(specs, profile_ids) {
            Ok(prepared) => prepared,
            Err(message) => {
                let old = self.take_old_and_install_empty(tx);
                if !old.is_empty() {
                    let _ = app.global_shortcut().unregister_multiple(old);
                }
                return (rx, Some(message));
            }
        };

        let old = self.take_old_and_install_empty(tx.clone());

        let manager = app.global_shortcut();
        if !old.is_empty() {
            if let Err(error) = manager.unregister_multiple(old) {
                return (
                    rx,
                    Some(format!(
                        "failed to replace macOS hotkeys while unregistering the old set: {error}"
                    )),
                );
            }
        }

        let mut registered = Vec::with_capacity(prepared.len());
        for item in &prepared {
            if let Err(error) = manager.register(item.shortcut) {
                if !registered.is_empty() {
                    let _ = manager.unregister_multiple(registered);
                }
                return (
                    rx,
                    Some(format!(
                        "profile \"{}\" hotkey could not be registered on macOS: {error}; \
                         choose another shortcut in Settings",
                        item.profile_id
                    )),
                );
            }
            registered.push(item.shortcut);
        }

        let bindings = prepared
            .iter()
            .map(|item| (item.shortcut.id(), item.binding))
            .collect();
        let mut route = self.route.lock().unwrap_or_else(|e| e.into_inner());
        route.sender = Some(tx);
        route.bindings = bindings;
        route.shortcuts = registered;

        (rx, None)
    }

    fn take_old_and_install_empty(&self, sender: Sender<HotkeyEvent>) -> Vec<Shortcut> {
        let mut route = self.route.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::mem::take(&mut route.shortcuts);
        route.sender = Some(sender);
        route.bindings.clear();
        old
    }

    fn dispatch(&self, shortcut_id: u32, state: ShortcutState) {
        let (sender, binding) = {
            let route = self.route.lock().unwrap_or_else(|e| e.into_inner());
            let Some(binding) = route.bindings.get(&shortcut_id).copied() else {
                return;
            };
            (route.sender.clone(), binding)
        };

        let Some(sender) = sender else {
            return;
        };
        let event = match state {
            ShortcutState::Pressed => HotkeyEvent::Pressed { binding },
            ShortcutState::Released => HotkeyEvent::Released { binding },
        };
        let _ = sender.send(event);
    }
}

#[derive(Debug)]
struct PreparedShortcut {
    shortcut: Shortcut,
    binding: BindingId,
    profile_id: String,
}

fn prepare(specs: &[HotkeySpec], profile_ids: &[String]) -> Result<Vec<PreparedShortcut>, String> {
    if specs.len() != profile_ids.len() {
        return Err("internal hotkey/profile mapping mismatch".to_string());
    }

    let mut by_id = HashMap::<u32, String>::new();
    let mut prepared = Vec::with_capacity(specs.len());
    for (index, (spec, profile_id)) in specs.iter().zip(profile_ids).enumerate() {
        let canonical = spec
            .canonical_shortcut()
            .map_err(|error| format!("profile \"{profile_id}\" hotkey is unavailable: {error}"))?;
        let shortcut: Shortcut = canonical.parse().map_err(|error| {
            format!("profile \"{profile_id}\" hotkey \"{canonical}\" is unavailable: {error}")
        })?;

        if let Some(first_profile) = by_id.insert(shortcut.id(), profile_id.clone()) {
            return Err(format!(
                "profiles \"{first_profile}\" and \"{profile_id}\" use the same hotkey; \
                 choose a different shortcut for one of them"
            ));
        }

        prepared.push(PreparedShortcut {
            shortcut,
            binding: BindingId::from(index),
            profile_id: profile_id.clone(),
        });
    }
    Ok(prepared)
}

/// Builds the plugin once; all later registration is owned by
/// [`MacosHotkeys::replace`]. No JavaScript permission is needed because the
/// frontend never invokes the plugin directly.
pub(crate) fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(
            |app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent| {
                app.state::<AppState>()
                    .macos_hotkeys
                    .dispatch(shortcut.id(), event.state());
            },
        )
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use utter_inject::parse_hotkey;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn preparation_preserves_profile_binding_positions() {
        let specs = [
            parse_hotkey("control+alt+space").unwrap(),
            parse_hotkey("shift+super+r").unwrap(),
        ];
        let prepared = prepare(&specs, &ids(&["en", "ru"])).unwrap();

        assert_eq!(prepared[0].binding, BindingId::from(0));
        assert_eq!(prepared[1].binding, BindingId::from(1));
        assert_eq!(prepared[0].profile_id, "en");
        assert_eq!(prepared[1].profile_id, "ru");
    }

    #[test]
    fn preparation_rejects_modifier_only_and_duplicate_chords() {
        let modifier_only = [parse_hotkey("ctrl+super").unwrap()];
        assert!(prepare(&modifier_only, &ids(&["en"]))
            .unwrap_err()
            .contains("en"));

        let duplicate = [
            parse_hotkey("ctrl+space").unwrap(),
            parse_hotkey("control+space").unwrap(),
        ];
        let error = prepare(&duplicate, &ids(&["en", "ru"])).unwrap_err();
        assert!(error.contains("en"));
        assert!(error.contains("ru"));
    }

    #[test]
    fn dispatch_maps_pressed_and_released_and_ignores_unknown_ids() {
        let adapter = MacosHotkeys::default();
        let shortcut: Shortcut = "control+Space".parse().unwrap();
        let (tx, rx) = unbounded();
        {
            let mut route = adapter.route.lock().unwrap();
            route.sender = Some(tx);
            route.bindings.insert(shortcut.id(), BindingId::from(2));
        }

        adapter.dispatch(shortcut.id() + 1, ShortcutState::Pressed);
        assert!(rx.try_recv().is_err());

        adapter.dispatch(shortcut.id(), ShortcutState::Pressed);
        assert_eq!(
            rx.recv().unwrap(),
            HotkeyEvent::Pressed {
                binding: BindingId::from(2)
            }
        );
        adapter.dispatch(shortcut.id(), ShortcutState::Released);
        assert_eq!(
            rx.recv().unwrap(),
            HotkeyEvent::Released {
                binding: BindingId::from(2)
            }
        );
    }
}
