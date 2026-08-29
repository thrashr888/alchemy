//! macOS application menu. Custom actions broadcast `menu://…` events whose
//! payload names the target window — each window's frontend ignores events
//! not addressed to it (JS "Any" listeners receive every event regardless of
//! emit target, so target-side filtering is the only reliable routing).
//!
//! The menu is built ONCE: AppKit only auto-populates the windows menu with
//! windows created after it's assigned, so rebuilding the menu would empty
//! the Window list. "Open Recent" is refreshed by mutating its items in
//! place (`fill_recents`).

use tauri::menu::{Menu, MenuItem, MenuItemBuilder, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// How many notebooks Open Recent shows.
const RECENT_LIMIT: usize = 6;

/// One user-facing command — THE single registry the native menu and the
/// Settings → Shortcuts tab both render from (hand-maintained parallel
/// lists are how registered shortcuts went missing from the menu).
///
/// `accelerator: None` on an item that still shows `keys` means the key is
/// handled frontend-side on purpose: native key equivalents win over focused
/// text fields, so context-dependent keys (⌘N, ⌘1–4) and field-respecting
/// keys (⌘F, ⌘←/→) must stay with the frontend's shortcutBlocked guard. The
/// menu item remains for discoverability and mouse use.
pub struct Command {
    pub id: &'static str,
    /// Menu item label; empty = not a menu item (documented gesture only).
    pub menu_label: &'static str,
    /// Native key equivalent set on the menu item.
    pub accelerator: Option<&'static str>,
    /// Display keys for the Shortcuts tab ("⌘ N"); empty = unkeyed.
    pub keys: &'static str,
    /// Shortcuts tab wording; empty = menu-only (not listed in the tab).
    pub label: &'static str,
    /// Where the shortcut applies ("" = everywhere).
    pub context: &'static str,
}

const CMD: &[Command] = &[
    // File
    Command {
        id: "menu-new-notebook",
        menu_label: "New Notebook",
        accelerator: None,
        keys: "⌘ N",
        label: "New notebook",
        context: "Home",
    },
    Command {
        id: "menu-new-note",
        menu_label: "New Note",
        accelerator: None,
        keys: "⌘ N",
        label: "New note",
        context: "Notebook",
    },
    // ⌥⌘N, not ⌘N or ⇧⌘N: both are spoken for (⌘N is the context-dependent
    // new notebook/new note, ⇧⌘N is New Window). This one is NOT
    // context-dependent — a new chat means the same thing everywhere — so it
    // can carry a real key equivalent, and ⌥⌘N means nothing to a text field.
    Command {
        id: "menu-new-chat",
        menu_label: "New Chat",
        accelerator: Some("CmdOrCtrl+Alt+N"),
        keys: "⌥ ⌘ N",
        label: "New chat across every notebook",
        context: "",
    },
    Command {
        id: "menu-new-window",
        menu_label: "New Window",
        accelerator: Some("CmdOrCtrl+Shift+N"),
        keys: "⇧ ⌘ N",
        label: "New window",
        context: "",
    },
    Command {
        id: "menu-add-files",
        menu_label: "Add Files…",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    Command {
        id: "menu-add-url",
        menu_label: "Add URL Source…",
        accelerator: Some("CmdOrCtrl+Shift+U"),
        keys: "⇧ ⌘ U",
        label: "Add a URL source",
        context: "Notebook",
    },
    Command {
        id: "menu-add-clipboard",
        menu_label: "Add Clipboard Source…",
        accelerator: Some("CmdOrCtrl+Alt+V"),
        keys: "⌥ ⌘ V",
        label: "Add the clipboard as a source",
        context: "",
    },
    // Edit
    Command {
        id: "menu-find",
        menu_label: "Find",
        accelerator: None,
        keys: "⌘ F",
        label: "Find in source, gallery, or home",
        context: "",
    },
    // View
    Command {
        id: "menu-search",
        menu_label: "Search & Commands…",
        accelerator: Some("CmdOrCtrl+K"),
        keys: "⌘ K",
        label: "Open the command menu",
        context: "",
    },
    Command {
        id: "menu-back",
        menu_label: "Back",
        accelerator: None,
        keys: "⌘ [",
        label: "Back (⌘ ← too)",
        context: "",
    },
    Command {
        id: "menu-forward",
        menu_label: "Forward",
        accelerator: None,
        keys: "⌘ ]",
        label: "Forward (⌘ → too)",
        context: "",
    },
    // Where you go, kept as its own group above the sidebar toggles below:
    // these change the center of Home, they don't show or hide a panel.
    // Unkeyed — ⌘1–4 belong to the sidebars.
    Command {
        id: "menu-home-notebooks",
        menu_label: "Go to Notebooks",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    Command {
        id: "menu-home-chat",
        menu_label: "Go to Chats",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    Command {
        id: "menu-home-registry",
        menu_label: "Go to Registry",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    // Two groups of sidebar toggles, one per view, in rail order — and ⌘1–4
    // runs down each group the same way. The same four keys legitimately mean
    // different sidebars in Home and in a notebook because the webview's
    // keydown handler (App.tsx) reads the view before it dispatches, and the
    // group that doesn't belong to that view is greyed out
    // (`set_menu_context`). That is also why none of the eight carries a
    // native accelerator: a key equivalent is global to the process and would
    // fire whichever view is on screen — the `keys` column documents them for
    // Settings → Shortcuts instead.
    Command {
        id: "menu-toggle-home-chats",
        menu_label: "Chats",
        accelerator: None,
        keys: "⌘ 1",
        label: "Show or hide Chats",
        context: "Home",
    },
    Command {
        id: "menu-toggle-home-staff",
        menu_label: "Staff",
        accelerator: None,
        keys: "⌘ 2",
        label: "Show or hide Staff",
        context: "Home",
    },
    Command {
        id: "menu-toggle-home-brief",
        menu_label: "Brief",
        accelerator: None,
        keys: "⌘ 3",
        label: "Show or hide Brief",
        context: "Home",
    },
    Command {
        id: "menu-toggle-home-reports",
        menu_label: "Latest Reports",
        accelerator: None,
        keys: "⌘ 4",
        label: "Show or hide Latest Reports",
        context: "Home",
    },
    Command {
        id: "menu-toggle-sources",
        menu_label: "Sources",
        accelerator: None,
        keys: "⌘ 1",
        label: "Show or hide Sources",
        context: "Notebook",
    },
    Command {
        id: "menu-toggle-studio",
        menu_label: "Studio",
        accelerator: None,
        keys: "⌘ 2",
        label: "Show or hide Studio",
        context: "Notebook",
    },
    Command {
        id: "menu-toggle-gallery",
        menu_label: "Gallery",
        accelerator: None,
        keys: "⌘ 3",
        label: "Show or hide Gallery",
        context: "Notebook",
    },
    Command {
        id: "menu-toggle-ledger",
        menu_label: "Ledger",
        accelerator: None,
        keys: "⌘ 4",
        label: "Show or hide Ledger",
        context: "Notebook",
    },
    Command {
        id: "menu-toggle-glass",
        menu_label: "Liquid Glass",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    // Notebook
    Command {
        id: "menu-export-note",
        menu_label: "Export Note…",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    Command {
        id: "menu-archive-notebook",
        menu_label: "Archive Notebook",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    Command {
        id: "menu-delete-notebook",
        menu_label: "Delete Notebook…",
        accelerator: None,
        keys: "",
        label: "",
        context: "",
    },
    // Reader-local keys (frontend-owned; documented here)
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⌘ [",
        label: "Reader back",
        context: "Reader",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⌘ ]",
        label: "Reader forward",
        context: "Reader",
    },
    // Selection and composer gestures (no menu items)
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⌘ A",
        label: "Select all sources or notes",
        context: "Notebook",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⇧ click",
        label: "Select a range of rows",
        context: "Notebook",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⌘ click",
        label: "Add or remove a row from the selection",
        context: "Notebook",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⌫",
        label: "Remove the selected rows",
        context: "Notebook",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "↩",
        label: "Send message · next find match",
        context: "",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⇧ ↩",
        label: "New line in the composer",
        context: "",
    },
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "esc",
        label: "Close dialog or menu · clear selection",
        context: "",
    },
    // The global hotkey lived only in the tray until now — this row is its
    // in-app documentation.
    Command {
        id: "",
        menu_label: "",
        accelerator: None,
        keys: "⌥ space",
        label: "Ask Alchemy from anywhere (works outside the app)",
        context: "",
    },
    // App/settings staples (kept here so the tab lists them from the table)
    Command {
        id: "menu-settings",
        menu_label: "Settings…",
        accelerator: Some("CmdOrCtrl+,"),
        keys: "⌘ ,",
        label: "Open Settings",
        context: "",
    },
];

/// Row shape the Shortcuts tab renders.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutRow {
    pub keys: String,
    pub label: String,
    pub context: String,
}

/// Every command with a Shortcuts-tab label, in registry order.
pub fn shortcut_rows() -> Vec<ShortcutRow> {
    CMD.iter()
        .filter(|c| !c.label.is_empty())
        .map(|c| ShortcutRow {
            keys: c.keys.to_string(),
            label: c.label.to_string(),
            context: c.context.to_string(),
        })
        .collect()
}

/// Build one registry-backed menu item.
fn cmd_item(app: &AppHandle, id: &str) -> tauri::Result<MenuItem<Wry>> {
    let c = CMD
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("menu command {id} missing from the registry"));
    let mut b = MenuItemBuilder::with_id(c.id, c.menu_label);
    if let Some(a) = c.accelerator {
        b = b.accelerator(a);
    }
    b.build(app)
}

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
    pub themes: Submenu<Wry>,
    pub generate: Submenu<Wry>,
    pub context: ViewContextMenu,
}

/// The View menu's two view-specific groups, held so their `enabled` can
/// follow the frontend's current view. Home's sidebar toggles and a
/// notebook's panel toggles are each dead in the other view, and a menu item
/// that does nothing is worse than one that is visibly unavailable.
pub struct ViewContextMenu {
    home: Vec<MenuItem<Wry>>,
    notebook: Vec<MenuItem<Wry>>,
}

impl ViewContextMenu {
    /// Enable exactly the group that belongs to the view now on screen.
    pub fn apply(&self, in_notebook: bool) -> tauri::Result<()> {
        for it in &self.home {
            it.set_enabled(!in_notebook)?;
        }
        for it in &self.notebook {
            it.set_enabled(in_notebook)?;
        }
        Ok(())
    }
}

/// Tell the native menu which view the frontend is showing, so the View
/// menu's Home-only and notebook-only toggles enable and disable with it.
/// Idempotent — the frontend calls it whenever the open notebook changes.
#[tauri::command]
pub fn set_menu_context(
    ctx: tauri::State<'_, ViewContextMenu>,
    in_notebook: bool,
) -> Result<(), String> {
    ctx.apply(in_notebook).map_err(|err| err.to_string())
}

/// Managed handles for the frontend-filled submenus (theme names and studio
/// generators live in TypeScript; the frontend pushes them over IPC at
/// startup, same in-place mutation as Open Recent).
pub struct ThemeMenu(pub Submenu<Wry>);
pub struct GenerateMenu(pub Submenu<Wry>);

/// Build the full app menu. NOTE: mark `window` as the NSApp windows menu
/// only AFTER `app.set_menu` — the underlying NSMenu doesn't exist until the
/// menu is attached, so marking earlier silently assigns nothing.
pub fn build(app: &AppHandle, recents: &[(String, String)]) -> tauri::Result<AppMenu> {
    let settings = cmd_item(app, "menu-settings")?;
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

    // ⌥⌘V, not ⇧⌘V, on the clipboard add: menu key equivalents win over
    // focused text fields, and ⇧⌘V is the platform-wide Paste and Match
    // Style — binding it here made typing users ingest a source instead of
    // pasting. (The registry's accelerator column carries this.)
    //
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
        .item(&cmd_item(app, "menu-new-notebook")?)
        .item(&cmd_item(app, "menu-new-note")?)
        .item(&cmd_item(app, "menu-new-chat")?)
        .item(&cmd_item(app, "menu-new-window")?)
        .item(&recent_menu)
        .separator()
        .item(&cmd_item(app, "menu-add-files")?)
        .item(&cmd_item(app, "menu-add-url")?)
        .item(&cmd_item(app, "menu-add-clipboard")?)
        .separator()
        .item(&import_okf)
        .item(&export_okf)
        .separator()
        .close_window()
        .build()?;

    // WKWebView routes clipboard shortcuts through the menu on macOS — the
    // predefined cut/copy/paste items are what make ⌘C/⌘V work in inputs.
    //
    // Two items here deliberately break from that. Find carries no
    // accelerator, because ⌘F must respect focused text fields, so the
    // frontend's shortcutBlocked-guarded handler owns the key. Undo and redo
    // go the other way: they are NOT the predefined items, because ⌘Z now
    // reverses the last app mutation (docs/RFC-professional-grade.md
    // Pillar 5) and that only works if the app receives the keystroke — a
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
        .separator()
        .item(&cmd_item(app, "menu-find")?)
        .build()?;

    // Filled from the frontend (theme names live in themes.ts).
    let theme_menu = SubmenuBuilder::new(app, "Theme").build()?;
    theme_menu.append(
        &MenuItemBuilder::new("Loading themes…")
            .enabled(false)
            .build(app)?,
    )?;
    // No ⌘[/⌘]/⌘←/⌘→ accelerators on Back/Forward: the menu's key equivalents
    // actually WIN over a focused text field (regression: ⌘→ stopped jumping
    // to line end while editing), so the frontend keydown handler in App.tsx
    // owns the shortcut — it guards with shortcutBlocked so text fields keep
    // the line-start/line-end meaning. The menu items stay for
    // discoverability and mouse use.
    // Two groups of sidebar toggles, one per view, and only the view you are
    // in is live: a notebook's Sources/Studio/Gallery/Ledger mean nothing on
    // Home, and Home's four cards mean nothing inside a notebook. Kept as
    // handles so `set_menu_context` can flip `enabled` as the frontend moves
    // between views — the menu itself is never rebuilt. Each group is in rail
    // order, which is also ⌘1–4 order (see the registry above).
    let home_toggles = [
        cmd_item(app, "menu-toggle-home-chats")?,
        cmd_item(app, "menu-toggle-home-staff")?,
        cmd_item(app, "menu-toggle-home-brief")?,
        cmd_item(app, "menu-toggle-home-reports")?,
    ];
    let notebook_toggles = [
        cmd_item(app, "menu-toggle-sources")?,
        cmd_item(app, "menu-toggle-studio")?,
        cmd_item(app, "menu-toggle-gallery")?,
        cmd_item(app, "menu-toggle-ledger")?,
    ];
    let mut view = SubmenuBuilder::new(app, "View")
        .item(&cmd_item(app, "menu-back")?)
        .item(&cmd_item(app, "menu-forward")?)
        .separator()
        .item(&cmd_item(app, "menu-search")?)
        .separator()
        .item(&cmd_item(app, "menu-home-notebooks")?)
        .item(&cmd_item(app, "menu-home-chat")?)
        .item(&cmd_item(app, "menu-home-registry")?)
        .separator();
    for it in &home_toggles {
        view = view.item(it);
    }
    view = view.separator();
    for it in &notebook_toggles {
        view = view.item(it);
    }
    let view_menu = view
        .separator()
        .item(&theme_menu)
        .item(&cmd_item(app, "menu-toggle-glass")?)
        .separator()
        .fullscreen()
        .build()?;

    // Filled from the frontend (the generator roster lives in
    // studioArtifacts.tsx).
    let generate_menu = SubmenuBuilder::new(app, "Generate").build()?;
    generate_menu.append(
        &MenuItemBuilder::new("Loading generators…")
            .enabled(false)
            .build(app)?,
    )?;
    let notebook_menu = SubmenuBuilder::new(app, "Notebook")
        .item(&generate_menu)
        .item(&cmd_item(app, "menu-export-note")?)
        .separator()
        .item(&cmd_item(app, "menu-archive-notebook")?)
        .item(&cmd_item(app, "menu-delete-notebook")?)
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
            &notebook_menu,
            &window_menu,
            &help_menu,
        ],
    )?;
    let context = ViewContextMenu {
        home: home_toggles.into(),
        notebook: notebook_toggles.into(),
    };
    // The app opens on Home, so start there rather than with both groups
    // live for the one frame before the frontend reports in.
    context.apply(false)?;
    Ok(AppMenu {
        menu,
        recent: recent_menu,
        window: window_menu,
        themes: theme_menu,
        generate: generate_menu,
        context,
    })
}

