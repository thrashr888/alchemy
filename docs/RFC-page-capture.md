# RFC: page capture — rendered, complete, and local

`extract_url` is reqwest + readability (`dom_smoothie` → `htmd`). That works
for server-rendered pages and fails on JS-rendered ones — `looks_blocked`
(ingest.rs) already diagnoses the miss ("may require login, block bots, or
render with JavaScript") but can only warn, not fix. The fix is already in
the bundle: Alchemy ships WebKit. Render the page in a hidden WKWebView,
hand the settled DOM to the same `readable_text` path saved pages use, and
the JS-rendered web becomes capturable — with zero new services, keys, or
binaries, and no URL ever leaving the machine.

## 1. Why a webview beats an automation stack

Bot detectors look for automation: the `navigator.webdriver` flag, headless
Chromium's missing surfaces, Playwright's driver fingerprint, CDP leaks.
playwright-stealth exists to *fake away* those tells (webdriver masking,
chrome-runtime shims, plugin spoofing) and its own README calls it a
proof-of-concept that beats only the simplest checks. rustwright's insight
is architectural — drop the Playwright driver, speak raw CDP, disable the
`Runtime` domain leak — but it's early-alpha, Chromium-only (~200 MB
download), and consumable from Python/Node, not as a Rust crate.

A WKWebView needs none of that because it isn't automation — it's a real
browser. No webdriver flag to mask, genuine WebKit JS/DOM/canvas
fingerprint, Apple's TLS stack indistinguishable from Safari's. The
strongest stealth is authenticity. What we do borrow from those projects:

- **UA normalization** (their idea, our one-liner): WKWebView's default UA
  lacks the `Version/x.x Safari/605.1.15` suffix real Safari sends — some
  detectors key on exactly that. Set the full Safari UA on the capture
  window. It's barely a lie: we *are* that engine. (The reqwest fast path
  keeps its Chrome UA; the two paths need not match.)
- **Consistency**: `Accept-Language` and `navigator.languages` should agree,
  as should platform and UA — mismatches are the actual tells.

## 2. The pipeline

```
add/preview/re-sync (commands.rs) ──► extract_url
    1. reqwest fast path (unchanged, incl. Google export endpoints)
    2. looks_blocked? ──no──► done
    3.      └─yes──► webview render pass ──► readable_text ──► done
    4.                  └─still blocked──► "assisted capture" state (§5)
```

- **Trigger.** `looks_blocked` already fires on <200 chars and on marker
  phrases ("enable javascript", "just a moment", "verify you are human",
  "sign in to continue"…). Today it annotates the source; it becomes the
  escalation gate. Escalation is automatic and default-on — the setting to
  disable it is cost control, not a safety cap.
  *Phase-1 amendment:* marker checks alone missed a whole class of
  ready-but-hollow captures (Apple's HIG pages fast-fetch to ~260 chars of
  chrome with no marker phrase), so any "success" under 500 chars also
  escalates, and a rescue is accepted only when it passes `looks_blocked`
  **and** strictly beats the fast path's length (`not_better` telemetry
  outcome otherwise). A failed rescue can never make a source worse.
- **Window.** A non-activating, offscreen-positioned utility window with a
  webview that has **zero Tauri capabilities** — remote origins get no IPC.
  New module `src-tauri/src/capture.rs`; all `extract_url` call sites (add,
  preview, cider re-sync) inherit it for free.
- **Profile.** A dedicated persistent `WKWebsiteDataStore` — the "capture
  profile." Isolated from the app's own webview and from Safari; cookies
  accepted or logins performed during assisted capture (§5) persist so the
  next capture of that site starts warm. One setting clears it.
- **Readback.** `with_webview` → objc2 → `evaluateJavaScript` with a
  completion handler feeding a oneshot channel. Returns a JSON payload;
  objc2 is already how Alchemy does Services and Spotlight.

## 3. Getting *all* the content — the settle detector

