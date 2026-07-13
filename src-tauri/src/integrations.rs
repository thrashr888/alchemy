//! macOS ambient integrations: alchemy:// deep links, the menu bar extra +
//! global hotkey, Services capture, and Spotlight indexing. See
//! docs/RFC-macos-integrations.md.
//!
//! Every inbound intent — deep link, tray click, Services payload, Spotlight
//! hit — funnels through one URL router. The backend buffers alchemy:// URLs
//! until the frontend declares itself ready (`integrations_ready`), then
//! forwards them as `integrations://url` events; the frontend owns the
//! routing table, so all entry points behave identically.

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_deep_link::DeepLinkExt;

/// Buffered alchemy:// URLs waiting for the frontend router to come up.
#[derive(Default)]
pub struct Router {
    ready: bool,
    pending: Vec<String>,
}
pub struct RouterState(pub std::sync::Mutex<Router>);

/// The tray's recent-notebooks submenu, mutated in place alongside the app
/// menu's Open Recent (rebuild_app_menu fills both).
pub struct TrayRecents(pub tauri::menu::Submenu<Wry>);

/// Deliver an alchemy:// URL to the frontend router (or hold it for init).
/// Every intent also summons the window — that's the point of all of them.
pub fn route_url(app: &AppHandle, url: String) {
    focus_main(app);
    let state = app.state::<RouterState>();
    let mut r = state.0.lock().unwrap();
    if r.ready {
        let _ = app.emit("integrations://url", url);
    } else {
        r.pending.push(url);
    }
}

/// Bring the main window to the front.
pub fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Summon the window and open the ask surface (⌘K palette).
fn summon_ask(app: &AppHandle) {
    focus_main(app);
    let _ = app.emit("integrations://ask", ());
}

/// The frontend calls this once its `integrations://url` listener is live;
/// returns (and clears) anything that arrived before then.
#[tauri::command]
pub fn integrations_ready(state: tauri::State<'_, RouterState>) -> Vec<String> {
    let mut r = state.0.lock().unwrap();
    r.ready = true;
    std::mem::take(&mut r.pending)
}

/// Note id -> owning notebook id, for alchemy://note/<id> routing.
#[tauri::command]
pub async fn locate_note(
    state: tauri::State<'_, crate::commands::AppState>,
    note_id: String,
) -> Result<Option<String>, String> {
    Ok(state
        .db
        .get_note(&note_id)
        .await
        .map_err(|e| e.to_string())?
        .map(|n| n.notebook_id))
}

pub fn encode(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// Tray "Add Clipboard as Source": URL-shaped text becomes a URL source,
/// anything else a pasted-text source — routed like any other alchemy:// add.
fn add_clipboard(app: &AppHandle) {
    let Some(text) = read_clipboard() else {
        let _ = app.emit("integrations://toast", "Clipboard has no text");
        return;
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        let _ = app.emit("integrations://toast", "Clipboard has no text");
        return;
    }
    let url = if (text.starts_with("http://") || text.starts_with("https://"))
        && !text.contains(char::is_whitespace)
    {
        format!("alchemy://add?url={}", encode(&text))
    } else {
        format!(
            "alchemy://add?text={}&title={}",
            encode(&text),
            encode("From clipboard")
        )
    };
    route_url(app, url);
}

#[cfg(target_os = "macos")]
fn read_clipboard() -> Option<String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        pb.stringForType(NSPasteboardTypeString)
            .map(|s| s.to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn read_clipboard() -> Option<String> {
    None
}

/// Show or hide the menu bar extra (Settings → General, and startup config).
pub fn set_tray_visible(app: &AppHandle, visible: bool) {
    if let Some(tray) = app.tray_by_id("alchemy-tray") {
        let _ = tray.set_visible(visible);
    }
}

/// Wire the whole surface: deep-link handler, tray menu, global hotkey.
pub fn setup(
    app: &tauri::App,
    recents: &[(String, String)],
    tray_enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(RouterState(std::sync::Mutex::new(Router::default())));

    // alchemy:// URLs (registered via Info.plist in the bundle; macOS only
    // routes them to bundled apps, so dev testing uses a debug bundle).
    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            route_url(&handle, url.to_string());
        }
    });

    // Menu bar extra. Recents mutate in place via rebuild_app_menu, same as
    // the app menu's Open Recent.
    let handle = app.handle().clone();
    let recent_menu = SubmenuBuilder::new(app, "Recent Notebooks").build()?;
    crate::menu::fill_recents(&handle, &recent_menu, recents)?;
    let tray_menu = MenuBuilder::new(app)
        .item(&MenuItemBuilder::with_id("tray:open", "Open Alchemy").build(app)?)
        .item(
            &MenuItemBuilder::with_id("tray:ask", "Ask Alchemy")
                .accelerator("Alt+Space")
                .build(app)?,
        )
        .separator()
        .item(&MenuItemBuilder::with_id("tray:clipboard", "Add Clipboard as Source").build(app)?)
        .item(&recent_menu)
        .build()?;
    app.manage(TrayRecents(recent_menu));

    // The menu bar wants a monochrome template glyph, not the app icon (the
    // squircle just flattens to a rounded blob). tray.png is the sigil drawn
    // black-on-transparent at 22pt@2x; macOS recolors it for the bar.
    let glyph = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))
        .expect("tray.png is a valid PNG");
    tauri::tray::TrayIconBuilder::with_id("alchemy-tray")
        .icon(glyph)
        .icon_as_template(true)
        .tooltip("Alchemy")
        .menu(&tray_menu)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "tray:open" => focus_main(app),
            "tray:ask" => summon_ask(app),
            "tray:clipboard" => add_clipboard(app),
            id if id.starts_with("recent:") => {
                focus_main(app);
                crate::menu::handle_event(app, id);
            }
            _ => {}
        })
        .build(app)?;
    set_tray_visible(app.handle(), tray_enabled);

    // Global hotkey: ⌥Space summons the ask surface from anywhere.
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = "Alt+Space".parse().expect("valid shortcut");
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _s, event| {
            if event.state() == ShortcutState::Pressed {
                summon_ask(app);
            }
        })?;

    Ok(())
}
