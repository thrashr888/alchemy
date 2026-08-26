//! Right-click the Dock icon and get your recent notebooks — the place
//! macOS users expect an app to show what it can reopen
//! (docs/RFC-professional-grade.md Pillar 6).
//!
//! AppKit asks the application delegate for this menu via
//! `applicationDockMenu:`, and Tauri owns the delegate. Rather than replace
//! it, the method is grafted onto the delegate's class at runtime — the one
//! seam AppKit offers when another framework got there first.
//!
//! The list has to be answered synchronously on the main thread, so it comes
//! from a cache refreshed by `rebuild_app_menu` (the same command that
//! refills Open Recent and the tray). Clicks route through
//! `menu::handle_event` with the identical `recent:<id>` payload the app menu
//! sends, so the Dock is a third entrance to one code path, not a second
//! implementation of it.
#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use tauri::AppHandle;

/// Matches Open Recent, so the two lists never disagree.
const RECENT_LIMIT: usize = 6;

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
static RECENTS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Cache the notebooks the Dock menu should offer. Called wherever Open
/// Recent is refilled; the Dock reads this when the user right-clicks.
pub fn set_recents(recents: &[(String, String)]) {
    if let Ok(mut held) = RECENTS.lock() {
        *held = recents.iter().take(RECENT_LIMIT).cloned().collect();
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "AlchemyDockMenuTarget"]
    struct DockTarget;

    impl DockTarget {
        /// Every Dock item points here, carrying its notebook id as the
        /// item's represented object.
        #[unsafe(method(openNotebook:))]
        fn open_notebook(&self, sender: &NSMenuItem) {
            let Some(app) = APP.get() else { return };
            let obj: Option<Retained<AnyObject>> = unsafe { msg_send![sender, representedObject] };
            let Some(obj) = obj else { return };
            let id: Retained<NSString> = unsafe { Retained::cast_unchecked(obj) };
            crate::menu::handle_event(app, &format!("recent:{id}"));
        }
    }
);

impl DockTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// The grafted `applicationDockMenu:`. Built fresh per right-click, because
/// the notebook list moves and a stale menu is worse than none.
extern "C" fn dock_menu(_this: &AnyObject, _sel: Sel, _app: &AnyObject) -> *mut NSMenu {
    let Some(mtm) = MainThreadMarker::new() else {
        return std::ptr::null_mut();
    };
    let Ok(recents) = RECENTS.lock() else {
        return std::ptr::null_mut();
    };
    // Returning nil leaves the Dock's own menu untouched, which is the right
    // answer when there is nothing to offer — an empty section would read as
    // broken.
    if recents.is_empty() {
        return std::ptr::null_mut();
    }
    let menu = NSMenu::new(mtm);
    // A disabled header, the way other apps label their Dock sections —
    // without it the notebook titles read as unexplained commands.
    let header = NSMenuItem::new(mtm);
    header.setTitle(&NSString::from_str("Recent"));
    header.setEnabled(false);
    menu.addItem(&header);

    let target = DockTarget::new(mtm);
    for (id, title) in recents.iter() {
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(title));
        unsafe {
            item.setAction(Some(sel!(openNotebook:)));
            item.setTarget(Some(&target));
            item.setRepresentedObject(Some(&NSString::from_str(id)));
        }
        menu.addItem(&item);
    }
    // AppKit reads the menu and lets go; the target must outlive that, and
    // nothing else holds it.
    std::mem::forget(target);
    // Autoreleased, not +1: this delegate method follows the normal
    // convention, so AppKit retains what it needs and releases the rest.
    Retained::autorelease_return(menu)
}

/// Graft `applicationDockMenu:` onto whatever delegate class Tauri installed.
/// Called once from setup, on the main thread.
pub fn setup(app: &tauri::App) {
    let _ = APP.set(app.handle().clone());
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ns_app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = ns_app.delegate() else {
        crate::note!("dock menu: no application delegate to extend");
        return;
    };
    let cls: &AnyClass = unsafe { msg_send![&*delegate, class] };
    // "@@:@" — returns an object, takes (self, _cmd, NSApplication).
    let types = c"@@:@";
    // SAFETY: the signature encoded in `types` matches `dock_menu`, and the
    // class is the live delegate's own — adding a method it does not already
    // implement is additive.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (cls as *const AnyClass).cast_mut(),
            sel!(applicationDockMenu:),
            std::mem::transmute::<
                extern "C" fn(&AnyObject, Sel, &AnyObject) -> *mut NSMenu,
                objc2::runtime::Imp,
            >(dock_menu),
            types.as_ptr(),
        )
    };
    if !added.as_bool() {
        // Already implemented — a future Tauri may grow its own. Leave it
        // alone rather than swizzle over something that works.
        crate::note!("dock menu: delegate already answers applicationDockMenu:");
        return;
    }
    // Re-assign the delegate through nil. AppKit caches which optional
    // delegate methods an object answers at the moment the delegate is set,
    // and Tauri set this one long before the method existed — without a
    // reset the Dock shows its stock menu and the graft looks like it
    // silently failed. Assigning the same object back is not enough; the
    // cache only clears on an actual change, hence the trip through nil.
    ns_app.setDelegate(None);
    ns_app.setDelegate(Some(&delegate));
    crate::note!("dock menu: recent notebooks attached to the Dock icon");
}
