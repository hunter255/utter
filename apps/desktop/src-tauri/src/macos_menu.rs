//! macOS application-menu customisation.
//!
//! Tauri's default application menu uses AppKit's predefined Quit item. On
//! macOS that item terminates `NSApplication` directly, without reliably
//! delivering Tauri's `RunEvent::ExitRequested` first. A live whisper.cpp
//! Metal context can therefore reach C++ static teardown before Utter's
//! runtime worker has released its model buffers. Replacing only that item
//! keeps the native menu intact while routing Command-Q through the same
//! orderly shutdown as the tray's Quit action.

use tauri::menu::{Menu, MenuItem, MenuItemKind};
use tauri::{AppHandle, Wry};

pub(crate) const MENU_QUIT: &str = "app-quit";

pub(crate) fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() else {
        return Ok(menu);
    };

    let items = app_menu.items()?;
    let Some(last_index) = items.len().checked_sub(1) else {
        return Ok(menu);
    };
    if !matches!(items[last_index], MenuItemKind::Predefined(_)) {
        return Ok(menu);
    }

    app_menu.remove_at(last_index)?;
    let quit = MenuItem::with_id(
        app,
        MENU_QUIT,
        format!("Quit {}", app.package_info().name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    app_menu.append(&quit)?;

    Ok(menu)
}

pub(crate) fn is_quit(id: &str) -> bool {
    id == MENU_QUIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_custom_application_quit_id_requests_shutdown() {
        assert!(is_quit(MENU_QUIT));
        assert!(!is_quit("quit"));
        assert!(!is_quit("close"));
    }
}
