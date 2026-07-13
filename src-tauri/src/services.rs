//! "Add to Alchemy" in the macOS Services menu — select text, a link, or
//! Finder files in any app and send them here as sources. A minimal NSObject
//! subclass is registered as NSApp's services provider (the NSServices entry
//! in Info.plist advertises it); the handler converts the pasteboard into an
//! alchemy:// URL and routes it like every other inbound intent, so payloads
//! that arrive while the app is still launching are buffered until the
//! frontend router is up. See docs/RFC-macos-integrations.md.
#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::{define_class, msg_send, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSString};
use tauri::AppHandle;

/// The provider outlives everything; a OnceLock hands its callback the app.
static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "AlchemyServicesProvider"]
    struct ServicesProvider;

    impl ServicesProvider {
        /// Selector named by NSMessage in the Info.plist NSServices entry.
        #[unsafe(method(addToAlchemy:userData:error:))]
        fn add_to_alchemy(
            &self,
            pboard: &NSPasteboard,
            _user_data: Option<&NSString>,
            _error: *mut *mut NSString,
        ) {
            handle_pasteboard(pboard);
        }
    }
);

impl ServicesProvider {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Register the provider on NSApp. Called once from setup (main thread).
pub fn setup(app: &tauri::App) {
    let _ = APP.set(app.handle().clone());
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let provider = ServicesProvider::new(mtm);
    let ns_app = NSApplication::sharedApplication(mtm);
    unsafe { ns_app.setServicesProvider(Some(&provider)) };
    // AppKit holds only a weak-ish reference; the provider must live forever.
    std::mem::forget(provider);
}

/// Finder files become file sources; a lone link becomes a URL source;
/// anything else is captured as a pasted-text source.
fn handle_pasteboard(pb: &NSPasteboard) {
    let Some(app) = APP.get() else { return };
    let encode = crate::integrations::encode;

    let mut files: Vec<String> = Vec::new();
    {
        // The classic filenames type is what NSServices delivers for Finder
        // selections (an NSArray of path strings).
        let t = NSString::from_str("NSFilenamesPboardType");
        if let Some(plist) = pb.propertyListForType(&t) {
            if let Ok(arr) = plist.downcast::<NSArray>() {
                for i in 0..arr.count() {
                    if let Ok(s) = arr.objectAtIndex(i).downcast::<NSString>() {
                        files.push(s.to_string());
                    }
                }
            }
        }
    }
    if !files.is_empty() {
        let q: Vec<String> = files
            .iter()
            .map(|p| format!("file={}", encode(p)))
            .collect();
        crate::integrations::route_url(app, format!("alchemy://add?{}", q.join("&")));
        return;
    }

    let text = unsafe { pb.stringForType(NSPasteboardTypeString) }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let url = if (text.starts_with("http://") || text.starts_with("https://"))
        && !text.contains(char::is_whitespace)
    {
        format!("alchemy://add?url={}", encode(text))
    } else {
        format!(
            "alchemy://add?text={}&title={}",
            encode(text),
            encode("Captured text")
        )
    };
    crate::integrations::route_url(app, url);
}
