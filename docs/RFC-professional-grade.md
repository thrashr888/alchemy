# RFC: Professional grade — trust, fidelity, and depth

Status: proposed. Six pillars that move Alchemy from "polished indie app" to
the tier where people bet their work on it — the Adobe standard, minus
Adobe's chrome. Each pillar is independently shippable; the priority order
at the end is the recommendation, not a schedule.

## Summary

The app already clears the polish bar: a token-driven design system across
23 themes, keyboard-reach and right-click audits, undo toasts, crash capture
with a restart screen, an auto-updater, 378 Rust tests, and a retrieval eval
harness. What professional-grade products add is a different axis — **trust**.
A professional user's questions are: will my library open tomorrow, will it
still be fast at 10,000 sources, does clicking a citation land on the exact
passage, can I always press ⌘Z, and does the app behave like the OS grew it.

Six pillars, in the order a failure costs the most confidence:

1. **Data trust** — snapshots, restore, and migration safety on the one
   LanceDB store everything lives in.
2. **Performance at scale** — explicit budgets asserted in CI against a
   seeded 10k-source fixture library.
3. **Core-loop fidelity** — the import → cite → reader loop tested as a
   contract against a hostile-input corpus.
4. **Every state designed and verified** — contrast, VoiceOver, and
   degraded states checked by machines, not eyeballs.
5. **A real undo model** — a session history stack behind the existing
   undo toasts; ⌘Z works everywhere.
6. **macOS citizenship depth** — drag-out, file associations, and
   Shortcuts on top of the shipped Services/Spotlight/URL-scheme layer.

Pillars 2 and 3 share one investment: a deterministic fixture corpus.
Build it once, both pillars stand on it.

## What exists today

- **One store, no net.** `db.rs` opens a single embedded LanceDB;
  `ensure_table` + atomic `add_columns` handle schema drift forward, but
  nothing handles it backward — a column append bricks every older binary
  that opens the store (shared dev/prod store policy exists *because* of
  this). There is no snapshot, no restore, no integrity check on open.
- **Partial escape hatch.** `export_notebook_okf` (commands.rs:9633) writes
  sources and notes as markdown bundles; chat turns, ledger, registry, and
  reports stay behind. Import exists (`ImportOkfModal`).
- **Nightly ground to stand on.** The Night Shift scheduler
  ([RFC-night-shift.md](RFC-night-shift.md)) is a resident Rust tick loop
  that runs while no window exists — the natural home for a nightly save,
  already being explored in the Night Shift expansion.
- **Perf instincts, no budgets.** The Lance scan-storm fix (coalesced
  corpus sweeps), debounced FTS rebuilds, and async imports each fixed a
  measured regression — but nothing *asserts* performance, so the next
  regression ships silently. `trace.rs` already writes rotated JSONL
  (`retrieval.jsonl`, `capture.jsonl`) and is trivially extensible.
- **Fidelity untested at the edges.** `ingest.rs` handles each filetype;
  PDFium renders pages. No test feeds the pipeline a scanned PDF, a
  malformed DOCX, RTL text, or a 2,000-page book and asserts the citation
  still lands.

  > Correction after implementation: this section claimed **"citations carry
  > offsets". They do not.** `Citation` (models.rs:339) is `chunk_id`,
  > `ordinal`, `snippet` — no span — and the chunks table (db.rs:3573)
  > stores none either. The reader re-derives the location at click time by
  > fuzzy match: `locatePassage` (ReaderPane.tsx:86-107) joins the first and
  > last twelve words with `\s+` and falls back to `snippet.length * 1.1`.
  > The load-bearing invariant is therefore not "offsets are right" but
  > **"the snippet is a byte-verbatim span of the source"** — which is what
  > Pillar 3 pins instead, in bytes, chars, and UTF-16 code units.
- **Frontend verification gap.** 378 Rust tests, **zero** frontend tests.
  37 of 50 components carry ARIA; 23 themes × dark/light is a 46-cell
  contrast matrix no human re-checks per change.
- **Undo is toast-scoped.** `store.ts` snapshots deletes and restores on
  toast click — the right semantics, but the memory dies with the toast.
  The Edit menu's Undo (menu.rs:108) is the native text-field item only.
- **macOS layer 1 shipped.** `alchemy://`, menu-bar extra, Services menu
  ("Add to Alchemy", services.rs), Spotlight (spotlight.rs), print export,
  window-geometry restore.

