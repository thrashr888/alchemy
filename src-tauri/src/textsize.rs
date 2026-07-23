//! Adherence to the macOS Accessibility "Text Size" control.
//!
//! WKWebView paints the whole UI in fixed CSS px and ignores the system
//! Accessibility text size, so a user who enlarges text system-wide (System
//! Settings > Accessibility > Display > Text Size, globally or per-app) sees no
//! change in Alchemy. No Tauri plugin covers this. We read the effective size
//! natively and republish it to the webview as a scale factor the CSS folds
//! into its root font-size (`--system-text-scale`); the rem-based type then
//! scales with the OS.
//!
//! ## Which native signal actually tracks the slider
//!
//! An earlier version read `NSFont.preferredFont(forTextStyle: .body).pointSize`.
//! On macOS that font does *not* reflect the Accessibility text size, so it was
//! inert. Apple DTS confirms this: Dynamic Type is a mobile-only feature, and on
//! the Mac the `preferredFont` fonts "do not take the user's Accessibility
//! setting into account." Verified live on this Mac (macOS 26/Tahoe): moving the
//! slider changed nothing, and Alchemy never appeared in the per-app list.
//!
//! The signal that *does* move is the private `com.apple.universalaccess`
//! preference domain, which the "Text Size" slider writes. Read live here with
//! `defaults read com.apple.universalaccess`:
//!
//! ```text
//! FontSizeCategory = {
//!     global = XXXS;                 // the global slider position (short form)
//!     "com.apple.mail" = UseGlobal;  // a per-app override, or "UseGlobal"
//!     version = "3.0";
//! };
//! ```
//!
//! So the effective category is the per-app override for our bundle id (unless
//! it is `UseGlobal`), else `FontSizeCategory["global"]`. Older notes referenced
//! `UIPreferredContentSizeCategoryName`; that key lives in `NSGlobalDomain`
//! (`defaults read -g`), NOT here, and is a coarse UICT-derived value that floors
//! at `XS` — so it can't see the `XXXS`/`XXS` stops. We use `FontSizeCategory`
//! (the fine slider signal) and keep the legacy key only as a last-resort
//! fallback for macOS versions without `FontSizeCategory`. We map the category
//! onto a Dynamic Type scale ladder (see `LADDER`) and divide by the anchor
//! (`L`, the macOS "Default" slider position) to get the scale.
//!
//! ## Opting into the per-app list
//!
//! A WKWebView/Tauri shell cannot appear in System Settings' per-app Text Size
//! list. That list is a private scan (`com.apple.universalaccess`'s
//! `dynamicTypeScanCache`) of apps that link Dynamic Type; Apple provides no
//! Info.plist key or entitlement for a third-party Mac app to opt in (Apple DTS,
//! forum thread 818858). List membership is *not* required to follow the global
//! setting, which is the core requirement — we read the domain directly.
//!
//! ## Change detection
//!
//! There is no public notification for Accessibility text-size changes (Apple
//! DTS: "no NSNotification for Accessibility Text Size changes"). We re-query on
//! window focus (wired in `lib.rs`) — the moment the user returns from System
//! Settings, which is exactly when the value can have changed. A guessed
//! Darwin/distributed-notification name would be dead code, so we deliberately
//! do not ship one.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Runtime};

#[cfg(target_os = "macos")]
use objc2::runtime::AnyObject;
#[cfg(target_os = "macos")]
use objc2_foundation::{NSDictionary, NSString, NSUserDefaults};

/// Our bundle id — the key a per-app text-size override lives under in
/// `com.apple.universalaccess` -> `FontSizeCategory`.
const BUNDLE_ID: &str = "com.thrashr888.alchemy";

