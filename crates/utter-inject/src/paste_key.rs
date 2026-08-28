//! Platform-specific primitive that asks the focused application to paste.
//!
//! `ClipboardPasteInjector` owns the clipboard lifecycle; this module owns
//! only the synthetic key chord. Linux retains its layout-independent
//! uinput Shift+Insert implementation while macOS uses Command+V.

#[cfg(target_os = "linux")]
mod platform {
    use utter_core::InjectError;

    pub(crate) struct PasteKey {
        keyboard: crate::uinput_kbd::VirtualKeyboard,
    }

    impl PasteKey {
        pub(crate) fn new() -> Result<Self, InjectError> {
            Ok(Self {
                keyboard: crate::uinput_kbd::VirtualKeyboard::new()?,
            })
        }

        pub(crate) fn paste(&mut self) -> Result<(), InjectError> {
            self.keyboard.paste()
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_core_graphics::{
        CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
        CGPreflightPostEventAccess,
    };
    use utter_core::InjectError;

    const KEY_COMMAND: u16 = 0x37;
    const KEY_V: u16 = 0x09;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct KeyStep {
        key_code: u16,
        down: bool,
        flags: CGEventFlags,
    }

    fn paste_steps() -> [KeyStep; 4] {
        let command = CGEventFlags::MaskCommand;
        [
            KeyStep {
                key_code: KEY_COMMAND,
                down: true,
                flags: command,
            },
            KeyStep {
                key_code: KEY_V,
                down: true,
                flags: command,
            },
            KeyStep {
                key_code: KEY_V,
                down: false,
                flags: command,
            },
            KeyStep {
                key_code: KEY_COMMAND,
                down: false,
                flags: CGEventFlags::empty(),
            },
        ]
    }

    pub(crate) fn post_event_access() -> bool {
        CGPreflightPostEventAccess()
    }

    pub(crate) struct PasteKey;

    impl PasteKey {
        pub(crate) fn new() -> Result<Self, InjectError> {
            if !post_event_access() {
                return Err(InjectError::NoBackend(
                    "macOS text-injection permission is not granted".to_string(),
                ));
            }

            Ok(Self)
        }

        pub(crate) fn paste(&mut self) -> Result<(), InjectError> {
            if !post_event_access() {
                return Err(InjectError::NoBackend(
                    "macOS text-injection permission was revoked".to_string(),
                ));
            }

            let source =
                CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok_or_else(|| {
                    InjectError::Backend(
                        "failed to create a macOS keyboard event source".to_string(),
                    )
                })?;

            for step in paste_steps() {
                let event = CGEvent::new_keyboard_event(Some(&source), step.key_code, step.down)
                    .ok_or_else(|| {
                        InjectError::Backend(format!(
                            "failed to create macOS keyboard event for key code {}",
                            step.key_code
                        ))
                    })?;
                CGEvent::set_flags(Some(&event), step.flags);
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn command_v_sequence_keeps_command_held_around_v() {
            assert_eq!(
                paste_steps(),
                [
                    KeyStep {
                        key_code: KEY_COMMAND,
                        down: true,
                        flags: CGEventFlags::MaskCommand,
                    },
                    KeyStep {
                        key_code: KEY_V,
                        down: true,
                        flags: CGEventFlags::MaskCommand,
                    },
                    KeyStep {
                        key_code: KEY_V,
                        down: false,
                        flags: CGEventFlags::MaskCommand,
                    },
                    KeyStep {
                        key_code: KEY_COMMAND,
                        down: false,
                        flags: CGEventFlags::empty(),
                    },
                ]
            );
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use utter_core::InjectError;

    pub(crate) enum PasteKey {}

    impl PasteKey {
        pub(crate) fn new() -> Result<Self, InjectError> {
            Err(InjectError::NoBackend(
                "clipboard paste is not available on this platform".to_string(),
            ))
        }

        pub(crate) fn paste(&mut self) -> Result<(), InjectError> {
            match *self {}
        }
    }
}

pub(crate) use platform::PasteKey;