## Pillar 1 — Data trust

**Goal: no sequence of crashes, upgrades, downgrades, or bad nights loses a
library, and the user can see that this is true.**

### Nightly snapshot (Night Shift job)

A `backup.rs` job registered with the Night Shift scheduler:

- **Mechanism:** APFS clonefile copy (`clonefile(2)` per file, fall back to
  plain copy) of the LanceDB directory into
  `<app-data>/backups/store/<YYYY-MM-DD>/`. Clonefile makes a multi-GB store
  snapshot near-instant and near-free until blocks diverge.
- **Retention:** 7 nightly + 4 weekly, pruned after each success. A
  Settings → Library row shows the last snapshot time and total size.
- **Quiesce:** take the snapshot between scheduler ticks with the write
  gate held (same single-flight idiom as the gist sweep) so no Lance
  commit is mid-flight.
- **Escape hatch alongside:** the same job loops `export_notebook_okf`
  over all notebooks into `<app-data>/backups/okf/latest/` — the
  human-readable copy that survives even a Lance format problem. Extending
  OKF to cover chat turns and ledger entries is a follow-up, not a blocker.

### Migration safety

- **Store version stamp:** a `store_version` file in the store directory,
  written by `Db::open` after `ensure_table`/column appends complete. An
  older binary that finds a newer stamp gets a designed "This library was
  upgraded by Alchemy vX — update to open it" screen (via the existing
  fatal → restart-screen path in diagnostics), not a Lance panic.
- **Pre-migration snapshot:** when `open()` detects it is about to append
  columns to an existing table (the `field_with_name` miss branch), it
  clonefile-snapshots the store into `backups/pre-migrate/<version>/`
  first. Upgrades become rehearsable and reversible.
- **Downgrade-path tests:** check in minimal fixture stores written by the
  last two releases (a store with one notebook/source/note is small);
  `cargo test` opens each and asserts every table reads. This is the test
  that would have caught both historical brickings.

### Integrity on open

`Db::open` gains a cheap validation pass: each expected table opens, row
counts are sane (readable, not asserted against anything), and the chunks
table's embedding dimensionality matches config. On failure: record
`fatal`, and the restart screen offers **Restore from last snapshot** —
which renames the bad store aside (never deletes) and clones the snapshot
back.

## Pillar 2 — Performance at scale

**Goal: named budgets, measured every release, asserted in CI — the next
scan storm fails a test instead of a user.**

### The budgets

| Metric | Budget | Where measured |
|---|---|---|
| Cold start → window interactive | < 1.5 s | startup trace |
| Hybrid search, 10k-chunk store | < 300 ms p95 | perf test |
| Chat first-token overhead (retrieval + prompt build, excl. model) | < 500 ms | retrieval trace |
| Import throughput, 100-page PDF | < 10 s excl. embedding | perf test |
| Idle CPU, app open, no activity | ~0% over 60 s | manual + activity_stats |
| Memory, 10k-source library open | < 800 MB | perf test |

Numbers are proposals to be calibrated against a first measurement pass,
then frozen; loosening one thereafter is a deliberate commit, not drift.

### Mechanics

- **Seeded fixture library:** a deterministic generator (reusing the eval
  seeding path — which must `flush_fts` itself, the known gotcha) builds a
  10k-source store with the built-in embedder, CI-safe, cached between
  runs. This is the shared corpus pillar 3 also uses.
- **`perf_budgets` test module:** `cargo test --lib perf_budgets` opens the
  fixture store and asserts the table above. Runs in CI on release
  branches; locally on demand.
- **Startup trace:** `trace.rs` gains `startup.jsonl` — stamps at process
  start, db open, tables ensured, scheduler up, first window ready.
  Release-over-release regression is a one-line jq away.
