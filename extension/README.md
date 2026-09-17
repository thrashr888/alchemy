# Alchemy Web Clipper

Sends pages, links, and text selections to Alchemy as sources through the
`alchemy://add` deep link. In Chrome, adding a whole page also scrapes the
rendered DOM from your logged-in tab and hands it to Alchemy's local receiver
on `127.0.0.1`, so private and login-walled pages the app could never fetch
itself still capture. No stored state, no remote network access — the only
host it talks to is your own machine.

## Layout

```
extension/
  src/              background.js and the icons — the whole extension
  chrome/           manifest.json for Chrome
  firefox/          manifest.json for Firefox
  store/            listing art (not shipped inside the package)
  dist/             build output, gitignored
```

One script assembles both:

```sh
scripts/build-extensions.sh            # both
scripts/build-extensions.sh firefox    # one
```

It writes `extension/dist/<browser>/` (load this unpacked) and
`extension/dist/<browser>.zip` (upload this to the store). `background.js`
lives in `src/` only; nothing copies it by hand.

## What it does

- **Toolbar button** — adds the current page to a notebook. Chrome captures
  the rendered DOM; Firefox sends the URL for the app to fetch.
- **Right-click a page** — "Add page to Alchemy" (same as the button).
- **Right-click a link** — "Add link to Alchemy" (adds the link target; the
  app fetches it, since a bare link isn't the open page).
- **Right-click a selection** — "Add selection to Alchemy" (becomes a text
  source, with the page URL kept as provenance).

Each action navigates the current tab to `alchemy://add?…`, and the browser
shows its "Open Alchemy.app?" confirmation — tick the remember box once to
stop being asked. If no notebook is named, Alchemy asks which one to use.
A captured DOM travels out of band to the local receiver; the deep link
carries only the URL and title, which the app pairs with the scrape. See
`docs/RFC-page-capture.md` §8.

## Chrome

### Load it locally

1. `scripts/build-extensions.sh chrome`
2. Chrome → `chrome://extensions` → toggle **Developer mode** (top right).
3. **Load unpacked** → pick `extension/dist/chrome/`.
4. Pin the flask icon from the puzzle-piece menu.

Re-run the script after editing `src/background.js`, then click the reload
arrow on the extension card.

### Publish to the Chrome Web Store

1. Register the developer account (one-time $5 fee):
   https://chromewebstore.google.com/register — use any Google account.
2. `scripts/build-extensions.sh chrome`
3. Developer dashboard → **New item** → upload `extension/dist/chrome.zip`.
4. Listing requirements before submitting:
   - Store icon: `src/icons/icon128.png` works as-is.
   - At least one 1280×800 screenshot; `store/` holds two.
   - Category: Productivity. Language: English.
   - Privacy tab: declare **no data collected** (true — the extension sends
     the page only to the user's own machine and stores nothing); justify
     `contextMenus`, `activeTab`, `scripting`, and the `http://127.0.0.1/*`
     host permission. Wording is in `STORE.md`.
5. Submit for review. First reviews typically take a few days; minimal
   permissions like these usually pass without questions.
6. Updates: bump `version` in `chrome/manifest.json`, rebuild, upload on the
   same dashboard item.

## Firefox

Firefox clips URLs, links, and selections. It does not hand over the rendered
DOM, and the difference is deliberate: the receiver admits exactly one fixed
`chrome-extension://` origin, because an extension scheme by itself is not an
identity — any installed extension can send one. Firefox mints a per-install
`moz-extension://` UUID that Alchemy cannot know in advance, so admitting it
would mean admitting every Firefox extension on the machine. The build
therefore asks for neither `scripting` nor a host permission, and installing
it prompts for nothing at all. See `src-tauri/src/clip.rs` and
`docs/RFC-page-capture.md` §8.1.

The other manifest differences: `background.scripts` instead of
`service_worker` (Firefox does not run MV3 service workers), and
`browser_specific_settings.gecko` carrying the add-on id
`alchemy-clipper@thrashr.dev`, a `strict_min_version` of 121.0, and
`data_collection_permissions: none`, which AMO is making mandatory.

### Load it temporarily

1. `scripts/build-extensions.sh firefox`
2. Firefox → `about:debugging#/runtime/this-firefox`
3. **Load Temporary Add-on…** → pick `extension/dist/firefox/manifest.json`.
4. Pin the flask icon from the toolbar overflow menu if you want it visible.

A temporary add-on lasts until you quit Firefox. After editing
`src/background.js`, rebuild and click **Reload** on the add-on's card.

To check the package the way AMO will:

```sh
npx web-ext lint --source-dir extension/dist/firefox
```

### Publish to AMO

Firefox refuses unsigned add-ons in release builds, so signing is required
even if you never list it publicly. Both routes start the same way.

1. Create a free Firefox Add-ons developer account (no fee):
   https://addons.mozilla.org/developers/
2. `scripts/build-extensions.sh firefox`
3. **Submit a New Add-on** → upload `extension/dist/firefox.zip`.
4. Choose the distribution:
   - **On this site** — listed on addons.mozilla.org, installable by anyone,
     and the only route that produces the public listing URL the app's
     Settings button wants.
   - **On your own** — Mozilla signs the build and hands back a signed
     `.xpi` to distribute yourself. No listing, no listing metadata.
5. Listing fields for the "On this site" route (copy in `STORE.md`):
   - Name, summary, and full description.
   - Category: Productivity (Firefox → Extensions).
   - Homepage, support site, and privacy policy URLs.
   - At least one screenshot; `store/` holds two at 1280×800.
   - License: MPL-2.0, matching the app.
   - Data collection: **none**.
   - A note to reviewers explaining that `alchemy://` is the macOS app's
     deep link and that the add-on contacts no remote host. Reviewers read
     source, and the whole add-on is one readable file.
6. Automated validation runs on upload. Human review follows; a first
   submission of this size is usually cleared within a few days.
7. Updates: bump `version` in `firefox/manifest.json`, rebuild, upload a new
   version under the same add-on.

Once the listing is live, put its URL in `FIREFOX_CLIPPER_URL` in
`src/components/SettingsDialog.tsx` — until then the Settings button for
Firefox stays disabled rather than pointing at a page that does not exist.

## Safari

Safari wraps WebExtensions in an app via Xcode:

```sh
scripts/build-extensions.sh chrome
xcrun safari-web-extension-converter extension/dist/chrome \
  --project-location extension/safari --app-name "Alchemy Web Clipper"
```

Open the generated project, run it once, then enable the extension in
Safari → Settings → Extensions (allow unsigned extensions under the
Develop menu during testing). Distribution requires an Apple Developer
membership — the same one that signs Alchemy releases. Like Firefox, Safari
gets a per-install extension origin, so it clips URLs rather than DOM.

## Store assets

`store/` holds the listing art, rendered from the homepage's design grammar:
two 1280×800 screenshots, the 440×280 small tile, and the 1400×560 marquee.
Both stores take the same images. Listing copy and privacy-form answers are
in `STORE.md`.
