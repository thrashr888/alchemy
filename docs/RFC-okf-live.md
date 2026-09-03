# RFC: Live OKF — a notebook on disk, and bundles as sources

OKF is a snapshot today. Export writes a bundle, import reads one, and
the moment either side keeps working the two drift apart. The Reminders
item asks for the other thing: **read/write OKF as a notebook, or as a
source.** Two shapes, one format:

- **A bound notebook.** A notebook that keeps itself on disk as an OKF
  bundle. Alchemy writes concept files as sources and notes change, and
  reads back whatever an agent, a `git pull`, or another machine changes
  in the folder. The folder is the notebook's shared surface — Claude
  Code edits a note by editing a markdown file, and the app sees it.
- **A bundle as a source.** Someone else's OKF corpus — a knowledge repo
  cloned from GitHub, a bundle a pipeline maintains — added as a living
  folder source. Read-only, resyncing, citable, the way a vault is.

The spec moved to v0.2 while we weren't looking (the exporter says v0.1):
provenance (`sources`), trust (`generated`, `verified`), and lifecycle
(`status`, `stale_after`) are first-class frontmatter now. Phase 0 brings
the exporter and the nightly OKF snapshot up to v0.2 first; both shapes
then write and read those fields where they mean something to us, and
preserve the rest untouched — the spec's own round-trip rule.

## 1. Goals

- A bound notebook and its bundle agree within seconds, in both
  directions, without the user exporting or importing anything again.
- Agents get the notebook as files. No MCP call is needed to edit a note;
  `cat`, `sed`, and `git` are the API. MCP still exists for the rest.
- A bundle folder behaves like a vault: add once, it stays fresh, its
  concepts are per-file sources with the reader, citations, show/hide.
- Zero new chrome beyond one verb, one chip, and one drop rule.

## 2. Non-goals

- **Chat transcripts in the bundle.** OKF is knowledge, not a log of
  talking about it. Conversations stay in the database, as they do today.
- **Git operations.** Alchemy writes files; who commits them is the
  user's or the agent's business. An auto-commit option can come later.
- **Merge resolution.** Concurrent edits to the same file resolve
  last-writer-wins by mtime (§5.4). Write-through lands within two
  seconds, so real races are rare, and the log records both sides.
- **Attested Computation** and the rest of spec §10. Read and preserved,
  never executed.

## 3. Phase 0: the exporter speaks v0.2

Before either shape, the writer we already have catches up with the spec.
`export_notebook_okf` (and the zip around it) emits v0.1 today; the
import side parses a quoted-scalar subset of frontmatter and nothing
nested. Phase 0 makes the one exporter v0.2-correct, because shapes A and
B both build on it and the nightly escape hatch runs it every night:

- **Frontmatter families.** Every concept carries
  `generated: { by: alchemy/<version>, at }` — `at` is the entity's
  `updated_at` for notes, `created_at` for sources. Notes derived from
  sources carry `sources:` entries (`id`, `resource` as the bundle-relative
  `sources/<slug>.md` path, `title`) for every source the note cites or
  was generated over, so a reader can walk a summary back to its inputs.
  Curator drafts and anything with a non-empty `origin` write
  `status: draft`; archived notes write `status: deprecated`. Sources keep
  `resource` and `tags`; `type` stays the human label it is.
- **Timestamps** are ISO 8601 with an explicit `Z`, everywhere.
- **`log.md`** appends dated entries per spec §9 instead of rewriting one
  line — the nightly loop then reads as a history, not a stamp.
- **Round-trip.** Import parses real YAML (nested maps, lists of
  `verified` entries) and keeps unknown keys; the bound-notebook manifest
  (§5.1) is what carries them back out. The v0.1 quoted-scalar files we
  have already written still parse.
- **The nightly snapshot.** The Night Shift's data-trust job
  (RFC-night-shift-area §7) loops the exporter over every notebook into
  `backups/okf/latest/`. It runs the v0.2 writer with no other change; if
  the loop is not yet wired to the scheduler, phase 0 wires it, gated by
  `background_enabled` like the store snapshot beside it.

Tests: a golden bundle for one notebook (frontmatter families present,
timestamps `Z`-suffixed, `sources:` paths resolve inside the bundle), and
a v0.1 bundle that still imports.

### What `sources:` can honestly say — as built