"Page loaded" is not "content arrived." An init script (installed before
any page script runs) plus a staged wait:

1. **Network quiet, no CDP:** the init script patches `fetch` and
   `XMLHttpRequest` to count in-flight requests. Quiet = zero in flight
   for 500 ms after the `load` event.
2. **DOM quiet:** a `MutationObserver` marks settled after 500 ms without
   mutations. Settled = load + network quiet + DOM quiet, hard-capped at
   ~10 s so a stray websocket or analytics beacon can't hold capture open.
3. **Completeness pass:** step-scroll to the bottom in viewport increments
   (capped at ~5 viewports so infinite feeds terminate) to fire
   IntersectionObserver lazy-loaders; unlazify images (`data-src` →
   `src`); drop full-viewport `position:fixed` overlays (consent scrims);
   brief re-settle.
4. **Payload:** `outerHTML` after settle, `document.title`, canonical URL,
   JSON-LD + OpenGraph metadata (byline, published date — feed the source
   card), and `innerText` as a last-resort fallback when readability finds
   no article node.

**The real risk — occlusion throttling.** WebKit throttles timers and rAF
in windows it considers invisible, which can stall the very lazy-loads the
scroll pass tries to trigger. Mitigation ladder: non-activating window
positioned offscreen (macOS may still report it occluded) → tiny
non-activating corner window for the seconds of capture → make it a
visible-by-choice "capturing…" affordance. Phase 1's job is to measure
which rung is needed; the settle telemetry (settle time, escalation rate,
rescue rate) decides.

*Phase-1 finding:* throttling never materialized — hidden windows hydrate
JS-rendered pages fine (settles cluster around ~2 s). The trap was a
**startup race** instead: the first settle poll can land before the page
context commits, read a transient "complete and empty" shell, and settle
in one poll with 0 chars. Two dwell floors fix it — `MIN_DWELL` (800 ms)
always, and `DEGRADED_DWELL` (3.5 s) when the probe reports the init
script missing, so a readyState-only signal can't declare victory on the
pre-hydration shell. The probe also reports `sawState`/`readyState` into
telemetry so a CSP-blocked init script is distinguishable from the race.

## 4. Capture memory

A per-domain record of which tier last succeeded: `fast | rendered |
assisted`. Future adds and cider re-syncs for that domain start at the
winning tier instead of re-failing the fast path first. Re-sync of a source
captured via webview reuses the webview. Stored alongside sources; nothing
is inferred from browsing history — only from captures the user asked for.

## 5. Assisted capture — the user is the bypass

We do not solve captchas, forge logins, or circumvent paywalls. When the
rendered pass still trips `looks_blocked`, the source enters a "needs a
hand" state with one action: **Open capture window.** The same capture
webview appears as a normal window; the user logs in, dismisses the
banner, or passes the human-check themselves — on their own machine, with
their own access — then hits **Capture now**, which runs the identical
settle-and-extract path. The persistent capture profile means most sites
need this once. This is the read-it-later pattern (Pocket, reader mode):
the user captures what they can already see.

## 6. Alternatives considered

| Option | Verdict |
|---|---|
| **rustwright** (Skyvern) | Alpha, Chromium-only, ~200 MB fetch, no Rust-crate surface yet. Right architecture, wrong maturity. Revisit if WebKit hit-rate disappoints. |
| **chromiumoxide + user's installed Chrome** | No bundled download, but inherits headless-Chrome detection and the stealth arms race. Possible future per-domain `engine: chromium` escape hatch. |
| **playwright-stealth** | Wrong runtime (Python); kept as a fingerprint checklist — nearly every item is moot on genuine WebKit. |
| **Jina Reader / Firecrawl** | Hosted: every URL you read leaves the machine, plus keys and metering. Rejected on local-first grounds, per the original note. |
| **Chrome extension** | Complementary, separately tracked: captures DOM from the user's real logged-in browser and posts through `alchemy://add`. This RFC's pipeline is the ingest seam it needs. |

