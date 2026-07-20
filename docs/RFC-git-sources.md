# RFC: git sources — repos, subtrees, and files as living sources

Paste `https://github.com/owner/repo` into Alchemy today and you get one
readability-mangled page capture of the repo home — `source_type:"url"`, no
awareness that a whole repo sits behind it. The reminder that spawned this
RFC wants git URLs (github.com and github.ibm.com) "to capture technical
context as well as business context," and asks: whole repo or sub-folder or
file? What about code-aware chunking and retrieval? This RFC answers: all
three scopes, one mechanism — and for retrieval, *embed the prose, grep the
code, outline the symbols*.

The unlock is that Alchemy already has two halves of a git connector:
**folder sources** (parent row + per-file child sources, mtime-reconciled by
`rescan_one_folder`, swept every minute) and **living sources** (`cider://`
origins re-synced by content hash stamped into `mtime`). A git source is a
folder source whose root is a git working tree — either one the user already
has on disk, or a managed shallow clone — plus ignore-aware file selection,
a size tier, and a throttled remote-change probe.

## 1. Input shapes

| Input | Default scope | Example |
|---|---|---|
| Repo web URL | **README only** (widenable) | `https://github.com/owner/repo` |
| Tree URL | subtree at ref, docs & code | `https://github.com/owner/repo/tree/main/src/retrieval` |
| Blob URL | single file at ref | `https://github.com/owner/repo/blob/main/src/db.rs` |
| Clone URL | whole repo, docs & code | `git@github.ibm.com:org/repo.git`, `https://host/any/repo.git`, `ssh://…` |
| Local path in a repo | that file/folder, docs & code | `/Users/…/Workspace/QDOS/src` |

**The URL's specificity is the intent signal.** Pasting a repo-home URL
means "the page I'm looking at" — which renders as the README — so that's
what it captures: one clean markdown source with provenance, strictly
better than today's chrome-mangled page capture, and *no clone of the
whole damn repo*. Mechanically it's the blob case with an auto-resolved
path (a kilobyte sparse fetch), so it costs almost nothing and re-syncs
via the same sha probe. A `/tree/` path or an explicit clone URL is a
deliberate reach for content, so those default wider. Widening later is
one click on the Scope row (§2's widen-in-place flow). Non-repo GitHub
paths — `/releases`, `/issues`, `/pulls`, `/wiki` — are API surfaces, not
git: they fall through to normal page capture.

Web-URL parsing is GitHub-shaped and applies to github.com **and any host
that answers a git probe** (§8) — which is how github.ibm.com works with
zero configuration. Clone URLs are host-agnostic by construction. Refs with
slashes (`feature/x`) are disambiguated by longest match against
`git ls-remote --heads --tags`. GitLab/Bitbucket *web* grammars (`/-/tree/`)
are deferred; their clone URLs already work. Two sources on different refs
of the same remote share one object store via `git worktree` — the 2026
agent-tooling ecosystem's isolation primitive, reused here as cheap dedupe.

## 2. One design: a working tree behind a folder source

- **Local path** inside a repo (`git rev-parse --show-toplevel` succeeds):
  stays a `folder`/file source — no new type — but the scanner becomes
  ignore-aware (§3) and the parent gains git provenance. Pointing Alchemy
  at a checkout stops ingesting vendored junk and starts ingesting code.
- **Remote URL**: `source_type:"git"` parent row, `url` = normalized origin
  URL. Backing store is a managed clone under `<app-data>/git/<source-id>/`:
  `clone --depth 1 --filter=blob:none [--branch <ref>] --no-checkout`, then
  `sparse-checkout set <path>` for tree/blob scopes, then checkout. Blobless
  + sparse means a one-file source fetches kilobytes, not the repo. After
  checkout, the existing `rescan_one_folder` reconcile ingests children
  exactly as if the user had added a local folder.
- **Children are ordinary sources** (`parent_id` set, typed by extension),
  so citations point at files — "cited: `src/db.rs`" — and a changed file
  re-embeds alone instead of re-embedding a monolithic digest.
- **The parent's `content` is a repo map**: provenance header (remote, ref,
  short sha, commit date), the file tree, per-file symbol outlines (§5),
  counts, and the skip list. Markdown frontmatter (title/description),
  when present, feeds child titles. The map makes the parent itself
  retrievable ("what is this repo?"), gives the semantic router a real
  summary, and follows the existing convention that provenance is a
  content header, not a schema change.

