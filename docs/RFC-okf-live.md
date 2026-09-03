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

**A path already held is a path kept.** §5.2 said a concept whose title
re-slugs moves, which read as "recompute every name from the title on every
pass" — and that is how one file got destroyed. A conflict copy carrying the
same `title:` as an existing note made a second concept; the newcomer took the
base slug, the older concept was renamed on top of it, and the base-slug file
went with it while the manifest still claimed it and `index.md` still linked
it. So placement now runs in two passes: a concept whose manifest path is
still in its own slug's family (`orders.md`, or the `orders-2.md` a collision
gave it) keeps that path, and only what is left picks a fresh name, avoiding
everything already claimed. A file moves when its title genuinely re-slugs and
at no other time. Two more rules fall out of the same reading: the writer
never renames onto, and never removes, a path another manifest entry claims.

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

- **Notebook ⋯ menu:** "Keep on disk as OKF…" picks a folder. A folder
  that does not exist yet is created — the picker always makes one, but
  `bind_notebook_okf` used to refuse a path an agent had not `mkdir`'d and
  nothing said so. An empty folder gets the seed pass; a folder that already is a bundle gets an
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

- **A delete is logged where the other side can read it.** §5.3's table says
  "delete the entity, log it", and the pass recorded it with `note!`, which is
  stderr and reaches neither `log.md` nor the app log. On a shared folder that
  entry is the only record the other Mac has of why a concept disappeared,
  which is the reason the table asks for it, so the reconcile pass now appends
  one dated line naming every path it removed — beside the write lines and the
  conflict-loser lines that were already there.

- **A delete needs two sightings, and stands down in an outage.** §5.3's
  table reads "file gone → delete the entity", and the pass took one look. On
  a Mac whose bundles are arriving over iCloud that is wrong twice over: a
  folder mid-move between two Notebooks roots is absent for as long as the
  move takes, and a bundle the other Mac is still downloading is absent for
  longer. So the manifest entry carries `missing_since`, the first pass that
  misses a claimed file only records when it missed it, and only a pass at
  least a minute later may act. And a bundle that loses **more than a third**
  of its claimed files at once is a sync outage, not a person deleting: the
  whole delete step stands down for that pass, nothing is even marked, and the
  reason goes to the app log. A file genuinely deleted is still deleted next
  pass; a file the cloud is carrying is not. The policy is one pure function
  over the manifest (`vanish_verdict`), so the clock is an argument rather
  than something a test has to wait on.

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
- **Read-back claims the file it read.** §5.3's table said "create the note"
  and stopped there, which left the file itself unclaimed: the writer then put
  the new note at *its* slug, the original stayed unknown to the manifest, and
  the next pass took it in again. Three notes became fifty-five in five
  minutes on a Dropbox bundle. So a `Create` records the path it came from as
  that entity's path, marked `adopted` — **the file is the concept**, and the
  writer keeps writing to it and never re-slugs it, not even when the title
  changes. A cloud conflict copy is the same shape and gets the same
  treatment; §5.6's "conflict copies need no code" now needs one line of it.
  A retitle therefore moves only the files Alchemy named, which is the right
  side to err on: renaming a file somebody else made is a surprise, and a
  stale slug is not.

- **The clock is read before the bytes.** §5.3's table is written per file,
  which read as "hash every file every pass" — and on 26 bindings, one of them
  holding 742 sources, a serial sweep of that took 228 s to notice an outside
  edit on OneDrive and never finished on Google Drive inside ten minutes. The
  manifest now records each file's own mtime alongside its hash, and a file
  whose clock has not moved is skipped without being opened. An unchanged
  bundle costs one stat per file; a changed one costs reads of the changed
  files. `file_mtime: 0` — every manifest written before this — reads as
  changed, so the upgrade costs one full pass and nothing after it.

  Not the directory's mtime, which is what a first draft of this reached for.
  APFS does not bump a directory when a file inside it is written in place, so
  a whole-bundle skip on `sources/` and `notes/` mtimes would silently stop
  reading back `printf >> note.md` on a closed notebook — the exact class of
  bug this is fixing. The saving it would buy over the per-file stats is
  microseconds against the minutes that were the actual problem.

  The watcher's half is timing, not cost: `rearm` now runs on every bind as
  well as on every open-set change, and the bound roots no longer sit behind
  the folder-source table read, so a folder somebody just asked the app to
  keep in step is watched from that moment rather than up to a minute later.

- **Reconcile stands down while a write is in flight.** Reading a bundle
  mid-write would see half of Alchemy's own pass and call it an outside
  edit, so the reconciler returns early when the notebook has a pending or
  running write. The hash is still the real echo suppressor; this only
  avoids the window where the manifest and the files disagree.

### 5.6 Shared folders: iCloud, Dropbox, two Macs

