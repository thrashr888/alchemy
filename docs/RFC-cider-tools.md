# RFC: Mac items as sources (via cider)

> **Pivot (2026-07-12):** the first draft proposed cider as *chat tools* with
> a per-action permission model. Paul's counter — Mac items as *sources* that
> auto-sync — is better, and this RFC now describes that design. Tools may
> come later; nothing here precludes them.

## Summary

Add Calendar, Reminders, and Apple Notes as **living sources**: pick a
Reminders list, a calendar range, or a Notes folder in the add-source modal,
and it becomes a source that re-syncs on the existing resync sweep — embedded,
citable, and grounded like any file. "What did I commit to this week?"
retrieves your own calendar items with citations instead of a tool call
dumping JSON into one turn.

## Why sources beat tools here

- **Reuses the sync machinery wholesale.** A `cider://` origin rides the same
  `url` field, refresh command, and minute resync sweep as folders and loose
  files. Change detection hashes fetched content into the existing `mtime`
  column (i64 = first 8 bytes of the content hash) — no schema change.
- **Grounding.** Embedded Mac content is retrieved with citations, weighted
  against the rest of the corpus — strictly better than context-stuffing.
- **Consent is structural.** Adding "Reminders: Shopping" as a source *is*
  the consent; it's visible in the sources list and deletable like anything
  else. No per-action confirmation UX needed because v1 has no writes.
- **The trade to state plainly:** unlike tools, source content is embedded
  and persisted in the local LanceDB index. Local-first makes this
  acceptable; the add-UI says it in one sentence. Mail and Contacts stay out
  of v1 for exactly this reason.

## Design

### Integration: subprocess, capability-detected

cider is binary-only today (`cider-cli`, no lib target). v1 shells out to
`cider … ` (JSON stdout), resolved from PATH/Homebrew. The UI shows Mac tiles
only when the binary exists; otherwise a quiet "brew install cider" hint.
Ship-quality follow-up: bundle cider as a Tauri sidecar (or split a
`cider-core` crate — we own the repo). TCC prompts attribute to Alchemy as
the responsible process.

### Source shape

- `source_type: "mac"`, origin in `url` as a cider URI:
  - `cider://reminders/list/Shopping`
  - `cider://calendar/upcoming/30` (rolling window, days)
  - `cider://notes/folder/Research`
- Fetch → render to readable markdown (reminders as checklists with due
  dates; events chronologically with date headings; notes as sections) →
  normal chunk/embed/store path.
- Refresh: `refresh_source_url` grows a `cider://` branch; the resync sweep
  treats mac sources like loose files — re-fetch, compare content hash
  (stored in `mtime`), re-embed on change. Rolling calendar windows change
  content daily by nature; the hash catches it.

### Commands

- `mac_available() -> bool`
- `list_mac_collections(provider) -> Vec<{id, label, detail}>` — reminders
  lists, calendars/ranges, notes folders (for the modal's picker step)
- `add_source_mac(notebook_id, provider, collection, label) -> Source`

### UI: the add-source modal (NotebookLM-style)

The + flow becomes one modal: a drop zone, then tiles — Upload files, Add
folder, From URL, Paste text, Calendar, Reminders, Apple Notes. Mac tiles
open a picker step (choose list/range/folder) with a one-line privacy note.
The rail, Cmd+K, and pending-flag flows open the same modal at the right
step. This is the UI investment the Mac providers justify; files/URL/text
get a nicer flow for free.

## Phasing

1. Add-source modal (pure frontend; immediately improves existing adds).
2. `mac.rs` backend: capability detection, three read-only providers,
   cider:// refresh + sweep integration.
3. Later: sidecar bundling, Mail/Contacts (privacy story first), writes/tools
   if sources prove insufficient.

## Open questions

- Calendar ranges: fixed presets (7/30/90 days) or a custom picker? Presets
  first.
- Should mac sources be excluded from "convert note to source"-style flows
  that assume file-ish content? Probably moot; verify.
- Sidecar signing: does bundling a Homebrew-built binary survive
  notarization, or do we build cider from source in release.sh?