A note records no source ids. `Note` has ten fields and none of them is
provenance (`models.rs`, `db.rs::notes_schema`); the selection a
generation ran over lives on the in-flight `GenJob` and is thrown away
when the job finishes; report notes carry a prompt and nothing else; and
the one place source ids do get persisted — `LedgerAnchor` on an
auto-evidence assertion — joins back to its note by fuzzy title overlap,
not by id. "Every source the note was generated over" is therefore not
knowable after the fact, and guessing it (every source in the notebook,
say) would put a claim in the file that the data does not support.

What *is* recorded, in the note's own text, is which documents it refers
to. The link graph already reads exactly that, three ways — an absolute
URL, a bare filename, an Obsidian wikilink naming a title — in one
Aho-Corasick pass over the notebook (`graph.rs`). So `sources:` is the
graph's outbound source edges for each note: the citations that are
actually there. Notes that name nothing get no `sources:` key rather than
an invented one. If a note ever grows a real provenance column, the
exporter should prefer it and keep the graph as the fallback.

### `alchemy:` — what the spec has no field for

The spec lets a producer add its own keys (§4.1), and a round trip was losing
everything ours has no home for: a notebook's colour and icon, a source's
real type and the user's own tags, a note's kind. All of it now travels under
a single `alchemy:` map, so it collides with nothing the spec defines and
nothing another producer writes. A reader that does not know the key ignores
it; ours reads it back.

- **Root `index.md`** gets frontmatter for the first time — it was the one
  concept document in the bundle without any, so the notebook's own identity
  had to be guessed back from the H1. It now carries `type: Notebook`,
  `title`, `description`, `generated`, and `alchemy: { id, color, icon }`.
- **Sources** carry `alchemy: { id, source_type, tags, author, image_url,
  parent }`. `source_type` is the real type — the spec-facing `tags:` stays
  what it was — and `tags` is the user's own labels, which are ground truth
  and feed routing. `parent` is the folder/git/notion parent's **slug**, so a
  folder source's shape survives; it resolves at emission, where every other
  cross-reference does, and import resolves it back in a second pass because
  a child's file can sort ahead of its parent's.
- **Notes** carry `alchemy: { id, kind, origin, status }`. `type:` is a human
  label and several kinds share one, so `kind` is what makes a Study Guide
  come back a study guide.

Two decisions inside that:

- **The notebook id is reused; entity ids are not.** Importing into a new
  notebook takes the bundle's `alchemy.id` when nothing here already claims
  it, so binding an export back to the notebook it came from lands on that
  notebook rather than a second copy. A collision means this is a merge, and
  a merge mints. Sources and notes always mint — reusing their ids would
  collide the moment a bundle is merged into a notebook that already holds
  them, and the manifest already carries the id→path mapping that matters.
  The ids are still emitted, so an outside tool can correlate.
- **`alchemy` is one of Alchemy's own keys**, not an unknown one. Otherwise
  the manifest would carry a stale copy through as "somebody else's key" and
  the next write would emit the block twice.

Bundles written before this still import: no frontmatter on the root index
means the H1 names the notebook, no `alchemy.source_type` means the
spec-facing `tags:` decides, and no `alchemy.kind` means the `type:` label
does — which is exactly the pre-namespace behaviour.

### Two other decisions phase 0 had to make

- **Where the nightly loop sits relative to the gate.** RFC-night-shift-area
  §7 says the snapshot is "gated only by `background_enabled`", but the
  code puts the store clone *ahead* of that gate — a clone is a metadata
  operation and losing the library is unrecoverable. The OKF loop is not
  in that class: it reads every source's text out of the store and writes
  a file per concept. It goes behind the gate, and off the pass thread
  (`tauri::async_runtime::spawn` with a running flag, the shape the
  reports batch already uses), so a large corpus cannot stall the minute
  tick. Once-per-day is stamped on disk (`backups/okf/last-run`) rather
  than in memory, so a relaunch does not buy the day a second full pass.
- **The nightly directory is rewritten, but its log is not.** `latest/`
  replaces each notebook's concept files and drops the ones the notebook
  no longer has (and drops whole directories for notebooks that are
  gone), while `log.md` accumulates — which is what makes the copy read
  as a history. Import also stops stamping notes with the moment they
  arrived: a note keeps the age `generated.at` (or the older
  `timestamp:`) records, and a concept marked `status: deprecated` comes
  back archived, so a note retired on one machine stays retired.