A bound bundle in iCloud Drive (or Dropbox, Google Drive, a git remote
pulled on both ends) is the free tier of RFC-sync-backend: the folder is
the transport, every Mac runs the same writer and reconciler, and nothing
new moves over a wire we own. It covers one person with two Macs and one
household sharing a notebook; it does not cover chat history, the ledger,
or a coworker without the same cloud account — those stay with the relay
in that RFC. It is the sync RFC's option (d) done at the layer where it
works: markdown concepts merge per file, a LanceDB directory does not.

The rules that make it hold:

- **The manifest is per machine and lives outside the bundle.** Entity
  ids are this machine's; another Mac binding the same folder must never
  read them, and two writers must never fight over one file. The manifest
  moves to `<app-data>/okf/<binding-id>.json`, keyed by the binding, and
  `.alchemy/` inside the bundle is retired. A bundle then carries nothing
  machine-shaped at all.
- **Log entries are per writer.** Both Macs appending to one `log.md` is
  its own newest-wins race, and the entry that records a lost conflict is
  the one that must not lose. Each writer appends under its own dated
  heading with the actor in the line, and a reconcile takes a log change
  from the other side as text, never as a concept.
- **Writes carry a person, not just the app.** `generated.by` becomes
  `human:<account>` (the macOS short name) when a person edited the
  entity in the app, and `alchemy/<version>` only for what the app made
  on its own — a generation, a curator move, a refresh. Read-back keeps
  the same distinction: an outside `human:` by-line is a person, and
  §5.3's origin rule treats it as one.
- **Cloud stubs hydrate, then reconcile.** An undownloaded iCloud file is
  a dot-stub, which the allowlist skips; folder sources already nudge
  stubs to hydrate in the background, and bound roots use the same nudge
  so a note written on the other Mac lands here without a Finder visit.
- **Conflict copies are notes.** Dropbox and Drive resolve a clash by
  writing `<name> (conflicted copy).md` beside the original; iCloud keeps
  the newer mtime and stashes the other as a version, which is the same
  policy as §5.4, so both layers pick the same winner. A conflict-copy
  file is a new concept and imports as a note with that title — the
  "keep both" outcome, for free. If §5.4's overwrite ever bites for
  notes, the cheap upgrade is to make the app's loser a sibling note
  titled the same way rather than a log entry; not a three-way merge.

Test: two data dirs, one folder. Bind on both, write a note on A, read
it on B, edit it on B, read it back on A; each side's manifest stays its
own, the log carries both actors, and neither pass echoes.

### Shared folders, as built

- **Unbinding is a claim, so it has to be true.** §5.5 says unbinding leaves
  the files where they are, and said nothing about what happens when a write
  is in flight — so the command removed the entry, an already-running write
  saved the whole bindings map back from the copy it had taken, and the
  binding came back carrying a `lastWriteAt` stamped after the unbind. The
  caller was told `okfPath: null` about a folder Alchemy was still writing.
  Three changes, in order: every read-modify-write of the bindings file takes
  one lock; a write records `lastWriteAt` only while the binding it belongs
  to is still that notebook's, so a finishing write can never resurrect or
  overwrite an entry; and the unbind removes the binding *first*, drops the
  pending deadline, waits briefly for a running write to finish, and then
  reports what is actually on disk — an error, if the entry somehow survived.
  The MCP tool goes through the same function rather than reaching for the
  sidecar itself, because the answer it prints is the thing that was wrong.

- **The manifest is keyed by a binding id, not the notebook id.** Rebinding a
  notebook to a different folder has to start from a clean record; inheriting
  the old folder's paths and hashes would make the first write to the new one
  a no-op for every file. So `OkfBinding` mints an `id` and the manifest is
  `<app-data>/okf/<id>.json`. `write_bundle` takes the manifest location as
  an argument, and `None` means a one-shot export into a fresh directory —
  there is no last time to compare against and no record worth keeping.
- **The nightly copy keeps a manifest too**, under `nightly-<slug>`, because
  it still needs to drop the concepts last night wrote for a source that has
  since gone. It lives beside the bindings, never in the bundle.
- **Migration** happens on bind: a `.alchemy/manifest.json` from this
  branch's earlier builds is copied to the new location and the directory
  removed, so an already-bound folder keeps its hashes rather than rewriting
  every file on the next pass.

**Actors, and the one thing the store does not record.** A note's actor comes
off what is already there: an `origin` naming an outside actor wins, `auto`
is the curator, a `kind` other than `note` is a Studio generation, and what
is left is a note a person wrote or edited — `human:<account>`. For a source,
every arrival is an import, and the store's own record of a person touching
one afterwards is the user's tags and their note (both documented as ground
truth from the user). **A bare rename the store does not record at all**,
which left the app credited with a title a person chose. It is recorded now,
beside the store rather than in it: the edit command and its MCP twin stamp
the source id into `<app-data>/okf_human_edits/<notebook>.json`, the same
per-parent sidecar shape the lifecycle uses, and the writer reads it alongside
tags and note. A column was the other option and lost — one store serves the
installed app and every dev build, so a schema change is a release-timing
hazard, and this is a fact only the bundle writer ever reads.

Two knock-on decisions:

