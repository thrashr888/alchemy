# RFC: Desktop AI apps — hand a notebook to the app you already use

Status: phase 1 (the handoff) built 2026-09-16 on `cld/wife-feedback-2`;
phase 2 (the Claude Desktop extension) built 2026-09-17 on `cld/wf-mcpb`;
phase 3 proposed, awaiting review.

## Summary

Alchemy's second user has Claude Desktop, ChatGPT, and GitHub Copilot on
her Mac — and no Ollama, no Apple Intelligence, no API key, no agent CLI.
First run offered her none of the things she has. Two facts shape what we
can do about it: the desktop apps expose no local API a notebook could
answer through, and Alchemy already runs an MCP server every one of them
could, in principle, connect to. So the design is a handoff in two grades:
carry the notebook to the app as a prompt (works everywhere, today), and
connect the app to Alchemy's MCP server where the app allows it (Claude
Desktop first), so the app can search the notebook instead of reading a
list.

## Phase 1 — "Open In …" (built)

A notebook's ⋯ menu (Home and the workspace) gains **Open In** with one row
per installed desktop app: Claude, ChatGPT, GitHub Copilot (presence is
`/Applications/<Name>.app` or `~/Applications/…`; `desktop_apps`). Choosing
one builds a prompt (`handoff_prompt`), copies it to the clipboard, brings
the app to the front (`open -a`), and toasts "Prompt copied — paste it into
Claude to pick up where you are."

The prompt names the notebook, lists up to 25 sources as
`title — origin: gist…` (the per-source gist the distiller already wrote,
so the app gets substance rather than titles), folds the rest into a
count, and then says: if you have the Alchemy connector, use it — here is
the notebook id, `search` returns cited passages and `get_source` the full
text; if you don't, work from the list and ask for pastes. It ends with
"Claude, here's my question: " so the person's cursor lands where they
type.

Why the clipboard and not a URL scheme: none of the three apps documents a
scheme that prefills a prompt, and a scheme that half-works is worse than a
paste that always does. Why not automation: driving another app's UI is
exactly the "acting outside Alchemy without explicit intent" the product
invariant forbids, and macOS would prompt for it anyway.

## Phase 2 — connect Claude Desktop to the MCP server (as built)

Claude Desktop cannot be pointed at a URL, but it installs Desktop Extension
bundles (`.mcpb`) through its own sheet and runs their servers as stdio
children under its own bundled Node — so the Mac needs no Node of its own.
Alchemy's server is streamable HTTP behind a private bearer token
(`mcp.json`). The bundle is the bridge, and Alchemy writes it.

The `npx mcp-remote` fallback from the proposal was dropped: it needs a Node
on the PATH, which is exactly the machine this phase exists for not having.

**The bundle.** `crate::mcpb` zips two files:

- `manifest.json` — `manifest_version: "0.3"` (plus `dxt_version: "0.1"`, so
  readers from the Desktop Extension era still parse it), `name`/`display_name`
  `Alchemy`, `version` = the app's `CARGO_PKG_VERSION`, license MPL-2.0 to
  match the repo, `server.type: "node"` running
  `node ${__dirname}/server/index.mjs`, and `compatibility` of
  `platforms: ["darwin"]` + `runtimes: { node: ">=18.0.0" }`. No
  `claude_desktop` version floor: there is no measured one to name, and an
  invented number would refuse installs that would have worked.
- `server/index.mjs` — the proxy (`skills/alchemy-mcpb/server/`), embedded in
  the binary with `include_str!` rather than shipped as a Tauri resource, so
  every build can write it whether or not it was bundled.

`.mjs`, not `.js`: the extracted bundle has no `package.json`, so a bare
`.js` would be read as CommonJS and the proxy's imports would not load.

The manifest's `tools` list is read off the running server's own routers
(`crate::mcp::tool_catalog`), sorted by name — a hand-kept list would start
lying one release after someone added a tool.

The bundle lands at
`~/Library/Application Support/com.thrashr888.alchemy/connectors/alchemy.mcpb`,
overwritten on every Connect so it always carries this build's version and
this build's proxy.

