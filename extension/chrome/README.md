# Alchemy Web Clipper (Chrome)

Sends pages, links, and text selections to Alchemy as sources through the
`alchemy://add` deep link. No host permissions, no stored state, no network
access — the extension is a button that composes a URL.

## What it does

- **Toolbar button** — adds the current page to a notebook.
- **Right-click a page** — "Add page to Alchemy".
- **Right-click a link** — "Add link to Alchemy" (adds the link target).
- **Right-click a selection** — "Add selection to Alchemy" (becomes a text
  source, with the page URL appended as provenance).

Each action navigates the current tab to `alchemy://add?…`; Chrome shows its
"Open Alchemy.app?" confirmation (check "Always allow" once to stop being
asked). If no notebook is named, Alchemy asks which notebook to use.

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
   - Privacy tab: declare **no data collected** (true — the extension has no
     host permissions and makes no requests); justify `contextMenus`
     ("adds right-click clipping actions") and `activeTab` ("reads the
     current tab's URL and title when the user clicks the button").
5. Submit for review. First reviews typically take a few days; minimal
   permissions like these usually pass without questions.
6. Updates: bump `version` in `manifest.json`, re-zip, upload on the same
   dashboard item.

## Safari / Firefox later

The manifest is WebExtensions-standard; Firefox needs only a
`browser_specific_settings.gecko.id` block, and Safari can wrap it with
`xcrun safari-web-extension-converter`. Both are follow-ups, not forks.
