# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

**Alchemy** — a local-first, macOS-focused research notebook inspired by NotebookLM (Tauri 2 + React 19 front-end, Rust backend, LanceDB embedded storage). Import sources, chat grounded in citations, generate documents; everything runs on-device by default. Package name is `alchemy`; the directory name `notebooklm-local` is historical.

## Commands

```bash
pnpm install            # postinstall fetches PDFium + builds the Swift fm sidecar
pnpm tauri dev          # run the full app
pnpm dev                # Vite front-end only
pnpm build              # tsc typecheck + vite build (this is the frontend "lint")
```

Rust (run from `src-tauri/`) — CI enforces all three, run before committing:

```bash
cargo fmt -- --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests and evals:

```bash
cargo test --lib <name> -- --nocapture           # single test
cargo test --lib evals -- --nocapture            # retrieval-quality eval (built-in embedder, CI-safe)
cargo test --lib evals -- --ignored --nocapture  # distill/rerank evals — need live Ollama
cargo test --lib rag_round_trip -- --nocapture   # e2e data path — no-ops if Ollama isn't running
```

Releases go through `scripts/release.sh` (see `RELEASE.md`). pnpm 11 quirks (`allowBuilds`, `verifyDepsBeforeRun: false`) are deliberate — don't "fix" `pnpm-workspace.yaml`.

## Architecture

`docs/ARCHITECTURE.md` is the authoritative deep-dive; `docs/RFC-*.md` documents each major feature's design (this repo is RFC-driven — write/update the RFC before implementing complex features). The short version:

**Data flow:** import → extract (`ingest.rs`, per-filetype) → structure-aware chunking → embed → LanceDB `chunks` table (vector + BM25 FTS). Chat embeds the question, runs hybrid search (vector + BM25 merged by reciprocal rank fusion), builds a numbered-excerpt grounded prompt (`rag.rs`), streams the answer as `chat://token` events, persists the turn with citations. Every retrieval appends a trace line to `<app-data>/traces/retrieval.jsonl`.

**Backend (`src-tauri/src`):**
- `db.rs` — one embedded LanceDB, one table per entity, filtered by `notebook_id` (not relational). `chunks`/`routes` tables are created lazily once embedding dimensionality is known. Updates/deletes use Lance predicate strings with single-quote escaping.
- `commands.rs` + `commands/` — the `#[tauri::command]` IPC surface. Errors are flattened to strings to cross IPC; serde structs in `models.rs` are `camelCase` for the TS side.
- `inference/` — provider abstraction: Ollama, OpenAI-compatible gateways, Apple Foundation Models (via the Swift sidecar in `sidecar/alchemy-fm`), headless agent CLIs (Claude Code, Codex, …), and a built-in local embedder. Model roles (chat/small/embed) route through `AiConfig`.
- `router.rs` / `gist.rs` — semantic router (per-source embedded routes, self-healing diff) and per-source distilled gists; both power "ask everything" meta-chat across notebooks.
- `mcp/` — embedded MCP server (rmcp, streamable HTTP on `127.0.0.1:41414`) exposing notebook/source/note CRUD + hybrid search to agents. Same process owns LanceDB, so no cross-process write conflicts; mutations emit `mcp://changed`. `connectors.rs` registers it (plus `skills/alchemy`) with installed agent clients. **Dev builds bind `mcp_port + 1` (41415)** so a dev instance and the installed app never collide — agent configs written by Connect point at the configured port (the installed app); to aim an agent at a dev build, temporarily edit its config to 41415.
- `integrations.rs` / `mac.rs` — Apple Notes/Reminders/Calendar/Stocks sources via the `cider` CLI (Paul's repo — fix bugs upstream there, don't work around them here).
- `diagnostics.rs` — error and crash capture (docs/RFC-diagnostics.md). Panic hook, JSONL log at `~/Library/Logs/com.thrashr888.alchemy/alchemy.log`, an `os_log` mirror on the `com.thrashr888.alchemy` subsystem, and `recent_errors` over IPC + MCP. **Print with `crate::note!`, never `eprintln!`** — `eprintln!` panics on a broken stderr and has aborted the app in the field. Anything that leaves the app unusable records at `fatal`, which raises the front-end's restart screen.

**Frontend (`src`):** `lib/types.ts` mirrors the Rust models, `lib/api.ts` is a typed `invoke` wrapper, `lib/store.ts` is the Zustand store (optimistic messages, streaming buffer). Components subscribe to Tauri events for streaming and cross-window refresh. In multi-window scenarios, JS `Any` event listeners are NOT filtered by target — self-filter by payload label.

## Design system

`DESIGN.md` is the source of truth for all visual/interaction decisions. Key rules: 23 themes (dark + light) driven by semantic CSS tokens in `src/index.css` and `src/lib/themes.ts` — **never hardcode a hex in a component**. Linear-inspired: hairline borders instead of tonal fills, color only when it means something, no colored left-border accents. Shared primitives live in `src/components/ui.tsx`.

`WRITING.md` is the source of truth for all user-facing words (website, release notes, in-app copy). Register scales with the surface: Apple-terse headlines, Google-plain body prose, HashiCorp-sober methodology, Vercel-clipped table cells. Translate internal vocabulary before publishing, claim only measured numbers, and run the tell check before shipping copy.

## Conventions

- Intelligent behavior ships default-ON; settings toggles are cost control, not opt-in gates.
- New user-facing features should be agent-reachable too (MCP tools / commands), not UI-only.
- Keep test notebooks/fixtures after verifying — they double as examples.
