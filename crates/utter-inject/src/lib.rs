//! Global hotkeys and text injection backends for Linux (evdev preferred,
//! X11 `global-hotkey` fallback), with non-Linux stubs so the workspace
//! keeps compiling on Windows and macOS ahead of platform-specific support.

pub mod hotkey;
pub use hotkey::{
    check_linux_permissions, create_source, parse_hotkey, BindingId, HotkeyEvent, HotkeyParseError,
    HotkeyShortcutError, HotkeySource, HotkeySpec, LinuxPermissionReport,
};

pub mod chain;
pub use chain::{injection_order, ChainInjector};

pub mod inject;
pub use inject::{ClipboardOnlyInjector, ClipboardPasteInjector, TypeInjector};

mod clipboard;
#[cfg(target_os = "macos")]
mod clipboard_macos;
#[cfg(target_os = "macos")]
mod clipboard_receipt;
mod modifier_wait;
mod paste_key;
pub use paste_key::{
    request_text_injection_permission, text_injection_permission, TextInjectionPermission,
};
mod uinput_kbd;

#[cfg(target_os = "linux")]
mod hotkey_evdev;
#[cfg(target_os = "linux")]
mod hotkey_x11;
