# Architecture

This document explains how NotebookLM Local is put together and why.

## Goals & decisions

| Decision | Choice | Why |
| --- | --- | --- |
| Shell | **Tauri 2** | Native desktop, tiny binaries, Rust backend with a web UI. |
| AI | **Ollama (local)** | Fully offline & private. Abstracted so a cloud/MLX provider can slot in later. |
| Storage | **LanceDB** | Embedded, Rust-native, vector search + columnar storage in one engine — no server. |
| UI | **React + Tailwind v4** | Fast iteration; themed into a Linear-grade dark aesthetic. |
| Scope | Sources, grounded chat, artifacts, notes | Audio Overview / slideshows intentionally excluded. |

## Backend modules (`src-tauri/src`)

### `models.rs`
Serde structs shared across the IPC boundary (`camelCase` for the TS side):
`Notebook`, `Source`, `Chunk`, `Citation`, `Message`, `Note`.

### `db.rs` — LanceDB layer
One embedded Lance database, one table per entity. We filter by `notebook_id`
instead of joining (LanceDB is not relational).

| Table | Key columns |
| --- | --- |
| `notebooks` | id, title, created_at, updated_at |
| `sources` | id, notebook_id, title, source_type, content, char_count, chunk_count, created_at |
| `chunks` | id, notebook_id, source_id, ordinal, text, **vector: FixedSizeList\<Float32, dim\>** |
| `messages` | id, notebook_id, role, content, citations (JSON), created_at |
| `notes` | id, notebook_id, title, content, kind, created_at, updated_at |

The `chunks` table is created **lazily** on first ingest, once the embedding
dimensionality is known from the model — so swapping embedding models with
different dimensions doesn't require a hardcoded constant.

Reads pull Arrow `RecordBatch`es and downcast columns; writes build a one-row
(or batch) `RecordBatch` and `add` it. Updates/deletes use Lance predicate
strings (`id = '...'`), with single-quote escaping for user-supplied values.

### `ingest.rs` — extraction & chunking
- **PDF** via `pdf-extract`, **text/markdown** via filesystem read, **URL** via
  `reqwest` + naive tag stripping, **pasted text** directly.
- Text is normalized (whitespace collapsed, paragraphs preserved) then split into
  **~280-word windows with ~40-word overlap** — model-agnostic and good enough
  for retrieval.

### `ai/` — provider
`AiConfig` (base URL + chat/embed model) and an `Ollama` HTTP client:
- `embed(texts)` → batched embeddings via `/api/embed`.
- `chat(messages)` → one-shot completion (used for artifacts).
- `chat_stream(messages, on_token)` → parses Ollama's NDJSON stream and invokes a
  callback per content delta (used for chat).

The provider is deliberately narrow so a cloud/MLX implementation can replace it
behind the same surface.

### `rag.rs` — prompt assembly
- `build_chat_messages` turns retrieved citations into numbered excerpts `[1..n]`,
  prepends a strict system prompt ("answer only from the excerpts, cite with
  `[n]`"), and includes a short rolling window of prior turns.
- `artifact_spec` / `build_artifact_messages` define the Summary / FAQ / Study
  guide / Briefing / Timeline instructions and cap the corpus size.

### `commands.rs` — IPC surface
All `#[tauri::command]`s. Notable flow — **`send_message`**:
1. Persist the user turn.
2. Embed the question, vector-search chunks (k=8) scoped to the notebook.
3. Build the grounded prompt with history.
4. Stream the answer, emitting `chat://token` events to the UI.
5. Persist the assistant turn (with citations) and emit `chat://done`.

Errors are flattened to strings so they cross IPC cleanly.

### `mcp.rs` — agent access (MCP server)
An embedded MCP server (official `rmcp` SDK, streamable HTTP on
`127.0.0.1:41414`, axum-hosted) exposes notebook/source/note CRUD and hybrid
`search` as agent tools, reusing the same `AppState` helpers the commands
call — one process owns LanceDB and the embedder, so there are no
cross-process write conflicts. Mutations emit `mcp://changed` so open windows
refresh live. Requests carrying a browser `Origin` header are rejected (CSRF
guard); enable/port live in `AiConfig` (Settings → Agents), and a discovery
file is written to `<app-data>/mcp.json`. See `docs/RFC-mcp-server.md`.

### `connectors.rs` — agent client registry
One-click registration of the MCP server (plus the bundled `skills/alchemy`
SKILL.md) with installed agent clients — Claude Code, Codex, OpenCode, Gemini
CLI, Antigravity, Kiro, IBM Bob, Hermes. Each target declares detection
paths, a config strategy (JSON merge / TOML append / manual snippet), and its
skills dir; Settings → Agents renders one row per target.

## Frontend (`src`)

- `lib/types.ts` mirrors the Rust models.
- `lib/api.ts` is a typed wrapper over `invoke`.
- `lib/store.ts` is a Zustand store holding notebooks/sources/messages/notes and
  all actions (optimistic user messages, streaming buffer, artifact generation).
- `components/` — `Sidebar` (notebooks), `SourcesPanel` (import + list),
  `ChatPanel` (streamed chat + citation chips), `StudioPanel` (artifacts + notes),
  `SettingsDialog` (Ollama config + model pickers), plus a small `ui.tsx` kit.
- Streaming: `ChatPanel` subscribes to `chat://token` and appends deltas to the
  store's `streamingText`; on completion it reloads canonical messages.

## Design system

A Linear-inspired dark theme defined as CSS variables in `index.css` and exposed
to Tailwind v4 via `@theme inline`: near-black canvas (`#08090a`), hairline
borders (`rgba(255,255,255,0.07)`), indigo accent (`#5e6ad2`), Inter typography,
compact spacing, thin scrollbars.

## Data flow summary

```
import source ─► extract ─► chunk ─► embed (Ollama) ─► chunks table (vectors)
ask question ─► embed ─► vector search (notebook-scoped) ─► top-k excerpts
            ─► grounded prompt ─► stream answer + citations ─► persist + render
```
