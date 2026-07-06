//! macOS application menu. Custom actions emit `menu://…` events to the
//! focused window; the frontend routes them to the same store actions the
//! in-app shortcuts use. "Open Recent" is rebuilt (via `rebuild_app_menu`)
//! whenever the notebook list changes.

use tauri::menu::{Menu, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// How many notebooks Open Recent shows.
const RECENT_LIMIT: usize = 6;

/// Build the full app menu. `recents` is (notebook id, title), newest first.
pub fn build(app: &AppHandle, recents: &[(String, String)]) -> tauri::Result<Menu<Wry>> {
    let settings = MenuItemBuilder::with_id("menu-settings", "Settings…")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let about = MenuItemBuilder::with_id("menu-about", "About Alchemy").build(app)?;
    let app_menu = SubmenuBuilder::new(app, "Alchemy")
        .item(&about)
        .separator()
        .item(&settings)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let mut recent_menu = SubmenuBuilder::new(app, "Open Recent");
    for (id, title) in recents.iter().take(RECENT_LIMIT) {
        recent_menu =
            recent_menu.item(&MenuItemBuilder::with_id(format!("recent:{id}"), title).build(app)?);
    }
    if recents.is_empty() {
        recent_menu = recent_menu.item(
            &MenuItemBuilder::new("No notebooks yet")
                .enabled(false)
                .build(app)?,
        );
    }
    let new_window = MenuItemBuilder::with_id("menu-new-window", "New Window")
        .accelerator("CmdOrCtrl+Shift+N")
        .build(app)?;
    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .item(&recent_menu.build()?)
        .separator()
        .close_window()
        .build()?;

    // WKWebView routes clipboard shortcuts through the menu on macOS — these
    // predefined items are what make ⌘C/⌘V/⌘Z work in inputs.
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let search = MenuItemBuilder::with_id("menu-search", "Search & Commands…")
        .accelerator("CmdOrCtrl+K")
        .build(app)?;
    let view_menu = SubmenuBuilder::new(app, "View")
        .item(&search)
        .separator()
        .fullscreen()
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .build()?;
    // Hand the submenu to AppKit as THE windows menu: macOS then appends and
    // maintains the list of open windows (titled per notebook) automatically.
    #[cfg(target_os = "macos")]
    window_menu.set_as_windows_menu_for_nsapp()?;

    Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
    )
}

/// Route a menu click to the focused window (falling back to any window) so
/// e.g. Settings opens once, where the user is — not in every window.
pub fn handle_event(app: &AppHandle, id: &str) {
    let windows = app.webview_windows();
    let target = windows
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| windows.values().next());
    let Some(win) = target else { return };
    // emit_to targets ONE window — plain emit broadcasts to every window,
    // which turned "New Window" into exponential window spawning.
    if let Some(nb) = id.strip_prefix("recent:") {
        let _ = win.emit_to(win.label(), "menu://open-notebook", nb.to_string());
    } else {
        let _ = win.emit_to(win.label(), "menu://action", id.to_string());
    }
}
