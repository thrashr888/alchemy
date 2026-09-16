# RFC: Alchemy for iPhone — a reader that thinks

Status: draft, awaiting review (2026-09-15).
Origin: Reminders item "iPhone app: uses native FM, costs $9, sync via
iCloud, does content diffs on import, hide most advanced functions to
simplify the UX." Builds on [RFC-okf-live.md](RFC-okf-live.md) §5.7 (the
iCloud container) and [RFC-inference-providers.md](RFC-inference-providers.md)
§4 (Foundation Models).

## Summary

A second app, not a port. The Mac app is a workshop: it imports, extracts,
embeds, runs models from four providers, hosts agents, and renders thirty-
seven generators. The phone is where the notebook gets *read*, where a
link found on the bus gets *kept*, and where a question gets *asked* while
the workshop is closed. Everything the phone needs already exists as
files in the iCloud container the Mac writes; the phone reads those files,
adds its own, and lets the Mac do the heavy work when it next opens.

Three verbs on the phone: **read**, **keep**, **ask**. Everything else
stays on the Mac.

## What the phone is for

| On the phone | Not on the phone |
| --- | --- |
| Read any source or note, full text, with find-in-page | Studio generators (all 37), reports, schedules |
| Keep a link, a photo, a pasted paragraph (share sheet) | Folder, git, Notion, Obsidian, Mac sources |
| Ask a question of one notebook, cited | Deep research, meta-chat across notebooks |
| Search titles and full text across notebooks | Grow, hygiene, the registry, feeds management |
| See what changed since the phone last opened | Settings beyond model choice and the one toggle |
| Switch notebooks | Agents, MCP, the CLI |

"Hide most advanced functions" is the whole product decision: the phone
has no Settings screen worth the name. One toggle (answer with the
on-device model / answer with the Mac's saved provider when it is
reachable) and the notebook list.

## Architecture: the bundle is the API

The Mac app already keeps every notebook on disk as an OKF bundle — an
`index.md`, `sources/<slug>.md`, `notes/<slug>.md`, originals in
`references/`, provenance in frontmatter — and, since stage two, in the
app's iCloud container. The phone app shares the container (same team,
same `iCloud.com.thrashr888.alchemy` identifier) and reads the bundles
directly. No server, no sync protocol of our own, no schema: the phone
sees exactly what the Mac wrote, and the Mac's reconciler already treats
another writer in the same folder as another machine (RFC-okf-live §5.6).

What the phone writes, it writes as the Mac would: a new
`sources/<slug>.md` with the frontmatter families the importer expects
(`title`, `url`, `kind`, `added`, `origin: alchemy-ios/<version>`, the
`human:<account>` by-line), and a `references/` copy of a photo or PDF.
The Mac, on its next launch or FSEvents tick, takes the file in through
the same read-back path as a file the user dropped in Finder: extracts,
chunks, embeds, gists. The phone never embeds anything.

### Reading

- **Notebook list** from the container's bundles, `index.md` frontmatter
  for title, icon, color, counts. Sorted by the bundle's last write.
- **Sources and notes** rendered from Markdown. Originals in `references/`
  open in QuickLook (PDF, images, Office files). Frontmatter is provenance,
  not prose — stripped, shown as a properties sheet, the same rule as the
  reader.
- **Find**: full-text over the bundle's Markdown, in-process, no index.
  `NSRegularExpression` over a few megabytes is instant; the phone does
  not need BM25 or vectors to find a phrase.
- **What changed**: the bundle's `log/` (per-writer entries, RFC-okf-live
  §5.6) since the phone's last open, as a short list at the top of the
  notebook. Read-only — the Mac's Arrivals strip was removed for being a
  queue to clear; this is a glance, not a queue.

### Keeping

- **Share sheet extension** (`ShareExtension` target): a URL, a photo, a
  PDF, or selected text from any app. Writes the source file into the
  bundle of the last-used notebook, with a notebook picker one tap away.
  Titles come from the page's `<title>` when reachable, else the URL host;
  the Mac replaces both with the real extraction later.
- **Content diffs on import.** The reminder asks for this specifically.
  Two cases: (1) a URL already in the notebook — the phone writes a
  `sources/<slug>.md` whose frontmatter carries `refresh_of: <sync id>`
  and the Mac's read-back treats it as a refresh request, so the Mac
  re-fetches and its existing diff machinery (source events, RFC-events)
  produces the change; (2) a pasted text that overlaps an existing
  source — the phone shows the overlap (shared lines, in-process diff)
  before saving, and offers "append to <source>" or "keep as new".
  Nothing on the phone rewrites an existing file; the Mac owns
  reconciliation.

### Asking