## 4. Bundle as a source (shape B)

The easy half, and the first thing after phase 0. Detection follows the Obsidian rule:
a picked or dropped folder is an OKF bundle when `probe_okf` says so
(an `index.md` at the root, or `sources/` / `notes/` beneath it). The
folder-source pipeline runs as today with a new `source_type` of `okf`
— same machinery as `obsidian`, distinct identity — and four bundle
behaviors:

- **Reserved files skip.** `index.md` and `log.md` at any level are
  listings and history (spec §3.1), never concepts. They neither ingest
  nor count.
- **Frontmatter is provenance, not prose.** Leading YAML is stripped from
  embed text and surfaced in the reader header, exactly as the vault
  rule does it. `title` names the child source; `description` and `tags`
  join the chunk's embed prefix the way vault tags do.
- **Lifecycle shows.** `status: deprecated` children start hidden
  (the per-file show/hide that folder sources already have), and a
  concept past its `stale_after` wears a stale badge in the panel and
  the reader. Trust tier (spec §5.3) rides the same header row: unverified,
  machine-confirmed, human-reviewed.
- **Links resolve within the bundle.** Bundle-relative and root-relative
  markdown links (`/tables/orders.md`) rewrite to the target child's
  source id at chunk time so citations hop between concepts. External
  links pass through.

Resync is the existing sweep plus FSEvents for open notebooks. No new
cadence, no new watcher.

**The drop rule changes.** Today any OKF folder dropped anywhere opens
the import dialog. New rule, stated once: **a folder is a living source;
a zip is an import.** A bundle folder dropped on a notebook (or picked
through Add Source, or added from a Spotlight hit) becomes an `okf`
source. An `.okf.zip` dropped anywhere still imports — a zip cannot stay
live. Home's Import… dialog keeps importing folders one-shot for people
who want a copy rather than a link, and gains the binding checkbox in §5.5.

### An allowlist, because a bundle is usually a repository

Reserved-file skipping is not enough. An exported bundle someone then runs
`ok init` in grows `.ok/`, `.okignore`, `.claude/`, `.codex/`, `.cursor/`,
`.pi/`, `.opencode/`, `.github/`, `.mcp.json`, `opencode.json`, and
`.gitignore` — tooling, not knowledge, and a list that will keep growing. A
skip list cannot be maintained against that.

So both readers use the same allowlist instead: a bundle's knowledge is
`sources/**.md` and `notes/**.md`, nothing hidden anywhere along the path,
and never `index.md` or `log.md`. Everything else in the folder belongs to
somebody else. The reconciler walks the same rule to any depth, so the ingest
side and the read-back side agree on what a bundle contains.

Ignore files are not load-bearing here, and must not become so. `ok init`
writes its scaffolding into `.git/info/exclude`, which ripgrep's walker does
honour — but that only helps in a git repository, and it does not cover
`.github/` or `.pi/extensions/` even there. The test walks a real OK project
with every ignore mechanism switched off, so it measures the rule rather than
the exclude file.

### Where the lifecycle lives — as built

