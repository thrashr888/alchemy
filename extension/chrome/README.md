# Alchemy Web Clipper (Chrome)

Sends pages, links, and text selections to Alchemy as sources through the
`alchemy://add` deep link. Adding a whole page also scrapes the rendered DOM
from your logged-in tab and hands it to Alchemy's local receiver on
`127.0.0.1`, so private / login-walled pages the app could never fetch itself
still capture. No stored state, no remote network access — the only host it
talks to is your own machine.

## What it does

- **Toolbar button** — captures the current page's rendered DOM and adds it
  to a notebook (falls back to a URL-only clip if the app's receiver is off).
- **Right-click a page** — "Add page to Alchemy" (same DOM capture).
- **Right-click a link** — "Add link to Alchemy" (adds the link target; the
  app fetches it, since a bare link isn't the open page).
- **Right-click a selection** — "Add selection to Alchemy" (becomes a text
  source, with the page URL appended as provenance).

Each action navigates the current tab to `alchemy://add?…`; Chrome shows its
"Open Alchemy.app?" confirmation (check "Always allow" once to stop being
asked). If no notebook is named, Alchemy asks which notebook to use. The
page's DOM (when captured) travels out of band to the local receiver — the
deep link only carries the URL and title, which the app pairs with the
scrape. See `docs/RFC-page-capture.md` §8.

## Try it locally (no store account needed)

1. Chrome → `chrome://extensions` → toggle **Developer mode** (top right).
2. **Load unpacked** → pick this `extension/chrome/` folder.
3. Pin the flask icon from the puzzle-piece menu.

Changes to these files take effect after clicking the reload arrow on the
extension card.

## Publish to the Chrome Web Store (first time)

1. Register the developer account (one-time $5 fee):
   https://chromewebstore.google.com/register — use any Google account.
2. Zip the folder contents (the files, not the parent folder):
   `cd extension/chrome && zip -r alchemy-clipper.zip . -x '*.DS_Store'`
3. Developer dashboard → **New item** → upload the zip.
4. Listing requirements before submitting:
   - Store icon: `icons/icon128.png` works as-is.
   - At least one 1280×800 screenshot (screenshot Chrome with the context
     menu open on a page).
   - Category: Productivity. Language: English.
   - Privacy tab: declare **no data collected** (true — the extension sends
     the page only to the user's own machine and stores nothing); justify
     `contextMenus` ("adds right-click clipping actions"), `activeTab` +
     `scripting` ("reads the current tab's content when the user clicks, to
     capture the page"), and the `http://127.0.0.1/*` host permission
     ("hands the captured page to the local Alchemy app").
5. Submit for review. First reviews typically take a few days; minimal
   permissions like these usually pass without questions.
6. Updates: bump `version` in `manifest.json`, re-zip, upload on the same
   dashboard item.

## Firefox

The same folder loads in Firefox as-is: the manifest carries
`browser_specific_settings.gecko.id` (`clipper@alchemy.thrasher.dev`,
min 121.0) and `background.scripts` alongside `service_worker`, so each
browser picks its supported key. Firefox assigns an unpredictable
`moz-extension://` origin, so Alchemy deliberately rejects its rendered-DOM
POST and the extension falls back to URL-only clipping; this avoids trusting
every installed Firefox extension. Test via `about:debugging` → This
Firefox → Load Temporary Add-on → pick `manifest.json`. Publish the
identical zip at https://addons.mozilla.org/developers/ (free account,
no fee; copy lives in STORE.md).

## Safari

Safari wraps WebExtensions in an app via Xcode:

```sh
xcrun safari-web-extension-converter extension/chrome \
  --project-location extension/safari --app-name "Alchemy Web Clipper"
```

Open the generated project, run it once, then enable the extension in
Safari → Settings → Extensions (allow unsigned extensions under the
Develop menu during testing). Distribution requires an Apple Developer
membership — the same one that signs Alchemy releases.

## Store assets

`store/` holds the listing art, rendered from the homepage's design
grammar: two 1280×800 screenshots, the 440×280 small tile, and the
1400×560 marquee. Listing copy and privacy-form answers are in
`STORE.md`.