- **On-device, always available:** Foundation Models on iOS 26+ (the same
  API the `alchemy-fm` sidecar wraps on the Mac). The phone assembles a
  grounded prompt the way `rag.rs` does — numbered excerpts, cite by
  number — but retrieval is *lexical*: the question's terms scored BM25-
  style over the bundle's Markdown in memory, top passages by paragraph.
  The on-device model's context is small, so the prompt caps at ~8
  excerpts of ~400 characters. Good enough for "what did the inspection
  say about the roof" over a notebook of a hundred sources; not a
  replacement for the Mac's hybrid retrieval, and the answer says so
  with a one-line "answered on-device" footer.
- **Through the Mac when it is reachable:** the Mac's MCP server already
  answers `search` and `chat` over loopback; over the local network it
  would answer the phone too. Not in v1 — it means exposing the server
  beyond loopback and a pairing step. Listed under "later".
- **Chat history** lives on the phone only (RFC-shared-notebook already
  keeps chat local to the machine that asked). A turn worth keeping is
  saved as a note in the bundle, by the phone, with the citation list —
  which the Mac indexes like any note.

## The $9

One-time purchase, no subscription: the phone spends nothing of ours.
There is no server, no metered model, no sync service — the container is
the user's iCloud, the model is the user's phone. The price buys the app
and pays for the App Store presence; it does not gate any feature. That
keeps the product invariant: access to a notebook never depends on a
model, a provider, or a plan — a bundle in the container is readable by
Files.app before the phone app is even installed.

Alchemy Pro ([RFC-alchemy-pro.md](RFC-alchemy-pro.md)) stays a separate
question about server-side costs; nothing here needs it.

## Design

The Mac app's language, one column: hairline borders, no tonal fills,
color only when it means something (the notebook's color on its icon,
citations in the accent). System type, system materials, the notebook
list as a plain list, the reader as a document. No custom navigation
chrome; `NavigationStack`, `.searchable`, the share sheet, QuickLook, and
the standard context menus (long-press on a source: Open original, Copy
link, Delete — the last one a proposal file in the bundle, the Mac
completes it, exactly as RFC-shared-notebook §3 treats deletes between
people).

Themes: the Mac's 31 themes are the Mac's. The phone follows the system
appearance and takes the notebook's color for its accent. That is the
"hide most advanced functions" rule applied to appearance.

## What the Mac app needs to change

Small, and all of it is honest about what already exists:

1. **Read-back accepts a phone-written source.** The importer already
   takes any `sources/*.md` with frontmatter; add `origin: alchemy-ios/*`
   to the by-line vocabulary and accept `refresh_of` as a refresh
   request. One frontmatter family, one branch in read-back.
2. **Notes written by the phone** carry `origin: alchemy-ios/*` and a
   `citations:` family (source slugs + quotes) so the reader can show them
   as citations rather than plain text.
3. **`log/` entries are already per-writer.** Nothing to change; the phone
   is another writer.
4. **The container identifier is shared.** The iOS target uses the same
   iCloud container entitlement; the Mac app's entitlement is unchanged.

Nothing in Lance, nothing in retrieval, no new IPC.

## Phasing

1. **Reader.** Notebook list, source and note reader, originals via
   QuickLook, find, what-changed. TestFlight to Paul's phone. The bundle
   format is the contract; if the reader is right, everything else is
   additive.
2. **Keep.** Share extension + in-app paste; the Mac's read-back changes
   (item 1 above). The URL-refresh diff comes with it.
3. **Ask.** Foundation Models grounded answer over lexical retrieval, save
   a turn as a note. The "answered on-device" footer. Ship: App Store,
   $9.
4. **Later.** Ask through the Mac over the local network (pairing,
   Bonjour, the MCP server bound to the LAN with a token); Handoff of the
   open source between Mac and phone; a widget for the Brief.

## Open questions for Paul

- **One target or two?** A single Xcode project with the Mac's Swift
  sidecar code shared as a package, or a separate repo. The sidecar is
  ~200 lines; a separate `alchemy-ios` repo with its own beads is cleaner
  for release cadence, and the bundle format is the only coupling.
- **iPad.** Free with the same code; a two-column layout is the only
  work. Ship it in phase 1 or hold to keep the review surface small?
- **Photos as sources.** OCR on the phone (Vision framework, on-device)
  before writing the source, or leave text extraction to the Mac's
  vision model? On-device OCR makes a photographed page searchable on the
  phone immediately; it also means two extractors can disagree. Proposed:
  phone writes the photo to `references/` and a `text:` frontmatter field
  from Vision as a hint; the Mac's extraction replaces it.

## Tests

The reader's parser gets the Mac's golden bundle (RFC-okf-live §3 tests)
as a fixture: every frontmatter family present, every source and note
renders, find hits the known phrases. Round-trip: a source written by the
phone's writer imports on the Mac through `cargo test` read-back fixtures
with the by-line and origin intact. The FM prompt assembly has the same
excerpt-numbering tests `rag.rs` has, ported.