/// The preference domain the Text Size slider writes.
#[cfg(target_os = "macos")]
const UA_DOMAIN: &str = "com.apple.universalaccess";
/// Dict of per-app + global text-size categories inside that domain.
#[cfg(target_os = "macos")]
const FONT_SIZE_CATEGORY_KEY: &str = "FontSizeCategory";
/// Key inside `FontSizeCategory` holding the global slider position.
#[cfg(target_os = "macos")]
const GLOBAL_KEY: &str = "global";
/// Sentinel a per-app entry uses to defer to the global position.
#[cfg(target_os = "macos")]
const USE_GLOBAL: &str = "UseGlobal";
/// Legacy fallback key. Lives in `NSGlobalDomain` (`defaults read -g`), NOT in
/// `com.apple.universalaccess`. It is the coarse UICT-derived value that floors
/// at `XS`, so it cannot see the finer `XXXS`/`XXS` slider stops — used only
/// when `FontSizeCategory` is entirely absent (other macOS versions).
#[cfg(target_os = "macos")]
const LEGACY_GLOBAL_KEY: &str = "UIPreferredContentSizeCategoryName";

/// The category we treat as the user's normal / unscaled setting: `scale == 1.0`
/// here, so the app renders pixel-identical to its fixed-px design. Scale is
/// `ladder[current] / ladder[ANCHOR_CATEGORY]`, so re-anchoring is a one-line
/// change.
///
/// ANCHOR: `L` — the macOS "Default" slider position. At that stop the OS emits
/// the literal `FontSizeCategory.global = "DEFAULT"` (its legacy UICT value is
/// `…CategoryL`), which `canon` folds to `L`. Anchoring here renders the app at
/// its design size when the system is at Default — the neutral point users
/// expect — shrinking below Default and growing above it. (An earlier anchor at
/// the smallest stop `XXXS` made the app match its design only at the tiny end
/// and read oversized at Default, which is what testing surfaced.)
const ANCHOR_CATEGORY: &str = "L";

/// Clamp so a mis-read or an extreme setting can't drive the UI to an unusable
/// size (rem type multiplies straight into this).
const MIN_SCALE: f64 = 0.75;
const MAX_SCALE: f64 = 2.0;

/// Broadcast to every window when the scale changes (events fan out app-wide).
const EVENT: &str = "ui://text-scale";

/// Last scale we published, so a window-focus re-query only emits on a real
/// change — otherwise every focus would re-lay-out the whole UI.
static LAST_SCALE: Mutex<Option<f64>> = Mutex::new(None);

/// Dynamic Type "body" reference point sizes per content-size category, in the
/// macOS `FontSizeCategory` short-form spelling (smallest -> largest). Only the
/// *ratios* matter: `scale = LADDER[current] / LADDER[anchor]`, so the absolute
/// values just set how far apart the rungs sit. The 12..23 standard range and
/// 28..53 accessibility range follow the iOS `.body` Dynamic Type ladder,
/// extended down to `XXXS`/`XXS` which macOS exposes but iOS does not.
const LADDER: &[(&str, f64)] = &[
    ("XXXS", 12.0),
    ("XXS", 13.0),
    ("XS", 14.0),
    ("S", 15.0),
    ("M", 16.0),
    ("L", 17.0), // Dynamic Type default
    ("XL", 19.0),
    ("XXL", 21.0),
    ("XXXL", 23.0),
    ("AXM", 28.0),    // AccessibilityMedium
    ("AXL", 33.0),    // AccessibilityLarge
    ("AXXL", 40.0),   // AccessibilityExtraLarge
    ("AXXXL", 47.0),  // AccessibilityExtraExtraLarge
    ("AXXXXL", 53.0), // AccessibilityExtraExtraExtraLarge
];