- **`human:` origins are not drafts.** §3 says a non-empty `origin` writes
  `status: draft`, but since §5.3 an origin can name a person. A note Kim
  edited is not a draft, so only machine origins — `auto`, or a
  `name/version` producer — earn one.
- **The same person on the other Mac is not a stranger.** `okf_is_ours`
  covers both `alchemy/<version>` and `human:<this account>`, so an edit made
  on the other Mac by the same short name clears origin as a deliberate edit
  rather than being attributed to a third party. That is the two-Mac case
  §5.6 is about; a different account keeps its name.

**An agent is not the person who left it running.** The sidecar above said
*that* somebody edited a source, and everything downstream read that as the
user — so an agent calling `update_source` over MCP put the user's name on
work the user never did, and the other Mac had no way to tell. The spec has a
grammar for exactly this (§7): `human:<id>` for people, `<producer>/<version>`
for agents. So the sidecar records *who* alongside *when* — `{at, actor}`,
with a bare number still deserializing as this Mac's person, which is all the
older records ever meant — and every write path stamps its own actor. In the
app that is `human:<account>` as before. Over MCP it is the session's
`clientInfo`, normalised into the producer shape: "Claude Code" 2.1.0 becomes
`claude-code/2.1.0`, a client that introduced itself with nothing becomes
`mcp-client/unknown`. Normalisation is also the guard — a client naming itself
`human:kim` gets `human-kim`, because the colon does not survive, so no
producer can forge a person. Notes join sources in the same file, keyed by
entity id: a note created or rewritten over MCP is the agent's, and the
in-app edit that takes ownership of it stamps the person back in (clearing
`origin` alone would have left the agent's name standing). Read-back needs no
change — an agent's by-line is not `okf_is_ours`, so §5.3 stores it as the
note's origin and the far Mac's writer emits it verbatim. `claude-code` on one
Mac reads as `claude-code` on the other, never as whoever is signed in there.

**The log heading carries the account**, not just the day, so two installs
append to different blocks and a cloud tool has a merge instead of a clash.
Two Macs with different short names never collide; two with the *same* short
name still share a heading, which is the one case this does not fix and the
case where the two writers are most likely to be the same person anyway.

**The two-machine test is two manifests and two binding ids** driving one
writer and one classifier against one folder — which is what two installs
are. It is not two `AppState`s: one of those needs an `Ai`, a config path, a
generation queue and a Tauri handle, none of which a unit test can stand up
and none of which this behaviour depends on. The `Db` is not the obstacle;
`AppState` is.

**Conflict copies need no code.** `<name> (conflicted copy).md` and
`<name> 2.md` pass the allowlist, carry no manifest entry, and so classify as
`Create` — ordinary new concepts, which is the "keep both" outcome. Both
copies keep the frontmatter `title:`, so they read as two notes with one
name and the writer's slug dedup keeps both files. Tested rather than
special-cased.

### 5.7 Notebooks on disk by default, in iCloud when it is there

Obsidian, Soulver, Pages and Numbers put their documents in iCloud Drive
without asking, and get sync and sharing for free. Alchemy does the same
with its bundles: §5's bound notebook stops being an opt-in verb and
becomes where a notebook lives. The store stays local — it is the index —
and the bundle is the portable truth, which is the split RFC-sync-backend
already draws.

**One Notebooks folder.** A single setting, `notebooks_dir`, with a
picker beside it in Settings ("Notebooks folder", Change…, Show in
Finder). Its default is resolved once, on first launch:

| iCloud Drive | default |
| --- | --- |
| the build carries the iCloud container entitlement | the container's `Documents/` (stage two, below) |
| on (`~/Library/Mobile Documents/com~apple~CloudDocs` exists) | `iCloud Drive/Alchemy/` |
| off | `~/Documents/Alchemy/` |

Not `Documents/Alchemy` when iCloud is on: Desktop & Documents syncing is
a separate switch most people leave off, so `Documents` is not a sync
location one can rely on. Dropbox and Drive users point the picker at
their folder. A Mac with iCloud Drive off gets a local folder and works
forever.

**Two stages.** Stage one is the plain folder above — no entitlement,
shipped in v0.55.0. Stage two is the app's own iCloud container, the
branded "Alchemy" folder with the app icon at the iCloud Drive root,
which needs the iCloud entitlement and an embedded provisioning profile
in the bundle: a signing and release-script change, and the same
container an iPhone app would read. Migration between stages is a folder
move; nothing in the bundles changes. Built — see "The iCloud container
(stage two), as built" below, and RELEASE.md for the portal setup that
switches it on.

**New notebooks bind at creation.** With `keep_on_disk` on (default on,
cost control only), a new notebook gets `<notebooks_dir>/<slug>/` and the
seed pass before its first source lands. Slugs dedupe the way the
exporter's do. System notebooks (Briefs) never bind. Existing notebooks
get one offer, once, on the first launch after upgrade — a banner in the
HealthBanner style: "Keep your notebooks on disk?" with Keep on disk and
Not now; Keep binds every active notebook in one pass with progress.
Not now leaves the ⋯ verb from §5.5 as the way in.

**A second Mac opens what it finds.** The Notebooks folder root is
watched like a bound root. A subfolder that probes as a bundle and is
bound to no notebook here is opened — import then bind — and announced
as an arrival ("Opened *Ferrari research* from iCloud"), whether it came
from the other Mac, from a share, or was there at first launch (batched
then, one announcement). A folder that fails the probe is left alone; a
bundle whose `alchemy.id` matches a notebook already bound elsewhere is
the same notebook and rebinds rather than duplicating.

**Sharing is Finder's.** iCloud folder sharing works on any folder, so a
bound notebook's folder is shareable today. A "Share folder…" verb on a
bound notebook reveals the folder in Finder with a one-line hint; calling
the share sheet itself (`NSSharingService` cloud sharing) is native code
and waits for cider or a sidecar. What the other person opens is a
bundle in their own Notebooks folder, which the paragraph above handles.

**Eviction is the trap.** Optimize Mac Storage can turn any bundle file
into a stub. Read-back has §5.6's hydration nudge; the other direction is
a source whose original (§6) or concept file is a stub when Refresh, the
reader, or Show in Finder wants it. Those hydrate first, bounded, and say
"Downloading from iCloud…" rather than failing. Nothing writes over a
stub: the writer treats a stub as absent and waits for the file.

**Not done here.** The data directory never moves into iCloud. The
entitlement was a separate release item, since built. Multi-writer rules are §5.6's,
originals are §6's; this section only decides where bundles live and that
they exist without being asked for.

### Notebooks on disk, as built

- **The default is a `serde(default)`**, which makes "resolved once, on first
  launch" fall out of how the config already loads: the field is absent until
  something writes it, and the first write is the first launch.
- **`keep_on_disk_asked` is the whole of the one-time offer.** Either button
  sets it, so Not now is remembered as firmly as Keep — the banner is a
  question, not a nag, and §5.5's ⋯ verb stays the way in afterwards.
- **The Notebooks root is watched under the empty notebook id.** `fswatch`
  maps a watched root to the notebook it belongs to; the root belongs to no
  notebook, and no notebook has an empty id, so the debounce loop reads that
  as "look at the folder, not at one notebook". It is watched whether or not
  a window has a notebook open, unlike folder sources — a bundle arriving
  while the user is on the shelf should still open.
- **Opening a found bundle reuses the import path**, which since §3 already
  reuses a bundle's own `alchemy.id` when nothing here claims it. So the
  rebind rule needed only the *other* case: an id this machine already has
  binds that notebook to the folder rather than importing a second copy.
- **Starter notebooks never bind by default.** §5.7 said "every active
  notebook", which on two Macs meant both of them bound their own copies of
  the four seeded samples — different ids per install, so each Mac's
  Notebooks-root watcher read the other's bundles as arrivals and imported
  them. Home ended up listing 47 notebooks, most of them twice, and the `-2`
  folders were exactly the starters. So the offer, at-creation binding, and
  the root watcher all skip them. The explicit ⋯ verb still binds one if a
  person asks: the rule is about what happens without being asked. A starter
  is recognised by its title, because that is the only thing seeding leaves
  behind — `seed_notebook` already skips by title, and a Lance column for a
  fact two callers read is the migration hazard the shared dev/prod store
  policy exists to avoid.

- **The root watcher opens a folder at most once, and never a notebook
  twice.** Three rules, one decision function. It skips a folder some
  notebook here is already bound to (two writers over one file is what §5.6
  forbids), spelling-normalized so a symlinked or trailing-slash binding
  still reads as the folder it is. It never *imports* a bundle whose
  `alchemy.id` names a notebook this Mac has: that notebook is either unbound
  — the folder is its bundle, and it rebinds — or bound elsewhere, in which
  case the folder is a duplicate of its bundle and duplicating the notebook
  to match is the wrong half to fix. And the bindings are re-read per folder,
  under a one-pass-at-a-time flag: the minute tick and the watcher's debounce
  both call this, and two overlapping passes each saw the same folder as
  unbound, which is how a first launch imported bundles its own seed pass was
  still writing.

- **A self-heal, because the state already exists.** Rules are for what has
  not happened yet; two Macs are already carrying the duplicates. One pass at
  launch, before anything writes, puts right four shapes: two notebooks over
  one folder (the older notebook keeps it — a binding carries no clock, and
  the duplicate is always the newer row — and the other is unbound and
  archived), a bound starter (unbound), a binding whose folder's `index.md`
  names a notebook that is bound elsewhere (unbound), and a second copy of a
  starter (archived). **It never deletes.** Every fix is an unbind, which
  leaves the files where they are as §5.5 promises, or an archive, which
  hides a notebook and keeps every row it has — a wrong guess costs a visit
  to the archive, not somebody's notes. What it did goes through
  `diagnostics::record` as well as `note!`: the 0.55.0 duplication left
  nothing in the app log but the startup line, and five bundles had been
  written since.

  **Once ever, not once per launch**, stamped beside the bindings. Every rule
  here undoes something a person may legitimately redo — bind a starter from
  the ⋯ menu, unarchive a copy they want to keep — and a repair that keeps
  happening is a policy. The stamp carries a number so a later pass can run
  when there is something new to fix.

  The stamp covers *that* kind of rule only. Tidying the folders themselves —
  consolidating two folders that claim one notebook, clearing empty
  leftovers, emptying the stage-one folder — undoes nothing a person can
  redo, so it runs every pass instead; see the two entries at the end of this
  section.

- **The offer banner is a third tone.** `error` and `warning` both tint their
  container and wear a warning triangle; an invitation is neither a failure
  nor a degradation, and colour here is semantic (DESIGN.md §2). `offer` is a
  hairline, no tint, and a drive glyph.
- **The stub rule is one predicate, used three times.** `is_evicted_stub` is
  the writer's "this file is here in name only" — the writer skips such a
  file and asks for it, `place_reference` will not copy over it, and
  `extract_any_file` refuses with "Downloading from iCloud…" instead of
  reporting a file plainly visible in Finder as gone. The nudge had to stop
  requiring a Tokio runtime for this: the bundle writer is synchronous, so it
  spawns a blocking task on the runtime and a plain thread off it.

- **A stub is a flag, not a filename — corrected.** The predicate above was
  written as "the file is not there and a `.name.icloud` placeholder is",
  which on macOS 26 and later is never true of anything. iCloud evicts **in
  place**: the file stays at its own path with `SF_DATALESS` in `st_flags`
  and `st_blocks` at zero, and no hidden sibling is written. So did every
  FileProvider mount under `~/Library/CloudStorage` — Dropbox, Google Drive,
  OneDrive — all along. The consequence was not one dead branch but six:
  the hydration nudge never fired from a bound root, `evicted_concepts`
  always returned nothing, all three writer guards were inert, and
  `free_reference_name` called `read` on dataless originals, which forces a
  blocking full download inside the write path. The check is now
  `st_flags & SF_DATALESS`, one call covering all four providers, with the
  dot-stub test kept as the secondary for systems that still write them.

  Two rules follow. **Nothing reads a dataless file** in the writer or the
  reconciler: `stat` is safe and a read is the download, so the reconciler
  counts one as present and unchanged rather than hashing it — reading a
  bundle to compare hashes was silently undoing the user's "free up space",
  one file at a time. And **the nudge is iCloud's alone**: `brctl download`
  has no equivalent for a FileProvider mount, so an evicted file there is
  left where it is until its own client brings it back, which is safe
  precisely because every caller already treats a stub as absent.

  `fswatch::is_scannable` needed nothing after all. It is a lexical rule over
  paths that may not exist, and an in-place eviction fires its events on the
  real path, which the rule already allows; the `.icloud` allowance beside it
  is still what covers the old layout. Datalessness answers through an
  injectable probe for the same reason the hydrator does — the gate cannot
  evict a real file, and a seam a test sets has to be the one the app runs
  through.
- **Asking is injectable, so the test can watch it.** `brctl` answers a
  scratch directory with "Path is outside of any CloudDocs app library", which
  every gate run printed. `set_icloud_hydrator` replaces where a request goes
  for the life of the process — not a `cfg(test)` shim, because a seam a test
  sets has to be the one the app runs through — and the stub test installs a
  recorder, so the assertion is now the paths the writer asked for rather than
  the absence of a write.
- **Agent reachability rides the existing `settings` tool** — `notebooksDir`
  and `keepOnDisk` join `SETTABLE_FIELDS`, `settings_set`, and the `get`
  snapshot, so no new tool was needed. Setting a path that is not a directory
  is refused rather than silently accepted.
- **Sharing is a reveal plus a sentence.** `NSSharingService` is native code;
  what the app can do today is put the user in front of the right folder and
  say what to do there, which is what the verb does.

### The iCloud container (stage two), as built

- **The entitlement ships only with its profile.** A Developer ID app that
  claims an iCloud container without an embedded provisioning profile does
  not launch, so the two are one switch: `Entitlements.icloud.plist` and
  `src-tauri/tauri.icloud.conf.json` sit beside the defaults and
  `scripts/release.sh` selects them only when `APPLE_PROVISIONING_PROFILE`
  points at a profile it has verified — unexpired, and carrying
  `iCloud.com.thrashr888.alchemy`. Unset, every byte of the build is what it
  was. The script also checks the finished bundle rather than trusting the
  bundler's ordering: the profile has to be inside `Contents/` before
  codesign seals it, and finding out otherwise from a notarized DMG is the
  expensive way. RELEASE.md carries the portal steps.
- **`NSUbiquitousContainers` ships unconditionally.** It is what turns the
  container's `Documents/` into "Alchemy" at the iCloud Drive root, with the
  icon, and macOS ignores a container the signature does not claim — so it
  lives in the one `Info.plist` rather than forking it for one release path.
- **The signature is the check, not the directory.** `Mobile
  Documents/iCloud~com~thrashr888~alchemy` does not exist until something
  asks the OS for it, so its absence would read as "no entitlement" on a
  correctly entitled fresh install. `codesign -d --entitlements -` on the
  running bundle answers the question actually being asked; it costs one
  subprocess, memoized, and only on the path that computes the
  `serde(default)` — that is, first launch. Anything not running from a
  `.app` (`cargo test`, the CLI, an unbundled dev build) takes the early
  return and never spawns it, which is also what keeps the tests off
  `codesign` and off anybody's real iCloud folder: `resolve_notebooks_dir`
  and `icloud_move_plan` take the entitlement answer as an argument.
- **Only the folder Alchemy chose is Alchemy's to move.** The migration is
  offered when the entitlement is there, the offer is unanswered, and
  `notebooks_dir` is still exactly the stage-one default with bound bundles
  in it. A Notebooks folder pointed at Dropbox or a second drive is a
  decision the user made, and stage two is not a reason to overrule it —
  Settings keeps the picker.
- **The move keeps the binding, not just the path.** `rebind_moved` rewrites
  `path` and leaves the binding id alone, so the manifest — every hash the
  reconciler has — survives. Minting a new binding would have made the whole
  bundle read as changed on the far side of a folder move.
- **The move takes the whole folder, not the bound half of it.** Relocating
  only bound bundles left the starter notebooks and their `-2` copies in
  `iCloud Drive/Alchemy`, so Finder showed two Alchemy folders side by side
  and the migration read as half-done. Every folder in the old location that
  probes as a bundle now travels, bound or not — a rename, never a delete —
  with the exporter's `-2` rule for destination collisions. Files that are not
  bundles stay where they are, which is why the old folder is removed only
  when it turns out to be empty afterwards: an empty directory going away is
  not a deletion, and anything left in it is somebody's.

  One exception, and it is the two-Mac case again: when the container already
  holds a bundle claiming the same `alchemy.id`, the copy in the old folder is
  the older one. It stays where it is rather than being written over the
  bundle the other Mac synced, and the log names both paths so the choice is
  visible. The banner counts both halves — "Your 19 notebooks and 8 other
  bundles move there. Nothing is deleted." — because a promise about a folder
  should cover the folder.

- **A folder is found by its id, never rebuilt from its path.** The move is a
  folder move on one Mac and an arrival on the other, and 0.56.0 had no
  account of the second half. Mac A moved its bundles into the container;
  iCloud carried the move to Mac B, whose bindings still named
  `iCloud Drive/Alchemy/<slug>`; and Mac B's next write-through `create_dir_all`'d
  all eighteen of those paths back into existence, so the folder both machines
  had just left came back on both of them. Then Mac B's own move matched
  destination *names*, found every one taken, and wrote nineteen `<slug>-2`
  copies of notebooks that were already in the container.

  Four rules, all of them the same rule — **`alchemy.id` is the identity, the
  folder name is only a name**:

  - **The writer never creates a bundle root.** Making one is bind's and
    seed's job. A write pass that finds the root gone marks the binding `lost`
    and writes nothing; subdirectories inside a root that *is* there are still
    created, as they always were.
  - **A lost binding follows its bundle.** The recovery looks under
    `notebooks_dir` for a bundle whose `index.md` claims this notebook's id
    and repoints the binding at it, keeping the binding id and so the manifest
    — every hash the reconciler has. The manifest's clocks are re-stat'd
    against the new location and any that no longer match are zeroed: after a
    move the recorded mtime is a claim about a file that may not be the one
    there now, and a claim like that is better paid for with one full read
    pass than trusted. Only when no bundle anywhere under `notebooks_dir`
    claims the notebook, *and* a previous pass already marked it lost, does
    the notebook get a fresh folder — under `notebooks_dir`, at its plain
    slug, never back at the path it lost. An unreachable Notebooks folder is
    an outage and buys nothing but a wait.
  - **The move checks identity before name.** A notebook whose bundle the
    container already holds is adopted — the binding points there and the old
    copy is left where it is — rather than copied in beside itself as `-2`.
    `-2` is for a genuinely different notebook that shares a title.
  - **The duplicates that already exist are put aside, not deleted.** One
    `alchemy.id` in several folders under `notebooks_dir` keeps one folder and
    the rest are *renamed* into `Duplicates/`, which the root watcher ignores.
    The keeper is chosen from the folder names alone — unsuffixed first, then
    lexically — because both Macs run this over the same synced folder, and a
    keeper picked from local state would have each of them setting the other's
    keeper aside forever. Local state decides only where the local binding is
    repointed afterwards.

    **And it runs on every pass, not once ever — corrected.** It first rode
    the heal stamp, which the two-Mac state showed was the wrong shape for
    it. The nineteen `<slug>-2` folders the other Mac wrote arrived here as
    *empty directories* — iCloud makes the folder and delivers the files
    after — so `bundles_under` could not see one of them, the stamp went down
    anyway, and when the files landed nothing would ever have looked again.
    Unlike the heal's unbinds and archives, this rule undoes nothing a person
    can legitimately redo: a second folder for one notebook is not something
    anybody asked for. So it runs at every launch and on every root-watcher
    pass, for one readdir plus one `index.md` read per root folder, and a
    folder that is not a bundle yet is left alone.

    The same pass takes out **empty directories older than ten minutes**. An
    empty directory is not data, so removing one is not a deletion; but a
    directory a sync client is still filling is empty too, which is why its
    own mtime has to be old before it counts as a leftover.

- **The stage-one folder is emptied on every pass too.** The one-shot offer
  (`icloud_move_asked`) was the only thing that ever looked at `iCloud
  Drive/Alchemy`, and on the real two-Mac state that left ten entries there
  with nothing that would ever look again: unbound starter bundles and their
  `-2` copies, plus two bundles the other Mac recreated after the move. One
  migration's worth of attention is not enough for a folder two Macs keep
  putting things into, and a folder the app no longer writes to is not a
  place to leave somebody's notebooks.

  The placement rules are the move planner's, with one deliberate difference.
  `plan_icloud_moves` *leaves* a bundle whose `alchemy.id` the container
  already holds — right for the duration of one migration, wrong as a
  standing rule, because leaving it is exactly what stranded those ten
  entries. So the leftover is renamed to `<container>/Duplicates/<name>`
  beside the keeper, where the consolidation already puts second folders and
  where a person can find it. Everything else that probes as a bundle moves
  in with the `-2` rule; files that are not bundles stay, empty directories
  over the grace go, and the old folder is removed only once it is genuinely
  empty. Only when the container is the active `notebooks_dir` — a Notebooks
  folder the user pointed elsewhere is still theirs — and never while a write
  for that notebook is in flight, under the same move guard as the migration.

- **Nothing moves under a write in flight, and nothing is deleted.** The move
  waits on §5.2's pending/flushing sets rather than renaming a bundle out from
  under its own writer. A rename that cannot be done in place falls back to a
  copy and leaves the original where it was, logged; a half-finished move must
  leave the user with their files, not a gap.

  **But it waits per notebook, not for the app.** The first version needed
  every bound notebook quiet for fifteen seconds, and on the Mac that most
  needed the move — the one whose watcher was rebinding bundles arriving from
  the other Mac — something was always writing, so "Move them" refused every
  single time. Two changes. A process-wide move-in-progress flag, held by an
  RAII guard so an early return cannot leave it set, makes the writer *defer*
  new scheduling: nothing already pending is cancelled (that is how a notebook
  goes quiet at all) and the held writes are scheduled the moment the move
  ends. And each folder waits only on its own writer, up to thirty seconds; a
  notebook that never settles is skipped and named in the log, and the move
  carries on with the rest. The user-facing refusal is now only for the case
  where nothing at all could be moved.

## 6. Originals travel in `references/`

A PDF, an image, a `.docx` crosses today as extracted text: the other Mac
gets the words and none of the pages, the gallery there is empty, and
Refresh has nothing to re-read. The spec already has the place for this
— §6 names a `references/` subdirectory for the artifacts concepts derive
from, addressable from `resource:`. The bundle grows one:

```
references/2018 488 Spider brochure.pdf   # the original, under its own name
sources/<slug>.md                         # resource: references/2018 488 Spider brochure.pdf
                                          # alchemy: { origin: "file:///…/2018 488 Spider brochure.pdf",
                                          #            sha256: "14030e98bcc8daf5" }
```

**Named by the original file, deduplicated by hash.** A bundle is read by
people as well as by programs, and `2018 488 Spider brochure.pdf` says
what `14030e98bcc8daf5.pdf` cannot. The name is the original's own, kept
as its maker wrote it — spaces, case and unicode included — with only what
a filesystem or a path parser would choke on taken out and the length
capped; a source whose origin has no filename (a clipboard image, a
captured page) falls back to its slug. Identity is still the content hash,
which now lives in the manifest (hash → the file that holds it) and in
`alchemy.sha256`, never in the filename: two sources over the same file
share one reference, the nightly loop and every write-through skip an
original already carried, and a *different* file that happens to be called
the same thing lands as `<stem>-2.<ext>` rather than overwriting it. The
writer removes a reference only when no manifest-claimed concept points at
it any more, so deleting a source takes its original with it and nothing
else does.

**What is copied, and what is only linked.** The distinction is whether
the bundle is the sensible home for the bytes:

| Source | In `references/` | Why |
| --- | --- | --- |
| a file the user dragged or picked (pdf, image, docx, pptx, xlsx, audio) | copied | the notebook is its only home in Alchemy; the other Mac cannot reach the path |
| pasted text, clipped pages, `url` sources | no | the concept body is the capture; a URL is re-fetchable |
| folder, git, Notion, Mac children | linked only | the parent is the origin and resyncs; copying a synced folder into a synced folder duplicates it forever |
| a file already inside the bundle folder | linked, bundle-relative | it is already there |
| anything over the size cap (50 MB, one setting) | linked, logged | one video should not make a bundle undeliverable |

`resource:` says which it was: a `references/` path means the bytes are in
the bundle, anything else is provenance to a place this bundle does not
own. `alchemy.origin` keeps the original machine path either way so a
bind-back can re-link.

**Import and read-back** ingest a `references/` original through the
ordinary file path — pdfium pages, image gallery, docx — and fall back to
the concept body when the reference is missing (evicted, over the cap,
stripped by a tool). The source's own path on the importing Mac is the
reference file, so Refresh and Show in Finder work there too. Zips carry
`references/` under the existing `OkfZipLimits`.

Folder children stay text-only on the far side. That is the same result
as today, and a per-folder "copy originals" opt-in can come when someone
asks for it rather than as a default that doubles every synced folder.

### Originals, as built

- **Which extensions travel.** §6's table names the categories; the list is
  the rich types whose bytes say something the extracted text cannot — PDFs,
  Office documents, images, audio. Plain text, markdown, HTML and CSV are
  their own extraction, so copying one would only duplicate the concept body.
- **The plan is made at gather time and acted on at write time**, because
  only the writer knows where the bundle is — which is also what lets "a file
  already inside the bundle" be a case at all. `gather_bundle` therefore
  takes the destination, and `export_notebook_okf` settles its directory from
  the notebook row (which carries no source text) before reading anything, so
  an export still reads every source's content exactly once.
- **Pruning only removes names the writer chose.** A reference is dropped
  when no manifest-claimed concept points at it *and* the manifest recorded
  the writer choosing that name — a `handout.pdf` someone put in
  `references/` by hand is not ours to delete. The claim has to live in the
  manifest now: a file named after its original is not self-identifying the
  way a sixteen-character hex stem was.
- **Migration is a rename on the next write.** A manifest reference whose
  file is hash-named, and a hash-named file the manifest never mentioned at
  all (which is every bundle the first build of this branch wrote), take the
  original's name in one `rename(2)` — so git sees a move, the concept's
  `resource:` updates in the same pass, and no duplicate is left behind. An
  evicted original is asked for and keeps its name until the bytes land; the
  write after that migrates it.
- **`alchemy.origin` is always written when there is a machine path**, copied
  or not, so a bind-back can re-link. `resource:` is the one that changes
  meaning: a `references/` path means the bytes are here.
- **Originals left behind are logged**, but only when they could have
  travelled and deliberately did not — over the cap. A link because the
  source is a URL or a folder child is that source's nature, not an event.
- **Zip limits.** A bundle zip now carries binaries, so the per-entry cap
  goes to 128 MB (the 50 MB copy cap with room for a scan that only just
  fits) and the total to 2 GB. The entry count and the compression ratio are
  unchanged: neither has anything to do with originals, and both are what
  actually stop a zip bomb.
- **Reference paths are refused if they climb out of the bundle.** A
  `resource:` is untrusted text from a file someone else may have written, so
  `okf_reference_path` canonicalizes and checks containment before returning
  anything to read.

## 7. Plumbing

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

## 8. Tests

- Round trip: seed → bind → edit a note file → note updates; edit a note
  in-app → file updates; the second write of an unchanged note is a no-op.
- Echo: a write-through never triggers a reconcile.
- Rename: retitling a note moves its file; the manifest follows.
- Preservation: a file with `verified:` and a custom key round-trips both
  through an in-app edit.
- Deprecated and stale concepts in an `okf` source start hidden and
  badged; `index.md` and `log.md` never become sources.
- The drop rule: a folder becomes a source in a notebook, a zip imports.
- Two machines: two data dirs bound to one folder converge both ways with
  separate manifests and both actors in the log (§5.6).
- Originals: a dragged PDF exports once under `references/` by its own
  name, a second source over the same file adds nothing, two different
  files called the same thing become `paper.pdf` and `paper-2.pdf`, a
  bundle left by the hash-named layout renames in one write with no
  duplicate behind it, deleting the last owner removes it, and import
  re-ingests it as pages rather than text (§6).
- Notebooks on disk: default resolution with and without the iCloud root
  present; a new notebook creates and seeds its folder; a bundle dropped
  into the Notebooks folder becomes a bound notebook exactly once; a
  bundle carrying a known `alchemy.id` rebinds instead of duplicating; a
  stub is treated as absent by the writer (§5.7).

## 9. Phasing

0. The v0.2 exporter and the nightly loop (§3). Every later phase writes
   through it.
1. Shape B — the `okf` source type and the drop rule. Small, ships alone.
2. Shape A write-through — bindings, manifest, writer, chip, menu, MCP.
3. Shape A read-back — watcher roots and the reconciler.
4. Shared folders (§5.6) — manifest out of the bundle, per-writer log,
   actors, stub hydration, the two-machine test.
5. Originals in `references/` (§6) — writer, import, read-back, zip.
6. Notebooks on disk by default (§5.7) — the Notebooks folder, binding at
   creation, the upgrade offer, and opening what a second Mac finds.

Each phase is useful on its own: a bound notebook that only writes is
already an always-current export.
