# Repository Guidelines

**Alchemy** — a local-first, macOS-focused research notebook inspired by NotebookLM (Tauri 2 + React 19 front-end, Rust backend, LanceDB embedded storage). Import sources, chat grounded in citations, generate documents; everything runs on-device by default. Package name is `alchemy`; the directory name `notebooklm-local` is historical.

## Project Structure & Module Organization

- `src/` contains the TypeScript frontend: `components/` for views and UI, `lib/` for API, state, themes, and shared types, and `assets/` for bundled assets.
- `src-tauri/src/` contains the Rust backend. Keep Tauri commands in `commands.rs` or `commands/`; organize domain logic in focused modules such as `rag.rs`, `ingest.rs`, `mcp/`, and `inference/`.
- `src-tauri/src/tests.rs` holds the Ollama-backed integration test; `src-tauri/evals/` contains retrieval evaluation fixtures.
- `docs/` stores RFCs and product documentation. Read `DESIGN.md` before making UI changes and `RELEASE.md` before release work.

## Product Invariant: User Sovereignty

Alchemy assists the user's judgment; it must not replace that judgment or
silently expand its authority. Keep sources and generated work inspectable,
attributed, portable, and recoverable. Automatic work inside a scope the user
chose may be default-on, but it must be bounded, visible, reversible, and
stoppable. Expanding beyond that scope, making a destructive change, or acting
outside Alchemy requires explicit user intent. Prefer proposals when machine
judgment would change the user's corpus. Access to a notebook must never depend
on a particular model, provider, or agent.

## Build, Test, and Development Commands

```bash
pnpm install            # postinstall fetches PDFium + builds the Swift fm sidecar
pnpm tauri dev          # run the full app
pnpm dev                # Vite front-end only
pnpm build              # tsc typecheck + vite build (this is the frontend "lint")
```

Use Node with pnpm, stable Rust, and `protoc` (`brew install protobuf`). The first Tauri build may take longer while LanceDB compiles.

Rust (run from `src-tauri/`) — CI enforces all three, run before committing:

```bash
cargo fmt -- --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests and evals:

```bash
cargo test --lib <name> -- --nocapture           # single test
cargo test --lib evals -- --ignored --nocapture  # distill/rerank evals — need live Ollama
```

**Slow work is opt-in.** A plain `cargo test` runs the 367 correctness tests
and nothing else: ~12s, no model calls, no corpus embedding. Two env flags buy
back the rest.

| flag | what it turns on | cost |
| --- | --- | --- |
| `ALCHEMY_EVALS=1` | the corpus evals (`evals::`, `retrieval_eval::`) — fixture corpora embedded through the built-in embedder | +23s, CPU-bound |
| `ALCHEMY_OLLAMA_TESTS=1` | anything that calls a live model (`rag_round_trip`, the LLM half of `eval_deep_rerank`) | model-speed |

```bash
ALCHEMY_EVALS=1 cargo test --lib -- --nocapture                         # retrieval quality
ALCHEMY_OLLAMA_TESTS=1 cargo test --lib rag_round_trip -- --nocapture   # e2e data path
```

Both default off because both used to fire on their own: the evals on every
run, and the Ollama tests whenever the port happened to answer — which on a
developer machine is always. Reachability isn't consent, and measurement
isn't correctness. CI sets `ALCHEMY_EVALS=1` (see `.github/workflows/ci.yml`),
so the retrieval numbers are still watched where it matters.

To exercise anything the OS has to know about — the `alchemy://` scheme,
file associations, the Dock menu, Services — build a real bundle, not
`--no-bundle`:

```bash
pnpm tauri build --debug --bundles app    # -> target/debug/bundle/macos/Alchemy.app
```

A bare executable has no `Info.plist`, so those integrations are never
registered and silently do nothing. Set `APPLE_SIGNING_IDENTITY` (a
`Developer ID Application: ...` name) in the **shell environment** first, or
macOS re-prompts for file access on every rebuild: privacy permissions are
keyed on the signing identity, and an ad-hoc bundle draws a fresh random one
each build. The Tauri CLI reads the process environment and does not load
`.env`. Never commit an identity — it belongs in the developer's env.

Releases go through `scripts/release.sh` (see `RELEASE.md`). pnpm 11 quirks (`allowBuilds`, `verifyDepsBeforeRun: false`) are deliberate — don't "fix" `pnpm-workspace.yaml`.

