# Repository Guidelines

## Project Structure & Module Organization

Alchemy is a local-first research notebook built with React, Vite, and Tauri.

- `src/` contains the TypeScript frontend: `components/` for views and UI, `lib/` for API, state, themes, and shared types, and `assets/` for bundled assets.
- `src-tauri/src/` contains the Rust backend. Keep Tauri commands in `commands.rs` or `commands/`; organize domain logic in focused modules such as `rag.rs`, `ingest.rs`, `mcp/`, and `inference/`.
- `src-tauri/src/tests.rs` holds the Ollama-backed integration test; `src-tauri/evals/` contains retrieval evaluation fixtures.
- `docs/` stores RFCs and product documentation. Read `DESIGN.md` before making UI changes and `RELEASE.md` before release work.

## Build, Test, and Development Commands

```bash
pnpm install                 # install frontend dependencies and build sidecars
pnpm tauri dev               # run the desktop app in development
pnpm build                   # TypeScript typecheck and production web build
cd src-tauri && cargo test   # run Rust tests (Ollama integration skips if unavailable)
cd src-tauri && cargo fmt -- --check
cd src-tauri && cargo clippy --all-targets -- -D warnings
```

Use Node with pnpm, stable Rust, and `protoc` (`brew install protobuf`). The first Tauri build may take longer while LanceDB compiles.

## Coding Style & Naming Conventions

Use 2-space indentation in TypeScript and `cargo fmt` for Rust. Prefer typed interfaces and explicit error handling over `any`. Name React components in `PascalCase` (`StudioPanel.tsx`), hooks with `use` (`useHomeActivity.ts`), and general TypeScript modules in `camelCase`. Keep Rust modules lowercase with focused responsibilities. Use theme-backed Tailwind semantic tokens; do not hard-code colors or weaken keyboard focus behavior.

**Never `eprintln!` or `println!` in shipping Rust code — use `crate::note!`.** Both macros unwrap the write and panic with "failed printing to stderr" when it fails. A bundled Mac app inherits whatever stderr the launcher left behind, and when a `pnpm tauri dev` parent terminal exits it becomes a broken pipe, so the next print panics from inside whatever thread or completion block ran it. That has already aborted the app: a Spotlight completion block printing its result took the whole process down with SIGABRT. `note!` writes through `writeln!` and drops the error. The eval and test modules still use `eprintln!` and that is fine — they run under `cargo test`, not in a bundle.

## Diagnostics

`src-tauri/src/diagnostics.rs` (see `docs/RFC-diagnostics.md`) is where failures go. When adding code that can fail:

- A failure a user will notice, in a place they cannot see — a background sweep, a server that could not bind, a completion handler — calls `crate::diagnostics::error("kind", message)`. Do not leave it as a print.
- A failure that leaves the app unusable records at `Level::Fatal`, which raises the front-end restart screen. Reserve it for that: a poisoned lock, repeated panics, a startup failure. An operation that failed is `error`, not `fatal`.
- Front-end code reports through `src/lib/diagnostics.ts`. Individual IPC calls need no handling — `api.ts`'s `run()` already logs every failure with its command name.
- Recording must never fail loudly, and a flood must never become the log. Both rules are enforced inside `diagnostics.rs`; do not route around them with a direct file write.

To read what has gone wrong: `tail ~/Library/Logs/com.thrashr888.alchemy/alchemy.log`, the `recent_errors` MCP tool, or `log stream --predicate 'subsystem == "com.thrashr888.alchemy"'`.

## Testing Guidelines

Add or update Rust tests with behavior changes; place unit tests near the relevant module or in `src-tauri/src/tests.rs` when they exercise the full data path. Run all four commands above before opening a PR. The CI workflow runs the frontend build plus Rust format, Clippy, and tests on every pull request.

## Commit & Pull Request Guidelines

Recent commits use short, imperative summaries such as `Split mcp.rs into per-domain tool modules`. Keep each commit narrowly scoped. PRs should explain user-facing behavior and implementation constraints, link the issue when applicable, and include screenshots or recordings for UI changes. Do not mix release, generated assets, or unrelated local edits into a feature PR.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
