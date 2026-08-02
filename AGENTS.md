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

## Testing Guidelines

Add or update Rust tests with behavior changes; place unit tests near the relevant module or in `src-tauri/src/tests.rs` when they exercise the full data path. Run all four commands above before opening a PR. The CI workflow runs the frontend build plus Rust format, Clippy, and tests on every pull request.

## Commit & Pull Request Guidelines

Recent commits use short, imperative summaries such as `Split mcp.rs into per-domain tool modules`. Keep each commit narrowly scoped. PRs should explain user-facing behavior and implementation constraints, link the issue when applicable, and include screenshots or recordings for UI changes. Do not mix release, generated assets, or unrelated local edits into a feature PR.