## 7. Phases

1. **Render pass** — `capture.rs`, settle detector, UA normalization,
   escalation behind `looks_blocked`, telemetry (escalation rate, rescue
   rate, settle p50/p95). Answers the throttling question empirically.
   *Shipped.* Field results: cerebras.ai 48 → 22,694 chars; randomlabs.ai
   → 38,530; all eight Apple HIG pages ~260 → 4k–55k; an autotrader
   "Access Denied" bot wall passed; x.com single tweets arrive on the fast
   path and profiles rescue to bio + top-of-timeline. Cloudflare-style
   challenges (classic.com) and true 404s correctly keep their fast
   result.
2. **Completeness + memory** — scroll/unlazify pass, metadata payload,
   per-domain capture memory, re-sync tier reuse.
   *Shipped.* Capture memory works end-to-end (`capture_domains.json`;
   re-syncs of a rendered-winning domain go webview-first, 30-day TTL);
   byline/date land as a `> By … · Published …` provenance line when the
   page exposes them; the scroll pass moves the dominant inner scroll
   container as well as the window, so `overflow:auto` feeds fire their
   lazy-loaders (a bare `window.scrollTo` never budges them).

   *Correction — the two "thin page" gaps were misdiagnosed.* Phase 2 read
   dupont and classic.com as listing grids readability under-extracted.
   Direct DOM inspection disproved that: dupont's Ferrari-328 search has
   **no results** (a 1,054-char body of chrome — nothing to extract), and
   classic.com renders a 1,344-char Cloudflare-limited shell with no market
   data, no `<main>`, no tables. x.com's timeline is login-walled after a
   few tweets. All three are **wall / empty-result** cases — phase 3
   territory — not extraction failures. A speculative link-list extractor
   was built and cut: it turned footer nav into fake "content" on
   classic.com and, on a genuine listing (npm search), locked onto the
   keyword-filter sidebar over the real results. A robust general listing
   extractor is its own project and needs a validated beneficial case
   before it ships; none of these pages is one. Net phase-2 change kept:
   inner-container scrolling, which is a safe strict improvement.
3. **Assisted capture** *(not planned)* — the visible login/challenge
   window is where the three misdiagnosed gaps above actually get solved,
   but it's explicitly out of scope for now.
4. **Extension ingest seam** — the clip receiver (§8): `clip.rs` localhost
   endpoint, extension-origin auth, URL-keyed held clips consumed by
   `ingest_url` ahead of the generic capture fallback, extension scrapes the
   live authenticated tab. This is the out-of-app answer to the same private
   / login-walled pages assisted capture targets in-app.

## 8. Addendum — the extension ingest seam (clip capture)

The hidden capture webview (§2) is powerful but blind in one specific way:
it is a **cookieless session**. Intranet pages, anything behind a login, a
paywall the user subscribes to, a private Notion/Google Doc — the render
pass reaches the same wall the fast path does, because it is not *the user's*
browser. Assisted capture (§5) is the in-app answer; the Chrome extension is
the out-of-app one, and it is strictly better when the user already has the
page open: it captures the DOM from the **real, logged-in tab**, so it sees
exactly what the user sees. RFC §6 already reserved the extension as
"complementary… posts through `alchemy://add`. This RFC's pipeline is the
ingest seam it needs." This section builds that seam.

### 8.1 Transport — why not the deep link

The clipper already fires `alchemy://add?url=…`. It cannot also *carry the
page*: a rendered DOM is tens of KB to megabytes, and after percent-encoding
it blows past every practical custom-scheme URL limit (Chrome's navigation
cap, macOS `openURL`, the deep-link plugin). Even extension-side–cleaned
plain text usually won't fit. The payload must travel out of band.

