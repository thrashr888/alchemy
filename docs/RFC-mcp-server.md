# RFC: Agent access — embedded MCP server + skill

## Summary

Give AI agents (Claude Code, Cursor, Codex) first-class access to Alchemy:
an **MCP server embedded in the running app**, exposing notebooks, sources,
notes, and hybrid search as tools, plus a shippable **agent skill** that
teaches the workflow. Anything a user can do in the UI, an agent can do
through MCP — create notebooks, ingest sources, search the corpus, write
notes — while the UI stays live via change events.

Prior art: Inkeep's OpenKnowledge ships exactly this shape (per-project MCP
server + installable skills) and it's the right call. Alchemy's version is
simpler — one app, one data dir, one server — and keeps its differentiator:
retrieval runs on the local embedder, so agent search costs nothing and
nothing leaves the machine.

## Background

- All state lives in one Tauri process: `AppState` (commands.rs:28) holds
  `Arc<Db>` (LanceDB), the `Ai` runtime behind a `RwLock`, config, and stats.
  There is no HTTP surface today.
- The ingestion pipeline (`extract_any_file`, `store_extracted`,
  `friendly_title` in commands.rs) and hybrid retrieval
  (`db::search_chunks` — vector + BM25 with reciprocal-rank fusion) are
  plain `&AppState` helpers, directly reusable by non-Tauri callers.
- `export_notebook_okf` already exists, so agent/interop thinking has
  precedent in the codebase.

## Proposal

### 1. Transport: streamable-HTTP server inside the app

Embed an MCP server in the running app with the official Rust SDK
(`rmcp` 2.x, `transport-streamable-http-server` feature) mounted on a tiny
axum router, bound to `127.0.0.1:41414` (configurable). Started in `setup()`
alongside the existing state; a handle in `AppState` lets Settings stop/start
it.

Client registration is one line, shown copyable in Settings:

```
claude mcp add --transport http alchemy http://127.0.0.1:41414/mcp
```

Discovery file `<app-data>/mcp.json` (`{ "port": 41414, "pid": … }`) for
tooling that wants to find the server without hardcoding the port.

**The app must be running for agents to reach it.** That's the same contract
OpenKnowledge has (most of its tools require the Hocuspocus server) and the
right trade: one process owns LanceDB, the embedder, config, and the menu's
recents — no cross-process write conflicts, no state drift.

### 2. Tool surface (v1)

Thin wrappers over the same helpers the UI commands call:

| Tool | Maps to |
|---|---|
| `list_notebooks` | `db.list_notebooks` |
| `create_notebook(title)` | same logic as command |
| `rename_notebook(id, title)` / `delete_notebook(id)` | same |
| `list_sources(notebook_id)` | `db.list_sources` |
| `add_source(notebook_id, url \| text \| file_path, title?)` | full ingest pipeline: extract → chunk → embed → dedupe → store |
| `get_source(source_id)` | metadata + full extracted content |
| `delete_source(source_id)` | same |
| `search(notebook_id, query, k?)` | **hybrid retrieval**: `ai.embed_one` + `db.search_chunks` (vector + BM25 + RRF), returns citation-shaped passages with source id/title/snippet/distance |
| `list_notes(notebook_id)` / `get_note(id)` | `db.list_notes` / `db.get_note` |
| `create_note` / `update_note` / `delete_note` | same as commands |

Deliberately excluded from v1: `send_message` (agents synthesize from
`search` results themselves — cheaper and doesn't pollute the user's chat
history), artifact generation, audio, report schedules. Easy to add later.

`search` is the same primitive the planned ambient-connections feature needs
(`related_passages`); the Rust helper is written once and both surfaces call
it.

### 3. UI liveness

Every mutating tool emits `mcp://changed { notebookId?, scope }` to all
windows. The store binds one listener (in the existing `bindListeners`
block, store.ts:292) and refreshes the affected lists when the payload
matches the notebook it's viewing — same self-filter-by-payload pattern the
multi-window events already use. Watching notebooks fill with sources while
Claude works is the demo.

### 4. Security

- Bind `127.0.0.1` only.
- **Reject any request carrying an `Origin` header** (403). Browsers always
  send `Origin` on cross-origin requests, so this kills the
  malicious-webpage → localhost CSRF/DNS-rebinding vector; real MCP clients
  don't send one.
- No auth token in v1: a local process that could call the server could read
  the LanceDB files directly — same trust boundary. A bearer token is a
  straightforward follow-up if that changes.

### 5. Config & settings

- `AiConfig` gains `mcp_enabled: bool` (default **true**) and
  `mcp_port: u16` (default 41414). Localhost-only + origin-guarded makes
  default-on acceptable, and an MCP server that's off by default never gets
  used.
- Settings gains an **Agents** section: the enable toggle + server status at
  the top, then **one row per agent client** (below).

### 5b. Client connectors

Registering the server by hand means seven different config dialects.
`connectors.rs` holds a registry — per client: detection paths, a config
write strategy, and its skills dir — and the Agents tab renders one row per
client with a status chip (*Connected / Detected / Not installed*) and one
primary action:

- **Connect** — Alchemy writes the client's own config (careful
  read-modify-write: JSON merge that preserves everything else and refuses
  malformed files, or TOML section append) **and** installs the skill where
  the client supports SKILL.md. One click, one mental model.
