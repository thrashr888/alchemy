# RFC: Desktop AI apps — hand a notebook to the app you already use

Status: phase 1 (the handoff) built 2026-09-16 on `cld/wife-feedback-2`;
phases 2–3 proposed, awaiting review.

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

## Phase 2 — connect Claude Desktop to the MCP server (proposed)

Claude Desktop reads `~/Library/Application Support/Claude/claude_desktop_config.json`
and runs each `mcpServers` entry as a stdio child; it ships its own Node
runtime for Desktop Extensions (`.mcpb`), so a bundle needs no Node on the
Mac. Alchemy's server is streamable HTTP with a private bearer token
(`mcp.json`). Two ways in, in order of preference:

1. **An `.mcpb` bundle Alchemy writes and opens.** `manifest.json` plus a
   dependency-free Node script that proxies stdio JSON-RPC to
   `http://127.0.0.1:<port>/mcp` with the token from `mcp.json` (re-read on
   each start, since the token rotates per launch — the app must write the
   token where the proxy can read it, which `mcp.json` already is). Settings
   → Agents lists "Claude Desktop" beside the CLIs with the same Connect
   verb; Connect writes the bundle to app data and opens it, and Claude
   Desktop's own install sheet finishes the job. RFC-mcp-server §rationale
   rejected auto-editing editor configs; a bundle the app installs through
   its own UI keeps that line.
2. **A `mcpServers` entry running `npx mcp-remote`** — only if a Node is on
   the PATH; a fallback, shown as a copyable snippet, never written silently.

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
3. `.mcpb` signing: Claude Desktop warns on unsigned bundles; is that
   acceptable for a bundle the app itself wrote on this Mac?

## Non-goals

Driving any app's UI; a tunnel for ChatGPT; a fourth chat provider that
pretends to be a desktop app.
