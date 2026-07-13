# RFC: macOS integrations — capture from anywhere, findable everywhere

Alchemy is a destination app: you open it, then work. These four
integrations make it ambient — content flows in from wherever you are, and
what's inside surfaces through the OS. Ranked by value-to-effort; each layer
builds on the previous.

## 1. `alchemy://` URL scheme (foundation)

`tauri-plugin-deep-link`, scheme `alchemy`. Routes:

| URL | Action |
|---|---|
| `alchemy://notebook/<id>` | Focus main window, open the notebook |
| `alchemy://note/<id>` | Open the note's notebook, open the note |
| `alchemy://add?url=<u>[&notebook=<id>]` | Add a URL source |
| `alchemy://add?text=<t>[&title=<t>][&notebook=<id>]` | Add a pasted-text source |

Add-routes without `notebook=` land in the current notebook, else the most
recently updated one, with a toast naming the destination. Parsing and
routing live in the frontend store (`handleDeepLink`) — the backend just
forwards URLs, so the same router serves Services, the tray, and Spotlight.

macOS only routes registered schemes to a **bundled** app, so dev testing
uses a debug bundle (`pnpm tauri build --debug --bundles app`).

## 2. Menu bar extra + global hotkey

Tauri's built-in tray + `tauri-plugin-global-shortcut` (no native code):

- Tray menu: **Ask Alchemy** (summon + ⌘K palette), **Add clipboard as
  source**, recent notebooks, Open Alchemy.
- Global hotkey **⌥Space** summons the window and opens the ask surface
  (palette everywhere; the homepage ask box focuses when visible).
- "Add clipboard": text that parses as a URL becomes a URL source, anything
  else a pasted-text source — same destination rule as deep-link adds.

## 3. Services menu — "Add to Alchemy"

`NSServices` entry in Info.plist (send types: string, URL, filenames) + a
small objc2 services-provider registered at setup. The handler reads the
pasteboard and forwards through the deep-link router (files go to
`addSourceFiles`). Payloads arriving before the webview is ready are
buffered in AppState and drained by the frontend on init. Like the URL
scheme, registration requires the bundle (plus `pbs -update` in dev).

## 4. Spotlight (CoreSpotlight)

Index every notebook and note title (+ 200-char description) as
`CSSearchableItem`s (`alchemy.notebook:<id>` / `alchemy.note:<id>`),
reindexed on launch and kept fresh on note/notebook mutations. Activation
arrives via `application:continueUserActivity:` — a method added to the app
delegate at runtime (objc2 `class_addMethod`), which converts the hit to an
`alchemy://` URL and reuses the router.

## Explicitly skipped

- **Quick Look extension** — needs an Xcode app-extension target; niche
  payoff (previewing .okf exports).
- **App Intents / native Shortcuts actions** — same extension-target cost;
  the URL scheme + MCP server already give automation two doors.
- **Dock menu extras** — the app menu and tray cover it.