/// Normalize any spelling of a content-size category to its canonical `LADDER`
/// token, or `None` if unrecognized. Accepts the macOS short forms, the long
/// `UICTContentSizeCategory*` forms, the `Accessibility*` long names, and the
/// `AX1..AX5` shorthand — the exact accessibility spelling macOS uses is not
/// verifiable without entering that range, so we accept the plausible set.
fn canon(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("UICTContentSizeCategory")
        .unwrap_or(trimmed);
    let up = stripped.to_ascii_uppercase();
    let c = match up.as_str() {
        "XXXS" => "XXXS",
        "XXS" => "XXS",
        "XS" => "XS",
        "S" => "S",
        "M" => "M",
        "L" | "DEFAULT" => "L",
        "XL" => "XL",
        "XXL" => "XXL",
        "XXXL" => "XXXL",
        "ACCESSIBILITYMEDIUM" | "AXM" | "AX1" => "AXM",
        "ACCESSIBILITYLARGE" | "AXL" | "AX2" => "AXL",
        "ACCESSIBILITYEXTRALARGE" | "AXXL" | "AX3" => "AXXL",
        "ACCESSIBILITYEXTRAEXTRALARGE" | "AXXXL" | "AX4" => "AXXXL",
        "ACCESSIBILITYEXTRAEXTRAEXTRALARGE" | "AXXXXL" | "AX5" => "AXXXXL",
        _ => return None,
    };
    Some(c)
}

/// Reference point size for a category (any spelling), or `None` if unknown.
fn category_points(raw: &str) -> Option<f64> {
    let c = canon(raw)?;
    LADDER.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

/// Reference points for the anchor category. `ANCHOR_CATEGORY` is a constant we
/// control; fall back to the Dynamic Type default (L = 17) if it is ever set to
/// something the ladder doesn't contain, so we never divide by zero.
fn anchor_points() -> f64 {
    category_points(ANCHOR_CATEGORY).unwrap_or(17.0)
}

/// Clamp a raw scale to sane bounds; a zero/NaN/negative value (bad read) stays
/// at 1.0 rather than collapsing the UI.
fn clamp_scale(scale: f64) -> f64 {
    if !scale.is_finite() || scale <= 0.0 {
        return 1.0;
    }
    scale.clamp(MIN_SCALE, MAX_SCALE)
}

/// Map a content-size category (any spelling) to a clamped UI scale factor.
/// Unknown/absent categories render at 1.0 (no scaling).
fn scale_for_category(raw: &str) -> f64 {
    match category_points(raw) {
        Some(points) => clamp_scale(points / anchor_points()),
        None => 1.0,
    }
}

/// The raw text-size signals read from `com.apple.universalaccess`. Every field
/// is surfaced verbatim by `dump_text_size_signals` for live verification.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Signals {
    /// `FontSizeCategory[<our bundle id>]` — a category, `UseGlobal`, or absent.
    per_app: Option<String>,
    /// `FontSizeCategory["global"]` — the global slider position.
    global: Option<String>,
    /// Legacy top-level `UIPreferredContentSizeCategoryName`.
    legacy: Option<String>,
}

/// Read the per-app override + global position (from `com.apple.universalaccess`)
/// and the legacy coarse value (from `NSGlobalDomain`). Fresh handles pick up
/// cross-process changes (NSUserDefaults auto-updates to the latest values), so
/// a focus re-query sees the slider's new position.
#[cfg(target_os = "macos")]
fn read_signals() -> Signals {
    use objc2::AnyThread;

    let mut out = Signals::default();

    // FontSizeCategory (per-app override + global slider) — the fine signal.
    let suite = NSString::from_str(UA_DOMAIN);
    if let Some(ua) = NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&suite)) {
        if let Some(fsc) = ua.dictionaryForKey(&NSString::from_str(FONT_SIZE_CATEGORY_KEY)) {
            out.per_app = string_entry(&fsc, BUNDLE_ID);
            out.global = string_entry(&fsc, GLOBAL_KEY);
        }
    }

    // Legacy coarse value lives in NSGlobalDomain; the standard defaults search
    // list covers it (we never set this key ourselves, so it resolves there).
    let std_defaults = NSUserDefaults::standardUserDefaults();
    out.legacy = std_defaults
        .stringForKey(&NSString::from_str(LEGACY_GLOBAL_KEY))
        .map(|s| s.to_string());

    out
}

