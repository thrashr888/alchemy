//! Spotlight (CoreSpotlight) indexing: every notebook and note title is a
//! searchable item whose unique identifier IS its alchemy:// URL, so an
//! activation routes through the same frontend router as deep links.
//!
//! Activation arrives via `application:continueUserActivity:` on the app
//! delegate. tao's delegate already implements it (for universal links) and
//! returns NO for everything else, so both it and the will-continue gate are
//! swizzled: Spotlight activities are handled here, anything else falls
//! through to tao's originals. See docs/RFC-macos-integrations.md.
#![cfg(target_os = "macos")]

use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{sel, AnyThread};
use objc2_app_kit::NSApplication;
use objc2_core_spotlight::{
    CSSearchableIndex, CSSearchableItem, CSSearchableItemActionType,
    CSSearchableItemActivityIdentifier, CSSearchableItemAttributeSet,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSUserActivity};
use tauri::{AppHandle, Manager};

const DOMAIN: &str = "com.thrashr888.alchemy";

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

// tao's original delegate IMPs, called for non-Spotlight activities.
static ORIG_WILL: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
static ORIG_CONTINUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

type WillFn = extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject) -> Bool;
type ContinueFn = extern "C-unwind" fn(
    *mut AnyObject,
    Sel,
    *mut AnyObject,
    *mut AnyObject,
    *mut AnyObject,
) -> Bool;

/// One indexable thing: (alchemy:// URL, title, description).
pub struct Entry {
    pub url: String,
    pub title: String,
    pub description: String,
}

/// Replace the whole Alchemy domain with `entries`. Rebuild-from-scratch
/// keeps deletions and renames simple; a few hundred items index in
/// milliseconds. CoreSpotlight is thread-safe — callable from any task.
pub fn reindex(entries: Vec<Entry>) {
    unsafe {
        let index = CSSearchableIndex::defaultSearchableIndex();
        let domain = NSString::from_str(DOMAIN);
        index.deleteSearchableItemsWithDomainIdentifiers_completionHandler(
            &NSArray::from_retained_slice(std::slice::from_ref(&domain)),
            None,
        );
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                #[allow(deprecated)] // initWithContentType needs a UTType dep for no gain
                let attrs = CSSearchableItemAttributeSet::initWithItemContentType(
                    CSSearchableItemAttributeSet::alloc(),
                    &NSString::from_str("public.item"),
                );
                attrs.setTitle(Some(&NSString::from_str(&e.title)));
                attrs.setContentDescription(Some(&NSString::from_str(&e.description)));
                CSSearchableItem::initWithUniqueIdentifier_domainIdentifier_attributeSet(
                    CSSearchableItem::alloc(),
                    Some(&NSString::from_str(&e.url)),
                    Some(&domain),
                    &attrs,
                )
            })
            .collect();
        if !items.is_empty() {
            let count = items.len();
            // Log the outcome — CS failures are silent otherwise, and a
            // wrong entitlement or container issue would look like "works".
            let done = block2::RcBlock::new(move |err: *mut objc2_foundation::NSError| {
                if err.is_null() {
                    eprintln!("spotlight: indexed {count} items");
                } else {
                    eprintln!("spotlight: index failed: {}", (*err).localizedDescription());
                }
            });
            index.indexSearchableItems_completionHandler(
                &NSArray::from_retained_slice(&items),
                Some(&done),
            );
        }
    }
}

/// Gather all notebooks + notes as Spotlight entries.
pub async fn collect(state: &crate::commands::AppState) -> Vec<Entry> {
    let mut entries = Vec::new();
    let Ok(notebooks) = state.db.list_notebooks().await else {
        return entries;
    };
    for nb in &notebooks {
        entries.push(Entry {
            url: format!("alchemy://notebook/{}", nb.id),
            title: nb.title.clone(),
            description: "Alchemy notebook".to_string(),
        });
        if let Ok(notes) = state.db.list_notes(&nb.id).await {
            for n in notes {
                let mut desc: String = n.content.chars().take(200).collect();
                if desc.is_empty() {
                    desc = format!("Note in {}", nb.title);
                }
                entries.push(Entry {
                    url: format!("alchemy://note/{}", n.id),
                    title: n.title,
                    description: desc,
                });
            }
        }
    }
    entries
}