A single blob URL skips the parent: one loose source, same clone mechanics.

**Overlap — one source per remote per notebook.** Adding
`repo/tree/main/docs` when the whole repo is already a source jumps to
the existing parent ("Already in this notebook as rust-helper — covers
`docs/`"), the URL-dedup pattern with a path-prefix matcher. Adding the
whole repo when only a subtree exists offers to **widen** the existing
source in place (same id, scope updated, rescan) instead of creating a
sibling. The one intentional exception: a local checkout and a remote
source of the same repo (matched via `remote.origin.url`) may coexist —
they are genuinely different states (your branch vs. origin) — but the
confirm step says so, and any child whose content is identical across the
two is skipped by the existing content dedup and *listed in the repo map*
("already here via rust-helper (local)"), the no-silent-caps rule again.

## 3. Selection — the `ignore` crate is the scanner

Adopt ripgrep's own machinery in-process: the `ignore` crate (BurntSushi's,
the walker behind `rg`) replaces the hand-rolled folder walk for **all**
folder sources, git or not. It honors `.gitignore`/`.ignore`/global
excludes, skips `.git/` and `node_modules/`-via-gitignore by construction,
handles symlink cycles, and — unlike `git ls-files` — still includes
brand-new files that haven't been committed yet. No subprocess, no PATH
dependency. On top of the walk:

- **Text sniff replaces the extension allowlist.** `FOLDER_EXTENSIONS`
  doesn't include `.rs`/`.ts`/`.py`; rather than chase extensions, accept
  any file that reads as UTF-8 with no NUL in the first 8 KB. Binary
  assets fall out naturally. (Rich types — pdf/docx/xlsx — keep their
  existing extractors.)
- **Skip by name**: lockfiles (`*.lock`, `package-lock.json`,
  `pnpm-lock.yaml`), minified bundles (`*.min.*`), source maps, snapshots,
  `vendor/`/`third_party/` dirs.
- **Caps** apply to the *embedded* tier only (§4). Anything skipped or
  left grep-only is *listed in the repo map with a reason* — silent
  truncation reads as "covered everything" when it didn't.
- Depth: `FOLDER_MAX_DEPTH = 6` is shallow for monorepos; raise to 12 for
  ignore-aware scans (the cap becomes a backstop, not the filter).

## 4. Two tiers: documents embed, repositories grep

A blob URL and a 4,000-file monorepo should not get the same treatment.
At add time, after selection rules, the scope lands in a tier:

- **Document tier** (single files, and scopes ≤ ~50 eligible files / ~1 MB):
  everything embeds as children. It's a document-like source; the existing
  pipeline is exactly right.
- **Repository tier** (everything larger): **the knowledge layer embeds,
  the code layer greps.**
  - *Embedded:* README* everywhere, `docs/**`, all `*.md`/`*.mdx`, the
    repo map with its symbol outlines (§5), and ~200 KB-max per embedded
    file. This is the business context — cheap, high-value, and the kind
    of prose embeddings are actually good at.
  - *Grep-only:* code files are scanned, outlined in the repo map, and
    reachable at **query time** via the ripgrep leg (§6) — never eagerly
    embedded. At rest they cost nothing.
  - *Promote:* any grep-only file or folder can be promoted to embedded
    from the repo reader (§7), and demoted. "Whole repo or sub-folder or
    file? Maybe both?" — this is both, per file, reversibly.

