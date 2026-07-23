//! Adherence to the macOS Accessibility "Text Size" control.
//!
//! WKWebView paints the whole UI in fixed CSS px and ignores the system
//! Accessibility text size, so a user who enlarges text system-wide (System
//! Settings > Accessibility > Display > Text size, globally or per-app) sees no
//! change in Alchemy. No Tauri plugin covers this. We read the effective size
//! natively and republish it to the webview as a scale factor the CSS folds
//! into its root font-size (`--system-text-scale`); the rem-based type then
//! scales with the OS.
//!
//! Which native signal actually tracks the slider (verified empirically on this
//! Mac, macOS 26/Tahoe): `NSFont.systemFontSize` is the *fixed* 13pt system
//! metric — it reads 13.0 at every setting and does NOT move. The signal that
//! moves is Dynamic Type: `NSFont.preferredFont(forTextStyle: .body)`, whose
//! point size macOS scales with the (global or per-app) text-size category. The
//! per-app override persists in `com.apple.universalaccess` -> `FontSizeCategory`.
//! We divide the effective body point size by the 13pt baseline for the scale.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Runtime};

/// The macOS default body/system point size; the scale baseline.
const BASE_POINT_SIZE: f64 = 13.0;
/// Clamp so a mis-read or an extreme setting can't drive the UI to an unusable
/// size (rem type multiplies straight into this).
const MIN_SCALE: f64 = 0.75;
const MAX_SCALE: f64 = 2.0;

/// Broadcast to every window when the scale changes (events fan out app-wide).
const EVENT: &str = "ui://text-scale";

/// Last scale we published, so a window-focus re-query only emits on a real
/// change — otherwise every focus would re-lay-out the whole UI.
static LAST_SCALE: Mutex<Option<f64>> = Mutex::new(None);

/// Turn a raw effective point size into a clamped UI scale factor. The guard
/// keeps a zero/NaN/negative read (font unavailable) at 1.0 rather than
/// collapsing the UI.
fn scale_from_points(points: f64) -> f64 {
    if !points.is_finite() || points <= 0.0 {
        return 1.0;
    }
    (points / BASE_POINT_SIZE).clamp(MIN_SCALE, MAX_SCALE)
}

/// Effective Dynamic Type body point size (reflects the Accessibility text-size
/// category, global or per-app). Font-metric reads are thread-safe.
#[cfg(target_os = "macos")]
fn effective_body_points() -> f64 {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSFont, NSFontTextStyleBody, NSFontTextStyleOptionKey};
    use objc2_foundation::NSDictionary;

    // preferredFontForTextStyle reflects the slider; systemFontSize would not.
    // Empty options dict = the default trait collection for this process.
    let opts: Retained<NSDictionary<NSFontTextStyleOptionKey, AnyObject>> = NSDictionary::new();
    let font = unsafe { NSFont::preferredFontForTextStyle_options(NSFontTextStyleBody, &opts) };
    font.pointSize()
}

#[cfg(not(target_os = "macos"))]
fn effective_body_points() -> f64 {
    BASE_POINT_SIZE
}

/// The current clamped system text scale (1.0 == macOS default).
pub fn current_scale() -> f64 {
    scale_from_points(effective_body_points())
}

/// Frontend reads this at boot — the main window and every `new_window` pop-out
/// share `src/main.tsx`, so each queries for itself.
#[tauri::command]
pub fn get_system_text_scale() -> f64 {
    current_scale()
}

/// Record the startup scale without emitting, so the first window-focus
/// re-query doesn't spuriously broadcast a "change".
pub fn prime() {
    *LAST_SCALE.lock().unwrap() = Some(current_scale());
}

/// Re-query and, if the scale changed since we last published, broadcast it to
/// every window. Wired to window-focus: the user returning from System Settings
/// (having moved the slider) refocuses Alchemy, which is exactly the moment the
/// value can have changed.
pub fn publish_if_changed<R: Runtime>(app: &AppHandle<R>) {
    let scale = current_scale();
    {
        let mut last = LAST_SCALE.lock().unwrap();
        if *last == Some(scale) {
            return;
        }
        *last = Some(scale);
    }
    let _ = app.emit(EVENT, scale);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_unity() {
        assert_eq!(scale_from_points(BASE_POINT_SIZE), 1.0);
    }

    #[test]
    fn scales_linearly_against_baseline() {
        assert_eq!(scale_from_points(19.5), 1.5); // 19.5 / 13
        assert_eq!(scale_from_points(9.75), 0.75); // 9.75 / 13
    }

    #[test]
    fn clamps_to_sane_bounds() {
        // 13 * 2 = 26 lands exactly on the max; anything larger is pinned.
        assert_eq!(scale_from_points(26.0), MAX_SCALE);
        assert_eq!(scale_from_points(100.0), MAX_SCALE);
        // 13 * 0.75 = 9.75 is the floor; anything smaller is pinned.
        assert_eq!(scale_from_points(4.0), MIN_SCALE);
    }

    #[test]
    fn degenerate_reads_stay_at_unity() {
        assert_eq!(scale_from_points(0.0), 1.0);
        assert_eq!(scale_from_points(-5.0), 1.0);
        assert_eq!(scale_from_points(f64::NAN), 1.0);
        assert_eq!(scale_from_points(f64::INFINITY), 1.0);
    }
}