/// Throttled refresh, piggybacked on the frontend's once-a-minute source
/// resync tick — Spotlight lags content by at most ~10 minutes.
const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
static LAST_REFRESH: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

pub async fn refresh_if_due(state: &crate::commands::AppState) {
    {
        let mut last = LAST_REFRESH.lock().unwrap();
        let now = std::time::Instant::now();
        match *last {
            Some(t) if now.duration_since(t) < REFRESH_INTERVAL => return,
            _ => *last = Some(now),
        }
    }
    reindex(collect(state).await);
}

/// Install the activation hooks and schedule the first index build.
pub fn setup(app: &tauri::App) {
    let _ = APP.set(app.handle().clone());
    swizzle_delegate();
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<crate::commands::AppState>();
        // Also stamps LAST_REFRESH so the first resync tick doesn't redo it.
        refresh_if_due(&state).await;
    });
}

fn swizzle_delegate() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ns_app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = ns_app.delegate() else {
        return;
    };
    unsafe {
        let obj: &AnyObject = delegate.as_ref();
        let cls = obj.class() as *const _ as *mut objc2::runtime::AnyClass;
        let will_sel = sel!(application:willContinueUserActivityWithType:);
        let cont_sel = sel!(application:continueUserActivity:restorationHandler:);
        if let Some(orig) = objc2::ffi::class_replaceMethod(
            cls,
            will_sel,
            std::mem::transmute::<WillFn, objc2::runtime::Imp>(will_continue as WillFn),
            c"B@:@@".as_ptr(),
        ) {
            let _ = ORIG_WILL.set(orig as usize);
        }
        if let Some(orig) = objc2::ffi::class_replaceMethod(
            cls,
            cont_sel,
            std::mem::transmute::<ContinueFn, objc2::runtime::Imp>(continue_activity as ContinueFn),
            c"B@:@@@?".as_ptr(),
        ) {
            let _ = ORIG_CONTINUE.set(orig as usize);
        }
    }
}

extern "C-unwind" fn will_continue(
    this: *mut AnyObject,
    _sel: Sel,
    ns_app: *mut AnyObject,
    activity_type: *mut AnyObject,
) -> Bool {
    unsafe {
        let ty = &*(activity_type as *const NSString);
        if ty.isEqualToString(CSSearchableItemActionType) {
            return Bool::YES;
        }
        match ORIG_WILL.get() {
            Some(&imp) => {
                let orig: WillFn = std::mem::transmute(imp);
                orig(this, _sel, ns_app, activity_type)
            }
            None => Bool::NO,
        }
    }
}

extern "C-unwind" fn continue_activity(
    this: *mut AnyObject,
    _sel: Sel,
    ns_app: *mut AnyObject,
    activity: *mut AnyObject,
    restoration: *mut AnyObject,
) -> Bool {
    unsafe {
        let act = &*(activity as *const NSUserActivity);
        if act
            .activityType()
            .isEqualToString(CSSearchableItemActionType)
        {
            let url: Option<String> = act.userInfo().and_then(|info| {
                let key: &AnyObject = CSSearchableItemActivityIdentifier.as_ref();
                info.objectForKey(key)
                    .and_then(|v| v.downcast::<NSString>().ok().map(|s| s.to_string()))
            });
            if let (Some(url), Some(app)) = (url, APP.get()) {
                crate::integrations::route_url(app, url);
                return Bool::YES;
            }
            return Bool::NO;
        }
        match ORIG_CONTINUE.get() {
            Some(&imp) => {
                let orig: ContinueFn = std::mem::transmute(imp);
                orig(this, _sel, ns_app, activity, restoration)
            }
            None => Bool::NO,
        }
    }
}
