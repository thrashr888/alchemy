# RFC: Hosted agents — interactive sessions via ACP

Status: phases 1-2 implemented (2026-08-20). Backend in
`src-tauri/src/acp/`, UI in `src/components/AgentPane.tsx` behind the
Chat/Agent toggle in the chat panel header. Verified live in the dev app:
prompt → `alchemy_list_notebooks` tool chip → answer, with the session's
subprocess reaped on stop. Phases 3-4 (Settings row, session resume) open.

## Summary

Alchemy's agent story has two legs today: **inbound** (the embedded MCP
server — agents reach into notebooks) and **outbound one-shot** (headless
agent CLIs as inference providers in `inference/`). This RFC adds the third:
**hosting the user's own coding agent inside Alchemy** over the Agent Client
Protocol (ACP, agentclientprotocol.com) — Zed's LSP-for-agents standard,
JSON-RPC over stdio to an agent subprocess.

The shape, proven by OpenKnowledge's native integrations: spawn whichever
agent the user already has installed (Claude Code, Gemini CLI, opencode,
Codex — 30+ harnesses), so there is no separate sign-in or billing; stream
its turns, thoughts, tool calls, and permission requests into native UI; and
hand it Alchemy's own MCP endpoint in `session/new` so the agent arrives
already knowing how to search notebooks and write notes.

## Spike findings (what de-risked this)

Spike: a minimal Rust host using `agent-client-protocol` v2.0.0 (Zed's
official SDK, 3.8M downloads).

- **The historical Tauri blocker is gone.** v1 of the crate was `!Send` and
  needed a `LocalSet`; v2 is Send-bounded end to end and runs on plain
  multi-thread tokio — it dropped into a stock `#[tokio::main]` binary with
  zero workarounds. Tauri 2's runtime is exactly this.
- **Full round-trip verified** against opencode 1.18.15: initialize →
  session/new → prompt → streamed `session/update` notifications (thought
  chunks, tool calls with Pending→InProgress→Completed transitions, message
  chunks) → EndTurn.
- **MCP hand-off verified live**: `NewSessionRequest.mcp_servers` with
  `McpServer::Http { name: "alchemy", url: "http://127.0.0.1:41414/mcp" }`
  against the installed app's real server — the agent called
  `alchemy_list_notebooks` and answered correctly. HTTP is a first-class
  stable variant, gated on the agent advertising
  `mcp_capabilities.http` in initialize (opencode, Gemini, and the Claude
  Code adapter all do).
- Claude Code (via `npx @zed-industries/claude-code-acp`, nothing globally
  installed) and Gemini CLI both completed initialize + session/new through
  the crate; prompts failed only on auth unavailable in the sandboxed spike
  shell (keychain OAuth / API key). From the real app they inherit the
  user's login. Protocol behavior was identical across all three agents.

Caveats that shape the design:

1. **Wire protocol v1 only.** The crate's `unstable_protocol_v2` feature is
   exactly that; no shipping agent speaks it. Pin the crate minor.
2. **Auth is the UX work, not the protocol.** Each agent advertises auth
   methods (`claude-login`, `gemini-api-key`); v1 surfaces the agent's
   message and tells the user to log in via the agent's own CLI.
3. Dependency weight: audit what actually lands in the build; the spike's
   host pulled ~155 crates but most overlap Alchemy's existing tree
   (axum, reqwest, tokio).

## Proposal

### 1. Backend: `src-tauri/src/acp/` module

- **Agent discovery** — probe PATH at Settings-open and session-start for
  known ACP entrypoints: `opencode acp`, `claude` (spawned through the
  `claude-code-acp` adapter via npx), `gemini --experimental-acp`,
  `codex acp`. Return name, version, availability. Same spirit as the
  existing headless-CLI provider detection in `inference/` — reuse where the
  binaries overlap.