/// Fetch a string value from an NSDictionary, or `None` if absent/not a string.
#[cfg(target_os = "macos")]
fn string_entry(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<String> {
    dict.objectForKey(&NSString::from_str(key))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|s| s.to_string())
}

/// Pick the effective category and record which signal it came from. Per-app
/// override wins (unless it is `UseGlobal` or an unrecognized value), then the
/// global slider, then the legacy key.
#[cfg(target_os = "macos")]
fn choose(signals: &Signals) -> (Option<String>, &'static str) {
    if let Some(per_app) = &signals.per_app {
        if !per_app.eq_ignore_ascii_case(USE_GLOBAL) && canon(per_app).is_some() {
            return (Some(per_app.clone()), "per-app");
        }
    }
    if let Some(global) = &signals.global {
        return (Some(global.clone()), "global");
    }
    if let Some(legacy) = &signals.legacy {
        return (Some(legacy.clone()), "legacy");
    }
    (None, "none")
}

/// The effective Accessibility text-size category (global or per-app), or `None`
/// when the domain is unreadable / unset.
#[cfg(target_os = "macos")]
fn effective_category() -> Option<String> {
    choose(&read_signals()).0
}

#[cfg(not(target_os = "macos"))]
fn effective_category() -> Option<String> {
    None
}

/// The current clamped system text scale (1.0 == the anchor / normal setting).
pub fn current_scale() -> f64 {
    match effective_category() {
        Some(category) => scale_for_category(&category),
        None => 1.0,
    }
}

/// Frontend reads this at boot — the main window and every `new_window` pop-out
/// share `src/main.tsx`, so each queries for itself.
#[tauri::command]
pub fn get_system_text_scale() -> f64 {
    current_scale()
}

/// Diagnostic dump of every raw text-size signal plus the chosen category and
/// computed scale. Debug/macOS builds only (the coordinator invokes it live to
/// verify the read path); release builds return a stub so it is not a surface.
#[tauri::command]
pub fn dump_text_size_signals() -> String {
    #[cfg(all(target_os = "macos", any(debug_assertions, feature = "debug")))]
    {
        dump_json()
    }
    #[cfg(not(all(target_os = "macos", any(debug_assertions, feature = "debug"))))]
    {
        String::from("{\"disabled\":\"text-size signal dump is debug/macOS only\"}")
    }
}

/// Build the diagnostic JSON. Compiled when a caller exists (the debug command,
/// or the startup print under the `debug` feature) to avoid dead code in
/// release.
#[cfg(all(target_os = "macos", any(debug_assertions, feature = "debug")))]
fn dump_json() -> String {
    let signals = read_signals();
    let (chosen, source) = choose(&signals);
    let scale = chosen.as_deref().map(scale_for_category).unwrap_or(1.0);
    serde_json::json!({
        "domain": UA_DOMAIN,
        "bundleId": BUNDLE_ID,
        "legacyGlobalKey": LEGACY_GLOBAL_KEY,
        "legacyGlobalValue": signals.legacy,
        "fontSizeCategoryGlobal": signals.global,
        "fontSizeCategoryPerApp": signals.per_app,
        "chosenCategory": chosen,
        "chosenSource": source,
        "anchorCategory": ANCHOR_CATEGORY,
        "anchorPoints": anchor_points(),
        "computedScale": scale,
    })
    .to_string()
}