`status`, `stale_after`, and the trust tier are not columns. A source row is
listed without its text on purpose (`query_sources` projects content away, so
the panel never reads a notebook's full corpus to draw a list), which rules
out reading the frontmatter at render time; and three new Lance columns for
a feature this size is the migration hazard the shared dev/prod store policy
exists to avoid. So the scan writes what each concept says about itself into
a machine-local sidecar keyed by parent id — `okf_lifecycle/<parent>.json`,
the shape `EmbedOverrides` already uses for per-file repo tiers — and one
command hands the notebook's whole map to the front end. Derived state, not
user data: deleting the file costs a rescan and nothing else.

Two readings the spec leaves open, decided here:

- **Trust tier.** Spec §5.3 names three tiers but not who counts as a
  machine. An actor in a `verified` entry written `name/version` is a tool
  (that is the shape `generated.by` uses); anything else is a person, and a
  human review outranks a machine one.
- **"Deprecated starts hidden."** Hidden means deselected, not unlisted: the
  concept stays in the panel, stays readable, and stays out of answers until
  the user ticks it back on. It is a default the first time the app sees the
  concept, never an override — a concept the user has already ruled on keeps
  their answer.

One thing generalized rather than special-cased: a `description:` in leading
frontmatter now joins the chunk's embed prefix beside tags, for any
frontmatter-bearing markdown, not only bundle concepts. A vault note that
describes itself deserves the same benefit, and forking the chunker by source
type to withhold it would have been the worse code.

## 5. A bound notebook (shape A)

### 5.1 The binding

A notebook is bound to at most one bundle root. The binding lives in a
machine-local sidecar, `<app-data>/okf-bindings.json`, keyed by notebook
id — not in a notebooks column. Paths are per-machine state (like
`mcp.json`), a column would sync them somewhere they mean nothing if
notebooks ever travel (RFC-sync-backend), and the shared dev/prod store
makes column migrations a release-timing problem we do not need here.

Inside the bundle, Alchemy keeps `.alchemy/manifest.json`: for every
concept file it manages, the entity id, the path, the hash of the body
and frontmatter it last wrote, and the unknown frontmatter keys it must
carry back out. Dot-directories are not concept documents (spec §3.1
reserves only `index.md` and `log.md`, and every consumer skips hidden
dirs), so the manifest is invisible to other tools and harmless in git.

### 5.2 Write-through

Every mutation that touches a bound notebook's sources or notes — add,
delete, rename, refresh, retag; note create, update, delete, curator
moves — schedules a bundle write. The writer debounces two seconds per
notebook (`fswatch::DEBOUNCE` is the same number for the same reason:
a sweep touching forty sources lands as one write) and then:

1. Rewrites only the concept files whose entity changed since the
   manifest's hash, and moves a file when its title re-slugs (the
   manifest knows the old path; a rename is `rename(2)`, which git reads
   as a move).
2. Regenerates `sources/index.md`, `notes/index.md`, and the root
   `index.md` — cheap, deterministic, always whole.
3. Appends one dated entry to `log.md` naming what changed (spec §9),
   attributed `alchemy/<version>`.

The exporter's frontmatter grows the v0.2 fields Alchemy actually knows:
`generated: { by: alchemy/<version>, at }` on every note it wrote, and
`sources:` on generated notes listing the cited concept paths, so a
reader of the bundle can follow a summary back to what it summarized.
Keys the manifest carried in from an outside edit (`verified`, custom
tags, anything) are re-emitted verbatim. The existing `export_notebook_okf`
becomes the seed pass of this writer, not a separate code path.

### Where the write-through hooks — as built

§6 says "subscribe where `sources://changed` is emitted and in the note
mutation commands", which is a list of a dozen call sites and a standing
invitation to miss the thirteenth. Two hooks cover all of it, because the
code already has two places that mean "this changed":

- **`Db::touch_notebook`** — every source mutation calls it (add, delete,
  rename, refresh, retag, folder scan), and it takes the notebook id.
  Hooking storage from the writer is a layering cost paid deliberately: the
  call inserts a deadline into a map and returns, so nothing reenters the
  database, and an unbound notebook costs one file read.
- **`index_note`** — every note create and every note edit re-indexes, which
  makes it the one place a note's current text is known. Deletion is the
  exception (there is nothing left to index), so `delete_notes` reads the
  owning notebooks before the rows go.

Two smaller decisions: `write_bundle` is one function for both the seed pass
and every write after it — a bundle nobody has written has an empty manifest,
so everything counts as changed, which is exactly what a first export means.
And a pass that changed nothing writes no log entry: a nightly "no change"
line is not a history.

### 5.3 Read-back

`fswatch` adds the bundle roots of bound notebooks to what it already
watches for open notebooks, and the closed sweep walks them on the same
ten-minute window. A change under `notes/` or `sources/` reconciles
against the manifest:

| On disk | In the manifest | Alchemy does |
| --- | --- | --- |
| new `.md` under `notes/` | absent | create the note (`kind` from `type:`, as import does) |
| new `.md` under `sources/` | absent | ingest as a source through the import path |
| changed, hash ≠ what we wrote | present | update title/content; sources re-chunk and re-embed |
| changed, hash = what we wrote | present | nothing — our own write echoing back |
| file gone | present | delete the entity, log it |

Echo suppression is the hash, not a timer: the writer records what it
wrote before it writes, so a watcher event for our own file compares
equal and stops. An outside edit that carries `generated.by` sets the
note's origin to that actor rather than clearing it, so a curator-managed
note edited by an agent stays attributed. A deliberate in-app edit still
clears origin, as today.

### 5.4 Conflicts