**Invariant: no repo is too large.** The embedded layer is budgeted; the
grep and outline layers scale with the working tree, which is bounded only
by disk. Ingest streams — children appear as the scan proceeds, and the
map's counts tick up — so a monorepo add is progress, not a hang.

The 2026 evidence says this tiering *is* the right retrieval trade, not a
cost compromise: keyword search via agentic tool use reaches ~90% of
embedding-RAG performance on code, while embeddings' edge is conceptual
prose. Cursor embeds everything but needs a server fleet, a custom code
embedding model, and Merkle-tree sync to make it economical; Continue.dev
proves the local version works (tree-sitter chunks in LanceDB — Alchemy's
own engine) but at IDE complexity. A notebook wants the docs *understood*
and the code *findable*.

## 5. Symbols — ast-grep's crates are the tree-sitter vehicle

ast-grep (`ast-grep-core` + `ast-grep-language`) wraps tree-sitter for
~25 languages behind one Rust API and adds structural pattern matching
(`fn $NAME($$$) { $$$ }`). Embedding it buys three things on one
dependency. The grammar set: **rs, ts/js/tsx, py, go, rb, java, c/cpp,
swift, php, html, shell** from ast-grep's built-ins, plus **HCL and
Dockerfile** registered as custom tree-sitter grammars (community
parsers behind the same API — not built-ins, same behavior once wired).
*Measured (2026-07-19):* the full set costs **+43 MB on the debug binary**
(641.6 → 684.7 MB, +6.7%) — release delta to confirm at the next release
build; the set trims if that lands fat. Languages with thin symbol shapes still earn their place: HCL
outlines as resource/module/provider blocks (Terraform repos read
beautifully), Dockerfiles as build stages, shell as functions; HTML gets
grep and chunking without pretending to have symbols.

1. **Symbol outlines in the repo map.** Each scanned code file contributes
   its top-level symbols (functions, types, impls/classes) as a one-line
   outline — *and the map embeds*. So `fn search_chunks_trace` is
   retrievable by BM25 and vector even when `db.rs` itself is grep-tier:
   the outline hit cites the file, the ripgrep leg or reader dives in.
   This is Aider's repo-map insight fused with tiering, and it's what
   makes "infinite length" honest — outlines cost bytes per file, not
   embeddings per function. Top-level only to start; nested granularity
   (methods in impls/classes) waits for retrieval traces to ask for it.
2. **Structural search for agents.** An `ast_search(pattern, lang)` tool
   beside grep for the in-app agent and MCP clients — "find every call
   site of `store_extracted`" as an AST query, not a regex prayer.
3. **Symbol-boundary chunking, eventually.** The chunker upgrade (§6's
   blank-line blocks → function boundaries) becomes a small delta once
   the crates are in-tree — still evidence-gated by retrieval traces.

Outlines and the ast tool land with the repository tier; chunking waits.

## 6. Chunking and retrieval

**Chunking.** `chunk_text` collapses whitespace and packs prose paragraphs —
right for documents, wrong for code. Code files that embed (document tier,
promoted files) get a sibling path: split on blank-line block boundaries,
pack to the same ~280-word budget, **preserve whitespace verbatim** in
`text` (citations show real code), fall back to line windows — never
sentence splits. `embed_text` gets a `[repo › path/to/file.rs]` prefix —
the path header feeds BM25 exact file-name hits and gives the embedder
context the snippet lacks. Markdown in repos keeps the existing chunker.