- **Copy command** — for clients whose config we shouldn't machine-edit
  (YAML), the row copies their own CLI registration one-liner instead.
- Every row also has a copy icon with the manual snippet as an escape hatch.

The dialects, as researched July 2026 (they genuinely all differ):

| Client | Config (user scope) | HTTP entry shape | Skills dir |
|---|---|---|---|
| Claude Code | `~/.claude.json` | `mcpServers.<n> = {type:"http", url}` | `~/.claude/skills` |
| OpenAI Codex | `~/.codex/config.toml` | `[mcp_servers.<n>] url = "…"` | `~/.codex/skills` |
| OpenCode | `~/.config/opencode/opencode.json` | `mcp.<n> = {type:"remote", url, enabled}` | `~/.config/opencode/skills` |
| Gemini CLI | `~/.gemini/settings.json` | `mcpServers.<n> = {httpUrl}` (`url` would mean SSE) | `~/.gemini/skills` |
| Antigravity | `~/.gemini/config/mcp_config.json` + legacy `~/.gemini/antigravity/mcp_config.json` (write both) | `mcpServers.<n> = {serverUrl}` | `~/.gemini/skills` |
| Hermes Agent | `~/.hermes/config.yaml` — **manual**: `hermes mcp add alchemy --url …` | `mcp_servers.<n>.url` (YAML) | `~/.hermes/skills/research` |
| AWS Kiro | `~/.kiro/settings/mcp.json` | `mcpServers.<n> = {url}` (transport auto-negotiated) | `~/.kiro/skills` |
| IBM Bob | `~/.bob/mcp.json` + `~/.bob/mcp_settings.json` (IDE/Shell disagree; write both) | `mcpServers.<n> = {type:"streamable-http", url}` — bare `url` falls back to legacy SSE | `~/.bob/skills` |
| Factory Droid | `~/.factory/mcp.json` | `mcpServers.<n> = {type:"http", url}` | `~/.factory/skills` |
| GitHub Copilot CLI | `~/.copilot/mcp-config.json` | `mcpServers.<n> = {type:"http", url, tools:["*"]}` | `~/.copilot/skills` |
| VS Code | `~/Library/Application Support/Code/User/mcp.json` | **`servers`**`.<n> = {type:"http", url}` — not `mcpServers` | reads `~/.copilot/skills` + `~/.claude/skills` |

Cursor and Windsurf are easy follow-ups (same registry shape) once their
current formats are verified.

### 6. The skill

`skills/alchemy/SKILL.md` in-repo; **Connect** copies it into each client's
skills dir (the SKILL.md format is now the de-facto standard across Claude,
Codex, OpenCode, Gemini/Antigravity, Kiro, Bob, and Hermes). Content (short,
per skill best practice):

- **Trigger:** when the user mentions Alchemy, their notebooks, or wants
  research material collected/organized locally.
- **Workflow:** create/find notebook → `add_source` for each URL/file/text →
  `search` to ground claims → write findings as notes with `create_note`.
- **Sharp edges:** duplicate sources are rejected by content hash — treat as
  success; URL ingests can land with `status: "error"` (bot-walled pages) —
  report, don't retry blindly; `search` returns passages, not whole docs —
  call `get_source` when full text is needed.

## Rationale & alternatives considered

- **Separate stdio binary sharing the crate** — rejected for v1. Two
  processes writing one LanceDB invites optimistic-concurrency conflicts,
  the second process duplicates the embedder/AI runtime, and the app UI
  can't see changes. A thin `alchemy mcp` stdio shim that *proxies* to the
  HTTP server (and optionally launches the app) is a clean future addition.
- **Tauri plugin (tauri-plugin-mcp or similar)** — the ecosystem plugins
  target driving the *webview*; we're exposing *domain tools*. Wrong layer.
- **Auto-registering with editors** (writing `.mcp.json` / `claude mcp add`
  on the user's behalf) — rejected; editor config belongs to the user. We
  show the one-liner instead.
- **Exposing `send_message` (full RAG chat)** — deferred. The calling agent
  is itself an LLM; giving it retrieval beats giving it a second model's
  answers, and it keeps notebook chat history human-only.

## Downsides & risks

- **New dependency surface:** `rmcp` + `axum` (tower/hyper tree). Moderate
  compile-time cost; both are the standard, well-maintained choices.
- **Open localhost port by default.** Mitigated by origin rejection and
  loopback bind; documented in README. Toggle exists for the cautious.
- **Agent-driven writes race user edits** (e.g. both editing a note). V1
  policy: last-write-wins, same as two app windows today. Notes are small;
  acceptable.
- **rmcp API churn** — the SDK is young; pin the minor version.

## Implementation plan

1. `src-tauri/src/mcp.rs`: server bootstrap + tool handlers over
   `AppHandle::state::<AppState>()`; mark the needed commands.rs helpers
   `pub(crate)`.
2. `mcp://changed` events + store listener refresh.
3. Settings Agents section + skill install; `skills/alchemy/SKILL.md`.
4. README + ARCHITECTURE note.

Quality gates: `cargo fmt && cargo clippy -- -D warnings && cargo test`,
plus an end-to-end pass driving the tools from a real MCP client against the
running app.
