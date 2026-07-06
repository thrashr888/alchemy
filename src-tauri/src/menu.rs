//! macOS application menu. Custom actions broadcast `menu://…` events whose
//! payload names the target window — each window's frontend ignores events
//! not addressed to it (JS "Any" listeners receive every event regardless of
//! emit target, so target-side filtering is the only reliable routing).
//!
//! The menu is built ONCE: AppKit only auto-populates the windows menu with
//! windows created after it's assigned, so rebuilding the menu would empty
//! the Window list. "Open Recent" is refreshed by mutating its items in
//! place (`fill_recents`).

use tauri::menu::{Menu, MenuItemBuilder, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// How many notebooks Open Recent shows.
const RECENT_LIMIT: usize = 6;

/// Managed handle to the Open Recent submenu, for in-place refreshes.
pub struct RecentMenu(pub Submenu<Wry>);

#[derive(Clone, serde::Serialize)]
struct MenuPayload {
    /// Window label this action is addressed to.
    target: String,
    id: String,
}

/// The built menu plus the submenu handles that get touched after setup.
pub struct AppMenu {
    pub menu: Menu<Wry>,
    pub recent: Submenu<Wry>,
    pub window: Submenu<Wry>,
}

/// Build the full app menu. NOTE: mark `window` as the NSApp windows menu
/// only AFTER `app.set_menu` — the underlying NSMenu doesn't exist until the
/// menu is attached, so marking earlier silently assigns nothing.
pub fn build(app: &AppHandle, recents: &[(String, String)]) -> tauri::Result<AppMenu> {
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

    let recent_menu = SubmenuBuilder::new(app, "Open Recent").build()?;
    fill_recents(app, &recent_menu, recents)?;

    let new_window = MenuItemBuilder::with_id("menu-new-window", "New Window")
        .accelerator("CmdOrCtrl+Shift+N")
        .build(app)?;
    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .item(&recent_menu)
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

    let menu = Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
    )?;
    Ok(AppMenu {
        menu,
        recent: recent_menu,
        window: window_menu,
    })
}

/// Replace the Open Recent items in place (the menu itself is never rebuilt).
pub fn fill_recents(
    app: &AppHandle,
    submenu: &Submenu<Wry>,
    recents: &[(String, String)],
) -> tauri::Result<()> {
    while submenu.remove_at(0)?.is_some() {}
    if recents.is_empty() {
        submenu.append(
            &MenuItemBuilder::new("No notebooks yet")
                .enabled(false)
                .build(app)?,
        )?;
        return Ok(());
    }
    for (id, title) in recents.iter().take(RECENT_LIMIT) {
        submenu.append(&MenuItemBuilder::with_id(format!("recent:{id}"), title).build(app)?)?;
    }
    Ok(())
}

/// Address a menu click to the focused window ("main", then any, as
/// fallbacks). The event broadcasts, but only the addressed window acts.
pub fn handle_event(app: &AppHandle, id: &str) {
    let windows = app.webview_windows();
    let target = windows
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .map(|w| w.label().to_string())
        .or_else(|| windows.contains_key("main").then(|| "main".to_string()))
        .or_else(|| windows.keys().next().cloned());
    let Some(target) = target else { return };
    if let Some(nb) = id.strip_prefix("recent:") {
        let _ = app.emit(
            "menu://open-notebook",
            MenuPayload {
                target,
                id: nb.to_string(),
            },
        );
    } else {
        let _ = app.emit(
            "menu://action",
            MenuPayload {
                target,
                id: id.to_string(),
            },
        );
    }
}