- **Frontend at scale:** the sources list, chat history, and gallery are
  the three lists that grow unboundedly — virtualize whichever of them the
  10k fixture shows janking (measure first; don't virtualize on faith).

## Pillar 3 — Core-loop fidelity

**Goal: import → chunk → cite → click → land-on-the-exact-passage is a
tested contract, including for hostile files.**

### The hostile corpus

`src-tauri/fixtures/hostile/` (checked in, each file small or generated):

| File | Exercises |
|---|---|
| scanned-only PDF (no text layer) | OCR path honesty — designed "no text" status, never silent empty |
| PDF with rotated + multi-column pages | offset → page mapping |
| generated 2,000-page PDF | chunking + reader perf, citation depth |
| DOCX with broken relationship XML | ingest error path, no panic |
| RTL (Hebrew/Arabic) markdown + PDF | chunk boundaries, reader rendering |
| CJK + emoji-dense text | char-vs-byte offset bugs |
| 0-byte file, wrong-extension file | designed error status |
| HTML with 10 MB of inline SVG | web ingest limits |

### The contract tests

- **Ingest never panics:** every corpus file through `ingest.rs` yields
  either content or a designed error status. This is a plain test loop
  today and a `cargo-fuzz` target later if it ever pays for itself.
- **Citation round-trip:** for each text-bearing corpus file: ingest, run
  a fixed query with the built-in embedder, take the top citation, and
  assert the excerpt is a verbatim span of the source — and that
  `build_chat_messages` showed the model that same span. Checked in bytes,
  chars, and UTF-16 code units (the webview counts the last). This pins the
  invariant every reader feature stands on.

  > Known violation, found by this work and left unfixed pending triage:
  > `word_windows` (ingest.rs:1585-1605) rebuilds text as
  > `words[start..end].join(" ")`, flattening interior whitespace. It is
  > reachable for a paragraph over 280 words containing no `.`/`!`/`?`,
  > since `split_oversized` (ingest.rs:1275) splits on sentence punctuation
  > and `normalize` (ingest.rs:1629) trims only line ends. No live impact —
  > `locatePassage`'s `\s+` regex absorbs it — but any feature built on
  > stored spans breaks here.
- **Reader landing:** for PDFs, assert citation → page resolution against
  known-good page numbers for the fixture queries (the frontend scroll is
  pillar 4's e2e concern; the backend mapping is testable in Rust).

Depth over breadth: this pillar deliberately hardens the one loop that *is*
the product before any new surface is added to it.

## Pillar 4 — Every state designed and verified

**Goal: the 46-cell theme matrix, the screen reader, and the degraded
states are checked by CI.**

- **Contrast matrix test:** the first frontend test. A vitest suite (new —
  `pnpm test` joins `pnpm build` as a gate) imports `themes.ts` and asserts
  WCAG AA contrast for the token pairs that carry meaning:
  `foreground`/`background`, `muted-foreground`/`surface`,
  `subtle-foreground`/`surface-2`, `citation`/`surface`,
  `destructive`/`background`, `primary`-fill/white — across all 23 themes ×
  dark/light. Any theme edit that breaks readability fails the build.
- **Store-logic tests:** the undo/restore snapshot paths in `store.ts`
  (and pillar 5's history stack) get unit tests — the frontend's most
  consequential untested logic.
- **ARIA completion:** close the remaining 13-of-50 components; add
  `aria-live` to streaming chat output and toast announcements; one full
  VoiceOver traversal of the main window documented as a checklist in
  DESIGN.md §accessibility so it's re-runnable.
- **Degraded states as designed states:** "Ollama unreachable", "no chat
  model configured", "embedding model changed (re-embed needed)" each get
  a designed presentation via `HealthBanner`/empty-state patterns — with
  the action that fixes them inline, not error prose. Inventory pass over
  every pane: empty, loading, error, degraded — a table in DESIGN.md
  records which component owns each.
- **RichEditor input integrity:** IME composition (Japanese/Chinese input)
  and dictation must never drop or duplicate characters — a manual test
  script first; automated through tauri-browser if regressions recur.

## Pillar 5 — A real undo model

**Goal: ⌘Z reverses the last destructive-or-content mutation anywhere in
the app; the toast is the surfacing, the stack is the memory.**

The undo toasts already built the hard part — snapshot-and-restore closures
for source, note, and notebook deletes. Generalize:

- **`history` in `store.ts`:** a bounded stack (50) of
  `{ label, undo(), redo() }`. Every mutation that today builds a toast
  closure *also* pushes here; mutations without toasts (tag edits, registry
  moves, ledger status flips, rename) start pushing too. Session-scoped;
  no persistence in v1.
- **Menu integration:** replace the native-only Edit → Undo
  (menu.rs:108) with app-routed items that show the label ("Undo Delete
  Source", "Redo Rename Notebook") — *except* when focus is in a text
  input/RichEditor, where native text undo must win. Focus check in the
  webview decides which handler claims the event, same self-filtering
  discipline as the multi-window event rules.
- **Scope honesty:** chat generation, imports, and connector disconnects
  are not undoable (regenerate/re-import/reconnect are their inverses);
  they never enter the stack, so ⌘Z never lies. Connector-source deletes
  keep their confirm dialog for the same reason (store.ts:73).
- **MCP parity:** agent-driven mutations push history entries too, so a
  user can undo what an agent just did — arguably the strongest trust
  feature in an agent-native app.

## Pillar 6 — macOS citizenship depth

Layer 1 ([RFC-macos-integrations.md](RFC-macos-integrations.md)) shipped:
URL scheme, menu-bar extra, Services, Spotlight. Layer 2, in value order:

- **Drag out:** notes and studio artifacts drag to Finder/Mail/Messages as
  real files. The inverse of the existing FileDrop.

  > Correction after implementation: **Tauri v2 has no drag-out API.**
  > `Window.startDragging()` (api 2.11.1) and `start_dragging` (tauri
  > 2.11.5) both move the *window*; `onDragDropEvent`/`dragDropEnabled` are
  > drag-destination only, which is what FileDrop already consumes. Real
  > drag-out needs the community `tauri-plugin-drag` crate: a Cargo.toml
  > dependency, a `.plugin(...)` line in lib.rs, and a capability
  > permission. The export half already exists and is reusable —
  > `api.exportNote` (api.ts:406) over `export_note_file` (export.rs:34),
  > with `exportTargets()` (StudioPanel.tsx:82) already choosing the
  > kind-true format. Deferred as its own change rather than half-wired.

- **File association + "Open With":** register the app for `.okf` bundles
  and as an "Open With" target for md/pdf/docx, so Finder becomes an import
  path.

  > Correction: the share sheet does **not** follow from the same
  > declaration. On macOS the Share menu is populated by share extensions,
  > not `CFBundleDocumentTypes` — a separate target entirely. The
  > no-extension paths are the shipped Services menu entry and share-sheet
  > Shortcuts.
- **Shortcuts actions:** phase 1 is URL-scheme-backed shortcuts shipped as
  a gallery (Add to Alchemy, Open Notebook, Ask Alchemy) — zero new
  runtime, works today.

  > Correction: "works today" held for Add and Open but not for Ask —
  > **`alchemy://ask` did not exist.** The tray item and ⌥Space reach the
  > palette over an `integrations://ask` event that no URL could raise, and
  > unknown `kind` values fell through silently. Adding the missing `ask`
  > arm to `handleIntegrationUrl` was part of this pillar's work. Phase 2 (real App Intents with parameters and
  return values) requires a Swift extension target; the `alchemy-fm`
  sidecar sets the precedent that this is tractable — deferred until the
  URL-scheme version proves demand.
- **Quick Look for OKF bundles:** deferred — low traffic until OKF export
  is a daily artifact (pillar 1 makes it one; revisit after).

## Shared infrastructure

One fixture investment serves three pillars: the deterministic 10k-source
generated store (pillar 2's budgets), the hostile corpus (pillar 3's
contracts), and the downgrade fixture stores (pillar 1's migration tests)
all live under `src-tauri/fixtures/` with a single seeding entry point.
Fixtures stay checked in after verification, per house rule.

## Priority order

1. **Pillar 1** — a lost library is the only unrecoverable failure.
   Nightly snapshot + store version stamp first; they're small and stop
   the two known catastrophes (silent loss, downgrade brick).
2. **Pillar 3** — fidelity failures are the product breaking its core
   promise; the corpus also unblocks pillar 2.
3. **Pillar 2** — budgets land cheaply once the fixture store exists;
   performance improves for every user at every scale.
4. **Pillar 4** — the contrast test and vitest scaffold first (highest
   automation per hour), ARIA completion and state inventory behind it.
5. **Pillar 5** — generalizes machinery that already exists.
6. **Pillar 6** — drag-out and file association first; Shortcuts phase 1
   whenever; the rest on demand.

## Non-goals

- Feature breadth, cross-platform, or Adobe's actual UI density — the
  Linear-derived restraint stays.
- Cloud telemetry. All measurement here is local (traces, CI, fixtures);
  crash-free-rate tracking reads `diagnostics.rs` output on this machine,
  nothing phones home.
- A document-versioning UI. Snapshots are library-level disaster recovery;
  per-note history is a different feature and a different RFC.