The app already owns the right pattern: the MCP server (`mcp.rs`) binds a
localhost HTTP port, writes a discovery file, and toggles from config. The
clip receiver is the same shape — a separate module (`clip.rs`) because its
**auth is the inverse of MCP's**. MCP *rejects* any request carrying an
`Origin` header (real MCP clients never send one; a browser always does). A
browser extension's `fetch` always sends `Origin: chrome-extension://<id>`
(or `moz-extension://…`), and a malicious web page **cannot forge that** —
the browser sets it. So the clip endpoint's rule is: allow when `Origin` is
absent (native/CLI) or an extension scheme; reject everything else with 403.
That single check closes the malicious-web-page and DNS-rebind holes (their
`Origin` stays their real site), while `host_permissions` for
`http://127.0.0.1/*` let the extension's cross-origin POST bypass CORS
preflight entirely.

### 8.2 Flow — held clip, unchanged deep link

```
extension click ─► scripting.executeScript (EXTRACT_JS, in the live tab)
    │                  → {url, title, ogTitle, byline, published, html}
    ├─► POST 127.0.0.1:<clip_port>/clip   (held server-side, keyed by URL)
    └─► alchemy://add?url=…&title=…       (existing deep link, unchanged)
                          │
        add modal ► user picks notebook ► add_source_url(url)
                          │
        ingest_url: git/notion detectors first (identity-reshaping);
                    then clip::take(url)? ──yes──► extracted_from_html ─► store
                                            └─no──► extract_url_rescued (as today)
```

The correlation key is the **normalized URL** — no clip id threads through
the frontend, so the deep link and the entire add flow are byte-for-byte
unchanged. The extension POSTs, then fires the deep link; by the time the
user picks a notebook (seconds later) the clip is waiting. `clip::take`
consumes it (10-min TTL, bounded entry count and payload size sweep out
anything abandoned). The clipped HTML runs through the *same*
`extracted_from_html` the webview capture uses — identical readability,
identical provenance line — so a clip and a rescue produce the same shape.

### 8.3 Precedence

A present clip **supersedes the generic page-capture fallback**, because an
authenticated render is authoritative and re-fetching a private page is
doomed. But identity-reshaping detectors still run first: a clipped GitHub
URL ingests as a repo (`ingest_git`), a Notion URL via the API — the clip
only stands in for the generic `extract_url_rescued` rung. If the clipped
DOM extracts to empty/thin text, `ingest_url` falls through to the normal
fetch path, so a broken clip never makes a source worse than URL-only would.

### 8.4 What this changes about the source

Clipped private pages are **snapshot sources**: re-sync is a no-op for them
(the app still can't reach the page), exactly like the `assisted` capture
tier. That's a deliberate, documented limit, not a regression — the content
exists only because the user's browser could see it once.

### 8.5 Cost, honestly stated

The clipper's privacy pitch changes: the manifest gains `scripting` (DOM
read on the active tab, still only on user gesture via `activeTab`) and
`host_permissions` for `http://127.0.0.1/*` (the localhost POST). It no
longer holds *zero* host permissions — the store listing copy must say so.
The receiver is default-on but a config toggle (`clip_enabled`), same as MCP,
for anyone who wants no localhost surface at all.

## Explicitly skipped

- **Captcha solving or paywall circumvention** — assisted capture keeps a
  human in exactly the loop the wall demands.
- **Crawling** — capture is one page per user intent; no link-following,
  no parallel sweeps, no scheduled scraping beyond existing re-sync.
- **Googlebot UA cloaking** — pretending to be a crawler to shake loose
  SSR invites cloaked/wrong content; authenticity is the strategy.
- **Cross-origin iframe stitching** — `evaluateJavaScript(in: frame)` can
  target frames natively if an embed-heavy site ever matters; niche until
  proven otherwise. Same-origin iframes are walked in the extract script.
- **Windows/Linux parity** — WebView2 `ExecuteScript` and WebKitGTK
  equivalents exist, but macOS-first, like the rest of the app.