/// Record the startup scale without emitting, so the first window-focus
/// re-query doesn't spuriously broadcast a "change".
pub fn prime() {
    *LAST_SCALE.lock().unwrap() = Some(current_scale());
    #[cfg(all(target_os = "macos", feature = "debug"))]
    eprintln!("[textsize] startup signals: {}", dump_json());
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
    fn canon_recognizes_every_ladder_token() {
        for (name, _) in LADDER {
            assert_eq!(canon(name), Some(*name), "short form {name}");
        }
    }

    #[test]
    fn canon_accepts_long_and_alias_spellings() {
        assert_eq!(canon("UICTContentSizeCategoryL"), Some("L"));
        assert_eq!(canon("UICTContentSizeCategoryXXXL"), Some("XXXL"));
        assert_eq!(canon("AccessibilityMedium"), Some("AXM"));
        assert_eq!(canon("AccessibilityExtraExtraExtraLarge"), Some("AXXXXL"));
        assert_eq!(canon("AX3"), Some("AXXL"));
        assert_eq!(canon(" l "), Some("L")); // trimmed + case-insensitive
        assert_eq!(canon("UseGlobal"), None);
        assert_eq!(canon("nonsense"), None);
        assert_eq!(canon(""), None);
    }

    #[test]
    fn category_points_maps_each_ladder_entry() {
        for (name, points) in LADDER {
            assert_eq!(category_points(name), Some(*points), "entry {name}");
        }
        assert_eq!(category_points("unknown"), None);
    }

    #[test]
    fn scale_is_ratio_to_anchor_for_every_entry() {
        let anchor = anchor_points();
        for (name, points) in LADDER {
            let expected = (points / anchor).clamp(MIN_SCALE, MAX_SCALE);
            let got = scale_for_category(name);
            assert!(
                (got - expected).abs() < 1e-12,
                "entry {name}: {got} != {expected}"
            );
        }
    }

    #[test]
    fn anchor_category_is_unity() {
        assert_eq!(scale_for_category(ANCHOR_CATEGORY), 1.0);
    }

    #[test]
    fn macos_default_maps_to_the_anchor() {
        // The macOS "Default" slider stop emits the literal "DEFAULT"; it must
        // fold to the L anchor and render at 1.0 (the app's design size), with
        // smaller stops below 1.0 and larger stops above.
        assert_eq!(canon("DEFAULT"), Some("L"));
        assert_eq!(scale_for_category("DEFAULT"), 1.0);
        assert!(scale_for_category("XXXS") < 1.0, "below Default shrinks");
        assert!(scale_for_category("XL") > 1.0, "above Default grows");
    }

    #[test]
    fn scale_is_monotonic_up_the_ladder() {
        for pair in LADDER.windows(2) {
            let lo = scale_for_category(pair[0].0);
            let hi = scale_for_category(pair[1].0);
            assert!(hi >= lo, "{} ({hi}) < {} ({lo})", pair[1].0, pair[0].0);
        }
    }

    #[test]
    fn largest_category_clamps_to_max() {
        // 53 / anchor is >= 2.0 for any sane anchor (anchor <= 26), so the top
        // of the ladder always pins to the ceiling.
        assert_eq!(scale_for_category("AXXXXL"), MAX_SCALE);
    }

    #[test]
    fn unknown_category_renders_at_unity() {
        assert_eq!(scale_for_category("UseGlobal"), 1.0);
        assert_eq!(scale_for_category("garbage"), 1.0);
    }

    #[test]
    fn clamp_holds_the_bounds() {
        assert_eq!(clamp_scale(1.0), 1.0);
        assert_eq!(clamp_scale(1.5), 1.5);
        assert_eq!(clamp_scale(3.0), MAX_SCALE); // above ceiling
        assert_eq!(clamp_scale(0.5), MIN_SCALE); // below floor
    }

    #[test]
    fn clamp_keeps_degenerate_reads_at_unity() {
        assert_eq!(clamp_scale(0.0), 1.0);
        assert_eq!(clamp_scale(-5.0), 1.0);
        assert_eq!(clamp_scale(f64::NAN), 1.0);
        assert_eq!(clamp_scale(f64::INFINITY), 1.0);
    }
}