Both sides changed since the last sync means the file's mtime is newer
than the entity's `updated_at`, or older. The newer one wins; the older
text goes into `log.md` under the entry that recorded the overwrite, so
nothing is lost silently. That is the whole policy. Merge tooling is a
non-goal until someone actually hits this.

### 5.5 Surfaces

- **Notebook ⋯ menu:** "Keep on disk as OKF…" picks a folder. An empty
  folder gets the seed pass; a folder that already is a bundle gets an
  import-then-bind (duplicates skip, as import does now). Bound
  notebooks show "Show bundle in Finder" and "Stop keeping on disk"
  instead. Unbinding leaves the files where they are.
- **Header chip:** a quiet "On disk" pill beside the notebook name,
  hover card with the path and the last write time. Click opens Finder.
- **Home Import… dialog:** when the pick is a folder, a checkbox —
  "Keep this notebook linked to the folder" — binds after importing.
- **MCP / CLI:** `bind_notebook_okf(notebook_id, path)`,
  `unbind_notebook_okf(notebook_id)`; `list_notebooks` reports
  `okf_path`. The skill's OKF section tells agents the folder is the
  editing surface once a notebook is bound.

### Read-back, as built

The §5.3 table is a pure function (`classify`) over the manifest, and §5.4's
rule is another (`disk_wins`), so both are tested without a store standing
up. Four things the RFC did not settle, decided in the writing:

- **A tie goes to disk.** §5.4 says the newer side wins but not what happens
  at equal timestamps. The file wins: it is what a person or an agent just
  saved, and it is the artifact they can see. When the app's version wins
  instead, the disk text goes into `log.md` and a write is scheduled to put
  the app's version back — the overwrite is recorded before it happens.
- **`generated.by` means an outside actor, not any actor.** Every file
  Alchemy writes carries `generated.by: alchemy/<version>`, so "an edit that
  carries `generated.by`" would match a person editing the body and leaving
  the frontmatter alone. Our own by-line therefore reads as *nobody claimed
  this*: origin clears, exactly as an in-app edit does. Another actor's
  by-line becomes the note's origin, which also takes it off the curator's
  list — a note an agent maintains is not the curator's to archive.
- **A source has no `updated_at`**, so the conflict clock for one is
  `max(fetched_at, created_at)` — when its text last came in, which is the
  same question.
- **Reconcile stands down while a write is in flight.** Reading a bundle
  mid-write would see half of Alchemy's own pass and call it an outside
  edit, so the reconciler returns early when the notebook has a pending or
  running write. The hash is still the real echo suppressor; this only
  avoids the window where the manifest and the files disagree.

## 6. Plumbing

- `okf` source type: `add_source_folder` detection beside the `.obsidian`
  check; reserved-file skip and frontmatter strip in the folder ingest
  path (the vault code does both already — factor, don't fork); link
  rewrite next to the wikilink resolver.
- Bindings sidecar and manifest: `okf.rs`, new. Owns the writer, the
  reconciler, and the manifest format. `commands.rs` keeps the IPC
  wrappers and the seed export, which moves into `okf.rs` as the writer's
  full pass.
- Hooks: the writer subscribes where `sources://changed` is emitted and
  in the note mutation commands; the reconciler is a third caller of
  `resync_sources_filtered`-style scoping in `fswatch.rs`.
- Frontmatter: replace `parse_okf_doc`'s quoted-scalar subset with a real
  YAML parse for reading (nested `generated`, lists of `verified`
  entries), keep hand-written emission for what we write.

## 7. Tests

- Round trip: seed → bind → edit a note file → note updates; edit a note
  in-app → file updates; the second write of an unchanged note is a no-op.
- Echo: a write-through never triggers a reconcile.
- Rename: retitling a note moves its file; the manifest follows.
- Preservation: a file with `verified:` and a custom key round-trips both
  through an in-app edit.
- Deprecated and stale concepts in an `okf` source start hidden and
  badged; `index.md` and `log.md` never become sources.
- The drop rule: a folder becomes a source in a notebook, a zip imports.

## 8. Phasing

0. The v0.2 exporter and the nightly loop (§3). Every later phase writes
   through it.
1. Shape B — the `okf` source type and the drop rule. Small, ships alone.
2. Shape A write-through — bindings, manifest, writer, chip, menu, MCP.
3. Shape A read-back — watcher roots and the reconciler.

Each phase is useful on its own: a bound notebook that only writes is
already an always-current export.