**The proxy.** Node 18+ standard library only, no dependencies to vendor or
audit inside a file the app writes on the user's machine. It reads
newline-delimited JSON-RPC from stdin, POSTs each message to the endpoint
with `content-type: application/json`, `accept: application/json,
text/event-stream` and the bearer token, and writes every JSON-RPC message
it gets back to stdout, one per line. A reply is either plain JSON or an SSE
body whose `data:` lines each carry one message; a notification gets 202 and
no body, and produces no output. stdout belongs to the protocol — everything
human goes to stderr.

The `initialize` exchange is gated: nothing else goes out until its reply
lands, because that reply's `mcp-session-id` header is what the rest of the
conversation carries. After that messages are independent, so a slow search
cannot hold up a cancellation.

**Nothing secret is in the bundle.** The proxy reads the port and token out
of `mcp.json` at runtime, and re-reads them on a 401 and retries once — so a
relaunch that rotates the token or moves the port costs a round trip instead
of the rest of the conversation. When the socket is refused it answers the
pending request with a JSON-RPC error saying "Alchemy isn't running — open
it and try again", rather than leaving Claude waiting; a notification, which
may not be answered, only reaches the log.

A Node test (`pnpm test:mcpb`, wired into CI) drives the proxy as a child
process against a throwaway HTTP server: JSON and SSE replies, the session
id, a silent notification, a 401 that forces the token re-read, and both
offline paths.

**The connector row.** "Claude Desktop" joins the CLIs in Settings → Agents
under a new `Strategy::Mcpb`. `installed` is `/Applications/Claude.app` (or
`~/Applications`); Connect writes the bundle and `open`s it, which raises
Claude Desktop's install sheet. We never write its config — RFC-mcp-server
rejected auto-editing a client's config, and an app with an install sheet is
the case that rule was waiting for.

`configured` is read from
`~/Library/Application Support/Claude/extensions-installations.json`: an
entry whose `manifest.name` is ours. Claude Desktop mints the extension id
itself (`local.dxt.<author>.<name>`), so the name is the only stable thing
to match on. Because that is also `strategy_present`, the launch-time
connector refresh can never re-open the install sheet behind the user's
back: an installed extension is already current, and an uninstalled one
means the target is skipped.

The row needs no UI special-casing — it renders from `ConnectorStatus` like
every other. One field is new: `connectNote`, because Connect here does not
finish the job, and the old toast would have claimed a skill install that
never happened. It says: "Claude Desktop will ask to install the Alchemy
extension. After that, ask Claude about any notebook."

**Known edge.** An app upgrade leaves the installed extension at the old
version. The proxy is version-independent (it discovers everything at
runtime), so it keeps working; re-clicking Connect re-installs the current
one. Nothing re-opens the sheet on its own.

ChatGPT's connectors require a public HTTPS MCP endpoint (developer mode);
a local server does not qualify without a tunnel, which is out of scope.
GitHub Copilot's desktop app has no connector surface we can target. Both
keep phase 1's handoff.

## Phase 3 — first run (proposed)

The first-run stage gains a fourth kind of door under "Already on this Mac":
**Claude Desktop** (and ChatGPT / GitHub Copilot when present). Choosing it
does not pretend Alchemy can answer through the app. It says so plainly —
"Alchemy will index your sources on this Mac; answers happen in Claude
Desktop" — connects Claude Desktop (phase 2) when possible, and turns the
in-app chat's empty state into the same **Open In** handoff. The built-in
embedder still indexes, Grow and the Registry still work, and the person is
never told to install Ollama. Whether the in-app chat should offer a
"paste your key" path at that moment, or stay quiet, is the open question
for review.

## Open questions

1. Phase 3's empty chat: handoff only, or also a one-line key prompt?
2. Should the handoff prompt include the person's profile line (the same
   one woven into system prompts) so the app's tone matches?
3. `.mcpb` signing: Claude Desktop warns on unsigned bundles. Shipped
   unsigned — the bundle is written by the app on this Mac, seconds before
   the sheet opens, and the warning is the user's own confirmation. Revisit
   if the warning reads as scarier than the install is.

## Non-goals

Driving any app's UI; a tunnel for ChatGPT; a fourth chat provider that
pretends to be a desktop app.