## Architecture

`docs/ARCHITECTURE.md` is the authoritative deep-dive; `docs/RFC-*.md` documents each major feature's design (this repo is RFC-driven — write/update the RFC before implementing complex features). The short version:

**Data flow:** import → extract (`ingest.rs`, per-filetype) → structure-aware chunking → embed → LanceDB `chunks` table (vector + BM25 FTS). Chat embeds the question, runs hybrid search (vector + BM25 merged by reciprocal rank fusion), builds a numbered-excerpt grounded prompt (`rag.rs`), streams the answer as `chat://token` events, persists the turn with citations. Every retrieval appends a trace line to `<app-data>/traces/retrieval.jsonl`.

**Backend (`src-tauri/src`):**
- `db.rs` — one embedded LanceDB, one table per entity, filtered by `notebook_id` (not relational). `chunks`/`routes` tables are created lazily once embedding dimensionality is known. Updates/deletes use Lance predicate strings with single-quote escaping.
- `commands.rs` + `commands/` — the `#[tauri::command]` IPC surface. Errors are flattened to strings to cross IPC; serde structs in `models.rs` are `camelCase` for the TS side.
- `inference/` — provider abstraction: Ollama, OpenAI-compatible gateways, Apple Foundation Models (via the Swift sidecar in `sidecar/alchemy-fm`), headless agent CLIs (Claude Code, Codex, …), and a built-in local embedder. Model roles (chat/small/embed) route through `AiConfig`.
- `router.rs` / `gist.rs` — semantic router (per-source embedded routes, self-healing diff) and per-source distilled gists; both power "ask everything" meta-chat across notebooks.
- `mcp/` — embedded MCP server (rmcp, streamable HTTP on `127.0.0.1:41414`) exposing notebook/source/note CRUD + hybrid search to agents. Same process owns LanceDB, so no cross-process write conflicts; mutations emit `mcp://changed`. `connectors.rs` registers it (plus `skills/alchemy`) with installed agent clients. **Dev builds bind `mcp_port + 1` (41415) and write their own discovery file (`mcp.dev.json`)** so a dev instance and the installed app never collide on the port or on `mcp.json` — agent configs written by Connect point at the configured port (the installed app); to aim an agent or the CLI at a dev build, temporarily edit its config to 41415 (CLI: `ALCHEMY_MCP_DISCOVERY="$HOME/Library/Application Support/com.thrashr888.alchemy/mcp.dev.json"`).
- `integrations.rs` / `mac.rs` — Apple Notes/Reminders/Calendar/Stocks sources via the `cider` CLI (Paul's repo — fix bugs upstream there, don't work around them here).
- `diagnostics.rs` — error and crash capture (docs/RFC-diagnostics.md). Panic hook, JSONL log at `~/Library/Logs/com.thrashr888.alchemy/alchemy.log`, an `os_log` mirror on the `com.thrashr888.alchemy` subsystem, and `recent_errors` over IPC + MCP. **Print with `crate::note!`, never `eprintln!`** — `eprintln!` panics on a broken stderr and has aborted the app in the field. Anything that leaves the app unusable records at `fatal`, which raises the front-end's restart screen.

**Frontend (`src`):** `lib/types.ts` mirrors the Rust models, `lib/api.ts` is a typed `invoke` wrapper, `lib/store.ts` is the Zustand store (optimistic messages, streaming buffer). Components subscribe to Tauri events for streaming and cross-window refresh. In multi-window scenarios, JS `Any` event listeners are NOT filtered by target — self-filter by payload label.

## Design system

`DESIGN.md` is the source of truth for all visual/interaction decisions. Key rules: 37 themes (dark + light) driven by semantic CSS tokens in `src/index.css` and `src/lib/themes.ts` — **never hardcode a hex in a component**. Linear-inspired: hairline borders instead of tonal fills, color only when it means something, no colored left-border accents. Shared primitives live in `src/components/ui.tsx`.

**Shaders.** The backdrop (`src/components/DitherBackground.tsx`, one GLSL ES 1.0
program with 20 theme-driven modes) and the Activity tile washes
(`src/components/settings/TileShader.tsx`) are WebGL1 on purpose — WKWebView
everywhere, no WebGPU. Never edit a `FRAG` blind: shader quality is aesthetic,
not just correct math, and one GLSL error kills the backdrop for every theme.
Run the harness, look at the pixels next to the reference, iterate:

```bash
python3 scripts/shader-harness.py --serve   # http://127.0.0.1:8791/ — contact sheet of every mode
```

Also a `shaders` entry in `.claude/launch.json` for the Browser pane. The page
sets `<html data-status="ok|fail">` and prints compile logs, so it doubles as
the compile gate. See `.claude/skills/shaders/SKILL.md` for the workflow.

`WRITING.md` is the source of truth for all user-facing words (website, release notes, in-app copy). Register scales with the surface: Apple-terse headlines, Google-plain body prose, HashiCorp-sober methodology, Vercel-clipped table cells. Translate internal vocabulary before publishing, claim only measured numbers, and run the tell check before shipping copy.

## Coding Style & Naming Conventions

Use 2-space indentation in TypeScript and `cargo fmt` for Rust. Prefer typed interfaces and explicit error handling over `any`. Name React components in `PascalCase` (`StudioPanel.tsx`), hooks with `use` (`useHomeActivity.ts`), and general TypeScript modules in `camelCase`. Keep Rust modules lowercase with focused responsibilities. Use theme-backed Tailwind semantic tokens; do not hard-code colors or weaken keyboard focus behavior.

**Never `eprintln!` or `println!` in shipping Rust code — use `crate::note!`.** Both macros unwrap the write and panic with "failed printing to stderr" when it fails. A bundled Mac app inherits whatever stderr the launcher left behind, and when a `pnpm tauri dev` parent terminal exits it becomes a broken pipe, so the next print panics from inside whatever thread or completion block ran it. That has already aborted the app: a Spotlight completion block printing its result took the whole process down with SIGABRT. `note!` writes through `writeln!` and drops the error. The eval and test modules still use `eprintln!` and that is fine — they run under `cargo test`, not in a bundle.

## Conventions

- Intelligent behavior ships default-ON; settings toggles are cost control, not opt-in gates.
- New user-facing features should be agent-reachable too (MCP tools / commands), not UI-only.
- Keep test notebooks/fixtures after verifying — they double as examples.

## Diagnostics

`src-tauri/src/diagnostics.rs` (see `docs/RFC-diagnostics.md`) is where failures go. When adding code that can fail:

- A failure a user will notice, in a place they cannot see — a background sweep, a server that could not bind, a completion handler — calls `crate::diagnostics::error("kind", message)`. Do not leave it as a print.
- A failure that leaves the app unusable records at `Level::Fatal`, which raises the front-end restart screen. Reserve it for that: a poisoned lock, repeated panics, a startup failure. An operation that failed is `error`, not `fatal`.
- Front-end code reports through `src/lib/diagnostics.ts`. Individual IPC calls need no handling — `api.ts`'s `run()` already logs every failure with its command name.
- Recording must never fail loudly, and a flood must never become the log. Both rules are enforced inside `diagnostics.rs`; do not route around them with a direct file write.

To read what has gone wrong: `tail ~/Library/Logs/com.thrashr888.alchemy/alchemy.log`, the `recent_errors` MCP tool, or `log stream --predicate 'subsystem == "com.thrashr888.alchemy"'`.

## Testing Guidelines

Add or update Rust tests with behavior changes; place unit tests near the relevant module or in `src-tauri/src/tests.rs` when they exercise the full data path. Run the frontend build and the three Rust gates above before opening a PR. The CI workflow runs the frontend build plus Rust format, Clippy, and tests on every pull request.

## Commit & Pull Request Guidelines

Recent commits use short, imperative summaries such as `Split mcp.rs into per-domain tool modules`. Keep each commit narrowly scoped. PRs should explain user-facing behavior and implementation constraints, link the issue when applicable, and include screenshots or recordings for UI changes. Do not mix release, generated assets, or unrelated local edits into a feature PR.

## Task tracking

Work items live in Linear: the "Alchemy" project on the "Paul Thrasher"
team (issue keys `PAUL-n`), read and written through the Linear MCP
connector. There is no in-repo issue tracker. Keep follow-ups there, not
in TODO lists or markdown files. The Apple Reminders "Alchemy" list is
retired; its open items were imported into Linear on 2026-10-04.

## Session completion

When ending a work session: run the quality gates if code changed, commit,
`git pull --rebase`, `git push`, and confirm `git status` reports "up to
date with origin". Work is not complete until the push succeeds; never
stop short of it, and never hand the push back to the user.