/// Replace the Theme submenu's items (frontend pushes the theme list at
/// startup — same in-place mutation as Open Recent). `current` gets a
/// leading dot; a click re-fills with the new selection.
pub fn fill_themes(
    app: &AppHandle,
    submenu: &Submenu<Wry>,
    themes: &[(String, String)],
    current: &str,
) -> tauri::Result<()> {
    while submenu.remove_at(0)?.is_some() {}
    for (id, label) in themes {
        let text = if id == current {
            format!("● {label}")
        } else {
            label.clone()
        };
        submenu.append(&MenuItemBuilder::with_id(format!("theme:{id}"), text).build(app)?)?;
    }
    Ok(())
}

/// Replace the Generate submenu's items (the studio generator roster,
/// pushed from the frontend at startup).
pub fn fill_generators(
    app: &AppHandle,
    submenu: &Submenu<Wry>,
    generators: &[(String, String)],
) -> tauri::Result<()> {
    while submenu.remove_at(0)?.is_some() {}
    for (kind, label) in generators {
        submenu.append(&MenuItemBuilder::with_id(format!("generate:{kind}"), label).build(app)?)?;
    }
    Ok(())
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
        // Opening a notebook can be asked for from places where the app is
        // not frontmost — the menu bar extra, and the Dock's right-click
        // menu. Emitting alone would switch notebooks behind whatever the
        // user is looking at, so bring the window forward too.
        if let Some(win) = windows.get(&target) {
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
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