**Retrieval.** Alchemy's hybrid is already the right architecture: BM25
catches `search_chunks_trace` typed verbatim, the vector side catches
"where does retrieval get fused," RRF merges. One addition, **in plain
notebook chat from day one**: when a notebook contains repository-tier
sources and the query carries code-shaped tokens (identifier regex:
`snake_case`, `camelCase`, `dotted.paths`, quoted strings), run the `grep`
crate (ripgrep's engine, in-process) over those working trees, window the
matching lines, and feed them into the same RRF fusion as ephemeral hits
citing the child file. No index, no staleness, no embedding cost — the
mechanism coding agents ride to ~90% parity, fused into notebook retrieval
instead of an agent loop. Queries with no code-shaped tokens skip the leg.
The agent additionally gets `grep` and `ast_search` (§5) tools over the
same trees for iterate-and-read depth. Verify along the way: Lance FTS
tokenization of `snake_case`/`camelCase` (fix only if traces demand it).

## 7. UI/UX — detection-first add, quiet panel, a repo reader on the paper

Every surface below reuses an existing convention; the only net-new layout
element is a hairline-separated tree pane inside the reader.

**Add flow — detect, then confirm scope.** No new modal tile in v1: the
existing *From URL* and *Add folder* entries morph when git is detected
(smart defaults, like everything else). Detection swaps the modal step for
a **scope confirmation**: identity line (repo glyph, `owner/repo`, branch
chip, short sha in mono), a **scope row** ("scope: `src/retrieval` —
change") whose picker lists the repo's directories — for remote repos the
blobless clone has trees before any checkout, so the folder list is
available in the confirm step at kilobyte cost — a three-step **include
ladder** ("README · Docs · Docs & code", preselected by the intent
mapping in §1), a tier preview for local adds ("412 files ·
2.1 MB — 38 docs embed, 374 code searchable"; remote adds show the split
live during ingest instead), and one cider-style behavior sentence:
*"Cloned shallowly into Alchemy's cache; re-synced hourly. Your git
credentials are used, never stored."* One primary button. Ingest streams:
the parent row appears immediately with ticking counts; children populate
as scanned; cancel lives on the row menu.

**The include ladder — "the code is the docs" is optional.** Three rungs,
one control: **README** (a single markdown source — the repo-home
default), **Docs** (prose only: code isn't listed, embedded, or grepped;
the repo reads like a folder of documents), **Docs & code** (the full
design). *Resolved (2026-07-19): the choice is captured pre-import,
inline* — when the pasted URL parses git-shaped, the modal's URL step
grows a quiet `Import: README · Docs · Everything` pill row, preselected
by the URL shape. No second dialog, no post-hoc re-ingest; unknown hosts
show the row only for unambiguous shapes (clone URLs, /tree, /blob) and
otherwise let the backend probe decide with the shape default. The rung
rides the cache sidecar, so every future rescan honors it; widening later
remains the §2 widen-in-place flow.

**SourcesPanel — the repo is one quiet row.** Parent row: monochrome
branch glyph, title, 12px meta line (`main @ 2236053 · 2h ago`), child
count badge, disclosure. Repo-tier children stay collapsed behind
"Browse 412 files →" (opens the reader tree); document-tier repos expand
like folders today. Per the icon policy, tier rides in dots, not borders:
embedded files carry a small `--citation` dot, grep-tier none, skipped
muted — full detail belongs to the reader, the panel stays calm. Errors
use the existing destructive-tinted pattern ("auth failed — check git
credentials for host", fix hint in a popover). Row actions on
hover/focus-within: refresh, open on GitHub, remove.

**The repo reader.** Opening a git parent fills the uncontained center
column — no third card, per the workspace rule. Anatomy, top to bottom:

- **DocProperties** (the existing primitive, built for exactly this):
  type "Git repository," origin remote as a link-out, branch, commit
  (mono sha + date), synced-ago with an inline Refresh, file counts
  ("38 embedded · 374 searchable · 7 skipped" — the skip count opens the
  reasons popover, the no-silent-caps rule as UI), and the editable
  Scope and Content rows (§ add flow).
- **Two panes under a hairline**: left, a ~240px file tree
  (`@pierre/trees` — path-first, type-to-filter, git-status annotations
  for dirty local files) with the same tier dots; right, content. Nothing
  selected → repo home: rendered README with the map's counts. A file →
  **Shiki** code view (SF Mono, line numbers) using a CSS-variables theme
  so all 21 schemes carry through; arriving via citation scrolls to and
  washes the cited line range (tinted, never bordered). The reader's
  outline rail switches from headings to ast-grep symbols for code files.
- **Per-file header**: an Embedded/Searchable state pill (click = promote
  or demote), Open on GitHub (remote+sha+path), Reveal in Finder (local),
  Copy path — hidden until hover/focus-within, as everywhere.

**Citations.** Chat chips show the file name (`db.rs`), full path in the
tooltip; clicking opens the reader at the highlighted lines. Grep-leg hits
cite identically — provenance is uniform regardless of which retrieval leg
found it. In the composer's source-picker chips a repo collapses to one
parent chip (parent-implies-children expands at query time via the
existing `source_id` filter).

**Sync surfacing.** Freshness lives in the parent meta line and
DocProperties; a resync that changes the sha toasts "owner/repo updated to
`abc123` — 14 files changed." The richer diff-on-resync view is deferred
(§13) along with `@pierre/diffs`, which earns its place only then.

**Settings.** One setting, not a feature flag: **auto-sync cadence**
(15 min / hourly default / 6 h / daily / off — off still allows manual
Refresh). Git sources themselves have no off switch — the smarter thing
is the only thing. A "Clear git cache" action joins later with cache-size
display.

## 8. Sync — probe cheap, re-ingest on change, never touch user repos

- **Local repo sources** ride the existing 1-minute sweep unchanged —
  `rescan_one_folder` already mtime-diffs children; ignore-awareness only
  changes which files are eligible. **Alchemy never runs `git fetch` (or
  anything ref-mutating) against a user's repo** — the working tree on
  disk is the user's truth, indexed as-is. On manual refresh, a read-only
  `ls-remote` compare may annotate the repo map ("working tree differs
  from origin/main") — and the cure for wanting upstream freshness is
  adding the remote URL as a source, where the clone is ours to fetch.
- **Remote sources**: a `sweep_due`-style throttle like mac sources
  (network is the new osascript): **hourly by default, setting-adjustable**
  — `git ls-remote origin <ref>`, one round-trip, no data transfer —
  compared against the sha stamped via `content_stamp` in the parent's
  `mtime` (the exact trick mac sources use; **no schema change**). On
  change: `fetch --depth 1` + checkout + rescan; only changed files
  re-embed. Manual refresh bypasses the throttle. Sources pinned to a tag
  or sha never change and skip the probe entirely. (Cursor's Merkle tree
  solves this at fleet scale; per-file mtime + one sha probe is the
  notebook-scale answer.)

## 9. Auth — the user's git is the credential store

Shell out to system `git` (the `cider` subprocess pattern: timeout, quiet
env) with `GIT_TERMINAL_PROMPT=0` so nothing ever hangs on a password
prompt — but *only* for clone/fetch/ls-remote/provenance; scanning and
searching are in-process crates (§3, §5, §6). Inheriting SSH agents,
credential helpers, and `gh auth` is precisely why github.ibm.com works:
the user's git already authenticates there. Alchemy stores no tokens, adds
no keychain surface, and a failed clone lands as `status:"error"` with
"check your git credentials for <host>". The assisted-capture stance
transplanted: the user's own access is the bypass; we never hold secrets.

Detection for GHE web URLs needs to know a host is GitHub-shaped without a
hosts list: when a pasted URL matches `owner/repo(/tree|/blob)…` shape on
an unknown host, probe `git ls-remote <https://host/owner/repo>.git` (~2 s
cap) before falling back to page capture. The verdict is remembered per
host in the same style as `capture_domains.json`, so the probe runs once
per domain, ever. Default-on; the toggle is cost control.

## 10. Write-back — decided: skipped for v1

Captured pages are records; editing them falsifies provenance — never.
Editing files in local working trees fits the cider write-through pattern
technically, but the honest use case (markdown only; code belongs in
editors) is thin, and remote-clone edits are a data-loss trap outright.
**Decision (2026-07-19): no write-back in this RFC.** The direction that
does fit Alchemy is the inverse — notes *out* to a git repo via the
existing OKF export (OpenKnowledge's model: markdown in git, history as
attribution, agents co-editing; code.storage-style hosted remotes slot in
there if a server is ever wanted). That's its own future RFC.

## 11. Plumbing (where it plugs in)

- Crates: `ignore` (walking), `grep-searcher`/`grep-regex` (query-time
  search), `ast-grep-core`/`ast-grep-language` with a curated grammar set
  (outlines + structural search) — all in-process. System `git` subprocess
  for clone/fetch/ls-remote/rev-parse only.
- `is_git_uri()` alongside `mac::is_mac_uri` — branch in the MCP
  `add_source` dispatch (mcp.rs), `refresh_source_url`, and the deep-link/
  clipboard path, so the Chrome-extension and `alchemy://add` flows get git
  for free.
- `ingest_git` mirrors `ingest_mac`: dedup on normalized origin URL →
  clone/checkout → parent row + rescan → stamp `mtime`.
- New `git.rs` module: URL grammar, probe + host memory, clone/fetch/
  worktree wrappers, tier logic, repo-map + outline rendering.
- Settings: `git_sources` default-true flag, probe cadence, "Clear git
  cache" beside the capture-profile clear.

## 12. Alternatives considered

| Option | Verdict |
|---|---|
| **GitHub REST API** (contents/tarball endpoints) | Needs token management for private/GHE, rate limits, GitHub-only. Clone-with-user's-git covers public+private+GHE+GitLab uniformly with zero stored secrets. |
| **Embed everything eagerly** (Cursor/Continue shape) | Right for an IDE with a server fleet or an IDE-sized index budget; wrong default for a notebook adding a 4k-file monorepo. Tiering + outlines + promote gets the value without the bill; traces can revisit. |
| **Single-source digest** (gitingest/repomix style) | One blob = citations point at a 2 MB wall, any change re-embeds everything, and it bypasses the folder machinery that already does per-file reconcile. The repo map keeps the digest's orientation value without its costs. |
| **git2 / gix crates** | Static linking and no subprocess, but libgit2 credential callbacks fight ssh-agent/credential-helper setups — auth is the hard part, and system git solves it. Revisit if shelling out proves brittle. |
| **Full clone** | `--depth 1 --filter=blob:none` + sparse gets a one-file source in kilobytes; history is irrelevant to a notebook. |
| **FS-watch local repos** | The minute sweep already reconciles; watching adds a subsystem for latency nobody asked for. |
| **Trigram index** (Zoekt-style lexical) | Fleet-scale infrastructure for sub-second search over millions of repos; ripgrep over one working tree is milliseconds anyway. |
| **Bundling every ast-grep grammar** | All ~25 languages would bloat the binary for long-tail parsers; a curated subset covers the working set, and unsupported languages still get grep + line-window chunks. |

## 13. Phases

1. **Local repos + code ingestion** — `ignore`-crate scanner for all
   folder sources, text-sniff eligibility, skip rules, code chunker with
   path prefixes, git provenance + repo map on the parent.
   *Shipped.* Field: a 48-file local repo ingests with lockfiles/binaries
   skip-listed, code verbatim (indentation intact), map headed
   `> remote · main @ sha · date`.
2. **Remote** — URL grammar, host probe + memory, shallow/sparse clone,
   ls-remote resync, cache management, MCP + deep-link routing.
   *Shipped.* Field: repo-home → one README source
   (`BurntSushi/ripgrep`, no clone of the whole repo); blob → one `code`
   source; `/tree/main/docs` → git parent with `> Scope: docs/`; clone
   URL → whole-repo parent. Cadence setting (15 m/hourly/6 h/daily/off);
   auto-sync exercised end-to-end pending a real upstream push.
3. **Repository tier** — *Shipped except promote/demote.* Tiering
   (docs-mode `lancedb/lancedb`: 232 prose children, zero code listed);
   ripgrep RRF leg in plain chat (trace `grep: 3` fused; the model cited
   a grep window naming the defining file); ast-grep symbol outlines in
   the map (`config.rs — get_config_path, load_config … (+133)`); the
   include ladder inline in the add modal; hover-caret disclosure for
   parents (row click opens the reader, the icon toggles); **the repo
   reader** — DocProperties, in-place file tree with tier dots, map as
   repo home, Markdown for prose and Shiki (css-variables theme riding
   the app tokens, all 21 schemes) for code, Embedded/Search-only badge.
   The tree is a small hand-rolled component rather than `@pierre/trees`
   — no beta dependency for ~100 lines of list; swap stays open if its
   git-status annotations earn their keep later. Reader v2 (2026-07-20):
   independently scrolling tree and file panes at full width;
   expand/collapse-all; Finder-style breadcrumbs with per-segment
   directory menus; README rendered atop the Overview (repo description
   stays un-fetched — it would be the feature's first GitHub API call);
   a line-numbers toggle (CSS counters over Shiki's line spans, numbers
   excluded from copy); select-code-to-chat with the same Explain/Ask
   actions prose sources get; a parent-jump link on child sources; and a
   policy fix — repository-tier scans skip asset images outside `docs/`
   (skip-listed in the map; research folders and docs diagrams keep
   their OCR). Git provenance rows (Origin, Ref) joined DocProperties,
   parsed back out of the content header — provenance stayed content,
   not schema, to the end. **Promote/demote shipped**: the reader's tier
   pill toggles it, the choice persists in app data
   (`embed_overrides/<parent-id>.json` — never inside a user's repo) and
   outranks the tier rule on every rescan. **Agent tools shipped** as
   MCP tools — `grep_sources` (raw regex over the working trees,
   ripgrep's engine) and `ast_search` (structural patterns like
   `$X.unwrap()`, compiled per language, matched across the bundled
   grammars) — so Claude Code and friends search repos through Alchemy
   without cloning anything themselves. The in-app agent keeps the fused
   retrieval leg instead of tool-calling: small local models are exactly
   where agentic tool loops degrade, and MCP serves the capable ones.
4. **Later, evidence-gated** — symbol-boundary chunking; nested outline
   granularity; FTS identifier tokenization; diff-on-resync view with
   `@pierre/diffs`; GitLab web grammar; notes→repo (OKF) as its own RFC.
   Along the way a LanceDB scan bug surfaced: an OR predicate missed
   freshly-`update()`d rows, so `all_folder_sources` runs two single-arm
   queries (db.rs) — treat OR-over-updated-rows as suspect generally.

## Explicitly skipped

- **Issues, PRs, wikis, releases** — API objects, not git. A future GitHub
  connector could add them; this RFC is about repositories.
- **Write-back** — decided out for v1; see §10.
- **Submodules** — not recursed; listed in the repo map so their absence
  is visible.
- **Crawling** — one repo per user intent; no org sweeps, no link-following.
- **Token storage / auth UI** — the user's git config is the auth story;
  building a keychain surface for this would be net-new risk for zero v1
  value.
- **Windows/Linux parity** — macOS-first, like the rest of the app.

No open questions remain, and as of 2026-07-20 every phase-1–3 item above
is shipped and field-verified; §13's later-bucket (symbol-boundary
chunking, diff-on-resync, GitLab web grammar, notes→repo) is the whole
backlog. The grammar-set binary cost is recorded in §5; the release-build
delta gets confirmed at the next release.
