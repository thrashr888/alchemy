//! Menu item images on macOS 26+.
//!
//! Since Tahoe, AppKit hides an `NSMenuItem`'s image unless the item opts
//! in with `preferredImageVisibility = .visible` (symbol images first, and
//! on macOS 27 every image). muda sets the image and nothing else, and
//! Tauri's menu API has no handle to the item, so the app's native menus —
//! every RowMenu row with an icon — came up text-only. This hooks
//! `-[NSMenuItem setImage:]` process-wide: after the original runs, an item
//! that just received an image is marked visible. Items without images are
//! untouched, and on a macOS that lacks the property nothing changes.

#[cfg(target_os = "macos")]
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
static ORIG_SET_IMAGE: OnceLock<usize> = OnceLock::new();

#[cfg(target_os = "macos")]
type SetImageFn = unsafe extern "C-unwind" fn(
    *mut objc2::runtime::AnyObject,
    objc2::runtime::Sel,
    *mut objc2::runtime::AnyObject,
);

/// Install the hook. Idempotent; a no-op off macOS or when the class is
/// missing the selector (a macOS older than the property).
pub fn install() {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::runtime::AnyClass;
        use objc2::{msg_send, sel};

        if ORIG_SET_IMAGE.get().is_some() {
            return;
        }
        let Some(cls) = AnyClass::get(c"NSMenuItem") else {
            return;
        };
        let has_property: bool = msg_send![
            cls,
            instancesRespondToSelector: sel!(setPreferredImageVisibility:)
        ];
        if !has_property {
            return;
        }
        if let Some(orig) = objc2::ffi::class_replaceMethod(
            cls as *const _ as *mut AnyClass,
            sel!(setImage:),
            std::mem::transmute::<SetImageFn, objc2::runtime::Imp>(set_image as SetImageFn),
            c"v@:@".as_ptr(),
        ) {
            let _ = ORIG_SET_IMAGE.set(orig as usize);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C-unwind" fn set_image(
    this: *mut objc2::runtime::AnyObject,
    sel: objc2::runtime::Sel,
    image: *mut objc2::runtime::AnyObject,
) {
    use objc2::msg_send;

    if let Some(orig) = ORIG_SET_IMAGE.get() {
        let orig: SetImageFn = std::mem::transmute::<usize, SetImageFn>(*orig);
        orig(this, sel, image);
    }
    if !image.is_null() {
        // NSMenuItemImageVisibility: 0 automatic, 1 visible, 2 hidden.
        let _: () = msg_send![this, setPreferredImageVisibility: 1isize];
    }
}
