# Shortcuts gallery

Alchemy registers the `alchemy://` URL scheme, and every inbound intent —
deep link, Services menu, menu bar extra, Spotlight hit — funnels through
one router. Shortcuts.app can open a URL, so the scheme is already an
automation surface: no extension target, no new runtime, nothing to
install beyond the app itself.

Each recipe below is a Shortcut you build once in Shortcuts.app. The
actions are stock; the only Alchemy-specific part is the URL.

## The routes

| URL | What happens |
| --- | --- |
| `alchemy://notebook/<id>` | Focus the main window, open that notebook |
| `alchemy://note/<id>` | Open the note's notebook, then the note |
| `alchemy://add?url=<encoded>` | Add a URL source |
| `alchemy://add?text=<encoded>&title=<encoded>` | Add a pasted-text source |
| `alchemy://add?file=<encoded path>` | Add a local file as a source |

Every route summons the window first — that is the point of all of them.

Values are percent-encoded. `add` accepts repeated `file=` parameters for
a multi-file capture. Any route may carry `&notebook=<id>` to name the
destination; without it, Alchemy asks which notebook to use and defaults
to the most recently updated one. `title` is optional on a text add.

Notebook and note ids are opaque strings. A note's link is on its
right-click menu in the reader ("Copy link"); a notebook id comes from the
`list_notebooks` MCP tool, or from any note link's notebook.

## Add Link to Alchemy

Send the page you are reading to a notebook. Pairs with the share sheet.

1. New Shortcut. Turn on **Show in Share Sheet**, accept **URLs**.
2. **Text** → `alchemy://add?url=`, then insert the Shortcut Input with
   **URL Encode** applied.
3. **Open URLs**.

Leave off `&notebook=` and Alchemy asks where it lands, which is usually
what you want from a share sheet. Pin a destination by appending
`&notebook=<id>` to the Text action.

## Add Text to Alchemy

Capture a selection or a clipboard scrap as a pasted-text source.

1. **Get Clipboard** (or accept **Text** from the share sheet).
2. **URL Encode** the text.
3. **Text** → `alchemy://add?text=<encoded>&title=Clipboard`.
4. **Open URLs**.

The title is what the source is called in the sources list, so a Shortcut
that captures from one place should name it: `title=Safari`, `title=Mail`.

## Add File to Alchemy

Route a file — a downloaded PDF, a rendered report, an export from another
app — into a notebook without leaving the Finder.

1. New Shortcut, **Show in Share Sheet**, accept **Files**.
2. **Get Details of Files** → **File Path**.
3. **URL Encode** the path.
4. **Text** → `alchemy://add?file=<encoded path>`.
5. **Open URLs**.

The path must be absolute. For several files, repeat the parameter:
`alchemy://add?file=<a>&file=<b>`.

## Open Notebook

A one-tap jump to the notebook you live in, worth a Dock or menu bar slot.

1. **Text** → `alchemy://notebook/<id>`.
2. **Open URLs**.

Same shape for a note: `alchemy://note/<id>`.

## Ask Alchemy

Not yet available as a URL. The ask surface has a global hotkey (⌥Space)
and a menu bar item, but no route — `alchemy://ask` is unhandled and the
router drops it silently. See "Still to wire" below.

## Open With

Alchemy declares itself an opener for Markdown, text, PDF, Word, EPUB, and
CSV files, and the owner of the Open Knowledge Format bundle it exports
(`.okf`, `.okf.zip`). The declaration lives in `bundle.fileAssociations`
in `src-tauri/tauri.conf.json`; Tauri turns it into the
`CFBundleDocumentTypes` and `UTExportedTypeDeclarations` entries of the
built bundle. That is what fills Finder's **Open With → Alchemy**.

The macOS share sheet reads share extensions, not document types, so the
declaration does not put Alchemy in the Share menu on its own. The
Services menu entry ("Add to Alchemy") and a share-sheet Shortcut from the
recipes above already cover that job without an extension target.

Registration takes effect for a bundled build only, same constraint as the
URL scheme: `pnpm tauri build --debug --bundles app` for dev testing.

`.okf.zip` is a compound extension. LaunchServices matches the longest
one, so the exported UTI wins over `public.zip-archive` — but a bundle
renamed to plain `.zip` opens in Archive Utility, as it should.

## Still to wire

Two gaps, both small, both in files this pass did not own.

**File opens are declared but not handled.** macOS delivers a document
open as `application:openURLs:` with a `file://` URL, which tao turns into
`RunEvent::Opened` and the deep-link plugin forwards to the handler at
`src-tauri/src/integrations.rs:252`. That handler passes the URL through
verbatim, and the frontend router rejects anything that is not the
`alchemy:` scheme (`src/lib/store.ts:898`), so an Open With currently
focuses the window and does nothing else. The fix belongs in the
`on_open_url` closure: translate a `file://` URL into
`alchemy://add?file=<encoded path>` before routing. The add route already
handles multiple files and OKF probing on the frontend side.

**`alchemy://ask` has no route.** The tray and the ⌥Space hotkey both call
`summon_ask` (`src-tauri/src/integrations.rs:76`), which emits
`integrations://ask`. Adding an `ask` arm to `handleIntegrationUrl`
alongside `notebook`, `note`, and `add` would give the gallery its third
recipe — optionally with `?q=` to pre-fill the palette.