- **Session lifecycle** — one hosted session per notebook window at a time
  (v1). Spawn via the SDK's `AcpAgent` (it owns the subprocess + stdio
  transport); `session/new` carries `cwd` (the notebook's export dir or the
  user-chosen project dir) and `mcp_servers` pointing at our own live MCP
  port (dev builds: the +1 offset port — this is the one place agent config
  should follow the *running* instance, unlike Connect's static configs).
- **Streaming** — `session/update` notifications become Tauri events
  (`acp://update`), labeled with session id and notebook id per the
  multi-window self-filtering rule. Chunks: agent message, thought, tool
  call + status, plan, available commands, usage.
- **Permissions** — ACP `RequestPermissionRequest` bridges to an
  `acp://permission` event; the UI answers via a Tauri command that resolves
  the SDK's `Responder`. Default-deny on window close.

### 2. Frontend: agent mode in chat

A chat-mode toggle (alongside existing chat styles): "Agent" mode routes the
composer to the hosted session instead of the RAG pipeline. Renders streamed
turns with tool-call chips (name + status), collapsible thought sections,
and inline permission prompts. No new window; hairline-border chips per
DESIGN.md, no new colors.

Settings gains an "Agents" row under the existing MCP/Connect section:
detected agents, pick default, per-agent auth status.

### 3. What v1 does not do

- No multi-session concurrency per notebook, no session persistence across
  app restarts (ACP `session/load` exists; later).
- No terminal embedding (ACP supports it; nothing in Alchemy needs it yet).
- No editing of notebook content by the agent except through our own MCP
  tools — the agent's file-system tools operate on its `cwd`, which is not
  the LanceDB data dir.

## Phases

1. ~~Backend module + discovery + session round-trip.~~ Done.
2. ~~Chat agent mode: streaming render, permission prompts, stop button.~~
   Done.
3. Settings row, default-agent choice, auth-status surfacing.
4. Later: `session/load` resume, plans panel, terminal.

## Implementation notes (things the build taught us)

- **Discovery must not read the login-shell environment.** `load_shell_env`
  spawns `$SHELL -l -c env`, and doing that once per agent to answer "is it
  installed?" blew past the 30-second IPC timeout on first open — the picker
  rendered "no agents found" on a machine with four. Availability now probes
  binaries only (`AcpAgentKind::command`) on the blocking pool; the shell env
  is attached only to the agent actually being started (`launch`).
- **Don't swallow discovery errors.** The first version caught and dropped
  the failure, so a broken probe was indistinguishable from an empty machine
  and the timeout above stayed invisible. The pane now separates "still
  loading", "genuinely none installed", and "the probe failed, here's why".
- Session cwd is a per-notebook scratch dir under app data, never the
  LanceDB dir: the agent's own file tools operate there, and notebook
  content stays reachable only through our MCP tools.
- The pane stops its session on unmount, so no agent subprocess outlives the
  UI driving it; the SDK's `ChildGuard` reaps the process group.
- **Auth failure looks like a transport failure.** An agent that isn't signed
  in dies at `session/new` with a generic wire error ("Query closed before
  response received"). The agent does advertise its auth methods at
  `initialize`, so the message leads with those ("Claude Code couldn't open a
  session — it may need you to sign in first (Log in with Claude Code)") and
  the wire text trails, flattened to one line.
- **Session failures must not be toasts.** They auto-dismissed before the
  message could be read, which made the auth problem above invisible in
  practice. Failures render in-pane until dismissed, and carry the fix: an
  "Open Terminal: `claude`" button plus a Retry that replays the prompt that
  never got sent. Login commands come from a fixed per-agent table, and a
  test asserts each is on `commands::terminal_command_allowed` — that
  allowlist, not the table, is the security boundary. An in-app login flow is
  still phase 3.

## Smoke-test status (2026-08-20)

Exercised live against the dev app, opencode 1.18.15:

- Tool calls through the MCP hand-off (`alchemy_list_notebooks`), streamed
  chunks, thought blocks, tool chips with status transitions.
- File writes land in the per-notebook scratch cwd under app data, never the
  LanceDB dir.
- Cancel mid-turn leaves the session alive and accepting the next prompt.
- Session replacement (start twice) and two notebooks holding independent
  sessions; stopping one leaves the other running.
- Error paths: unknown agent, prompt with no session, stale permission
  answer — all return specific messages, and a failed start does not disturb
  a running session.
- No orphaned agent subprocesses after stop.

**Not yet covered, and worth knowing:**

- **The permission path has never fired.** opencode auto-allows tool use in
  ACP mode, so `acp://permission` and `PermissionPrompt` are written but
  unexercised — asking it to write a file produced no prompt. An agent with
  stricter defaults (Claude Code) is needed to validate that flow.
- **Claude Code end-to-end is unverified.** The adapter reaches `session/new`
  but this machine's `claude` OAuth is expired ("OAuth session expired and
  could not be refreshed"), which is environmental rather than an Alchemy
  bug. Its *failure* path is now well covered — the notice, the sign-in hint,
  Open Terminal, and Retry were all exercised against this exact failure —
  but a successful Claude Code turn has never run. Re-test after signing in;
  that run also validates permissions.
- Gemini and Codex are detected but never launched.
