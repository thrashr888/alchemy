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
    let check_updates =
        MenuItemBuilder::with_id("menu-check-updates", "Check for Updates…").build(app)?;
    let about = MenuItemBuilder::with_id("menu-about", "About Alchemy").build(app)?;
    let app_menu = SubmenuBuilder::new(app, "Alchemy")
        .item(&about)
        .separator()
        .item(&settings)
        .item(&check_updates)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        // A custom Quit instead of the predefined item: quitting must set the
        // intentional-exit flag, or the residency guard in lib.rs would
        // prevent it (docs/RFC-night-shift.md).
        .item(
            &MenuItemBuilder::with_id("menu-quit", "Quit Alchemy")
                .accelerator("CmdOrCtrl+Q")
                .build(app)?,
        )
        .build()?;

    let recent_menu = SubmenuBuilder::new(app, "Open Recent").build()?;
    fill_recents(app, &recent_menu, recents)?;

    let new_window = MenuItemBuilder::with_id("menu-new-window", "New Window")
        .accelerator("CmdOrCtrl+Shift+N")
        .build(app)?;
    let add_url = MenuItemBuilder::with_id("menu-add-url", "Add URL Source…")
        .accelerator("CmdOrCtrl+Shift+U")
        .build(app)?;
    // ⌥⌘V, not ⇧⌘V: menu key equivalents win over focused text fields, and
    // ⇧⌘V is the platform-wide Paste and Match Style — binding it here made
    // typing users ingest a source instead of pasting.
    let add_clipboard = MenuItemBuilder::with_id("menu-add-clipboard", "Add Clipboard Source…")
        .accelerator("CmdOrCtrl+Alt+V")
        .build(app)?;
    // One export verb: the .okf.zip is the notebook's portable form (share
    // it, back it up, unzip it for an OKF folder) — the separate
    // folder-export and "share as zip" items said the same thing twice.
    let export_okf = MenuItemBuilder::with_id("menu-export-okf", "Export Notebook…")
        .accelerator("CmdOrCtrl+Shift+E")
        .build(app)?;
    let import_okf = MenuItemBuilder::with_id(
        "menu-import-okf",
        "Import Notebook (Open Knowledge Format)…",
    )
    .build(app)?;
    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .item(&recent_menu)
        .separator()
        .item(&add_url)
        .item(&add_clipboard)
        .separator()
        .item(&import_okf)
        .item(&export_okf)
        .separator()
        .close_window()
        .build()?;

    // WKWebView routes clipboard shortcuts through the menu on macOS — the
    // predefined cut/copy/paste items are what make ⌘C/⌘V work in inputs.
    //
    // Undo and redo are deliberately NOT the predefined items: ⌘Z now
    // reverses the last app mutation (docs/RFC-professional-grade.md
    // Pillar 5), which only works if the app receives the keystroke — and a
    // menu accelerator consumes it before any webview keydown fires. Having
    // claimed it, the frontend owes text fields their own undo back, so
    // textUndo.ts offers a focused editor or input first claim and falls
    // through to the session history stack only when nothing is being typed.
    let undo = MenuItemBuilder::with_id("menu-undo", "Undo")
        .accelerator("CmdOrCtrl+Z")
        .build(app)?;
    let redo = MenuItemBuilder::with_id("menu-redo", "Redo")
        .accelerator("Shift+CmdOrCtrl+Z")
        .build(app)?;
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .item(&undo)
        .item(&redo)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let search = MenuItemBuilder::with_id("menu-search", "Search & Commands…")
        .accelerator("CmdOrCtrl+K")
        .build(app)?;
    // No ⌘←/⌘→ accelerators here: the menu's key equivalents actually WIN
    // over a focused text field (regression: ⌘→ stopped jumping to line end
    // while editing), so the frontend keydown handler in App.tsx owns the
    // shortcut — it guards with shortcutBlocked so text fields keep the
    // line-start/line-end meaning. The menu items stay for discoverability
    // and mouse use.
    let back = MenuItemBuilder::with_id("menu-back", "Back").build(app)?;
    let forward = MenuItemBuilder::with_id("menu-forward", "Forward").build(app)?;
    let view_menu = SubmenuBuilder::new(app, "View")
        .item(&back)
        .item(&forward)
        .separator()
        .item(&search)
        .separator()
        .fullscreen()
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .build()?;

    // The diagnostics log (docs/RFC-diagnostics.md) lives in a folder no one
    // should have to be talked through finding over a support thread.
    let log = MenuItemBuilder::with_id("menu-reveal-log", "Show Diagnostics Log").build(app)?;
    let help_menu = SubmenuBuilder::new(app, "Help").item(&log).build()?;

    let menu = Menu::with_items(
        app,
        &[
            &app_menu,
            &file_menu,
            &edit_menu,
            &view_menu,
            &window_menu,
            &help_menu,
        ],
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
    // Clipboard adds run entirely backend-side (pasteboard access).
    if id == "menu-add-clipboard" {
        crate::integrations::add_clipboard(app);
        return;
    }
    if id == "menu-quit" {
        crate::scheduler::request_quit(app);
        return;
    }
    // Reveals a folder — no window involved, and it must work even when the
    // one thing broken is the window.
    if id == "menu-reveal-log" {
        if let Err(err) = crate::commands::reveal_log() {
            crate::diagnostics::error("reveal-log", err);
        }
        return;
    }
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
