# RFC: `alchemy` — a command-line client for the running app

Status: draft. Design only, nothing implemented. Companion to
[RFC-mcp-server.md](RFC-mcp-server.md), which this depends on entirely —
the CLI adds no backend surface, only a second mouth on the one that
already exists.

## Summary

Ship an `alchemy` binary that is a **thin MCP client over the app's own
embedded server**. Every subcommand is one `tools/call` against
`127.0.0.1:41414/mcp` — the same tools Claude Code and Codex already use.
No new IPC surface, no second LanceDB writer, no duplicated ingest
pipeline.

What that buys, in the order people will actually use it:

```
alchemy add ~/Downloads/report.pdf          # file in from the shell
curl -sL https://example.com/spec | alchemy add - --title Spec
alchemy search "kiln warranty"              # search from anywhere
alchemy notes --json | jq -r '.[].title'    # scriptable, pipeable
```

The convention that makes this cheap: *new user-facing features should be
agent-reachable too*. The corollary is that a good agent surface is already
a good CLI surface. Alchemy has 60+ MCP tools with hand-written
descriptions; the CLI is a `clap` front door onto them and roughly zero
new logic.

Prior art is cider — Paul's own CLI, already linked into this crate — for
flag shape, exit discipline, and the "JSON on stdout, chatter on stderr"
contract.

## What exists today

- **The server is already the product surface.** `src-tauri/src/mcp/`
  exposes notebooks, sources, hybrid search, notes, studio, registry,
  ledger, mac writes, settings, and diagnostics. Tool descriptions carry
  the sharp edges (duplicate rejection, `status: "processing"`, poll
  `get_note` after `generate`) — a CLI inherits all of it.
- **Discovery already exists.** `mcp.rs` writes `<app-data>/mcp.json`
  (`{port, url, pid}`) on every start and deletes it on stop. Nothing
  reads it yet. A CLI is exactly what it was written for.
- **Dev builds bind `port + 1`** (`effective_port`), which today means
  hand-editing an agent's config to point at 41415. A CLI that reads
  `mcp.json` gets the right port for free — and could read the *dev*
  instance's file if the data dirs ever diverge.
- **The transport is stateless** under MCP 2026-07-28 (rmcp 3 keeps
  legacy sessions only for older clients). A one-shot client can POST
  `initialize` → `tools/call` and exit without session bookkeeping.
- **`skills/alchemy-pi/` already contains a working streamable-HTTP MCP
  client in ~200 lines of dependency-free fetch**, written for pi. It is
  the proof that this client is small, and the model for the Rust one.
- **There is no Cargo workspace**: `src-tauri/Cargo.toml` is a standalone
  package. Anything new is either a workspace root (disturbing the Tauri
  build) or a second standalone crate.
- **cider's contract**, verified live: JSON to stdout always, `--pretty`
  pretty-prints it, `--envelope` wraps in `{data, ok}`, `--dry-run` on
  mutations returns `{action, dry_run: true, message, ok}` and touches
  nothing.

## Design

### 1. Transport: HTTP to the running app, discovered not guessed

Resolution order for the endpoint:

1. `--url` / `--port` flags.
2. `$ALCHEMY_MCP_URL`.
3. `<app-data>/mcp.json` — `~/Library/Application Support/com.thrashr888.alchemy/mcp.json`.
4. `http://127.0.0.1:41414/mcp` as the last-resort default.

The client sends no `Origin` header, which is what the server's
`reject_browser_origins` middleware demands, and refuses a non-loopback
`--url` unless `--allow-remote` is passed. A CLI that can be talked into
POSTing notebook contents to an arbitrary host is a data-exfiltration
primitive; loopback-only by default keeps the CLI inside the same trust
boundary the server already drew.

Long calls (`add_source` on a scanned PDF, `generate`) send a
`progressToken` in `_meta`, because `sources.rs::Heartbeat` only beats for
clients that sent one — without it a five-minute OCR import looks hung.

### 2. When the app isn't running

The app owns LanceDB, the embedder, and config. There is no offline mode
and pretending otherwise would mean a second writer, which
RFC-mcp-server.md already rejected. So there are exactly three honest
behaviors, and the CLI should do all three:

- **`--launch auto` (default).** `open -g -b com.thrashr888.alchemy`
  launches the app *without* stealing focus, then polls `mcp.json` and the
  port for up to 20 seconds. Typing `alchemy add report.pdf` and having it
  work is the entire ease-of-use claim; failing because a GUI app was
  closed would forfeit it. The app is already tray-resident (RFC-night-shift),
  so a background launch is not an intrusion.
- **`--launch never`.** For cron and CI, where silently launching a GUI
  app on someone's desktop is rude. Exits **3** with
  `alchemy: app not running (start Alchemy, or run with --launch auto)`.
- **`alchemy status`** never launches anything: prints running/not, port,
  version, pid, exits 0 or 3. This is what a script polls.

Deliberately *not* proposed: falling back to the `alchemy://` deep-link
scheme when the app is down (`open "alchemy://add?file=…"` already works
and launches the app). It works, but it is fire-and-forget — no source id,
no error, no exit code — so it would make `alchemy add` succeed loudly
while doing nothing verifiable. Deep links stay the Services/Dock path.

### 3. Command surface

Nouns follow the tool groups. The curated set is what a person types; the
escape hatch covers the rest, permanently.

| Command | MCP tool |
|---|---|
| `alchemy add <path\|url\|-> [--notebook N] [--title T]` | `add_source` (`file_path` / `url` / `text`; `-` reads stdin) |
| `alchemy search <query> [-n N] [-k 6]` | `search` |
| `alchemy ask <query>` | `ask_everything` (all notebooks) |
| `alchemy grep <pattern>` / `alchemy ast <pattern>` | `grep_sources` / `ast_search` |
| `alchemy notebooks [--archived]` | `list_notebooks` |
| `alchemy notebook new\|rename\|archive\|rm` | `create_notebook` / `rename_notebook` / `archive_notebook` / `delete_notebook` |
| `alchemy sources [-n N]` / `alchemy source <id>` | `list_sources` / `get_source` |
| `alchemy refresh <id…>` / `alchemy rm <id…>` | `refresh_source` / `delete_source` |
| `alchemy tag <id> <tags…>` / `alchemy annotate <id> <text>` | `set_source_tags` / `set_source_note` |
| `alchemy notes [-n N]` / `alchemy note <id>` | `list_notes` / `get_note` |
| `alchemy note new [--file F\|-]` / `alchemy note export <id> --format docx` | `create_note` / `export_note` |
| `alchemy generate --kind briefing [--wait]` | `generate`, then poll `get_note` |
| `alchemy commission --kind deep --prompt "…"` | `commission_run` |
| `alchemy hygiene [-n N]` | `source_hygiene` |
| `alchemy errors [--minutes 60]` | `recent_errors` |
| `alchemy doctor` | `mcp` reachability + `settings op:"setup"` |
| `alchemy tools [--schema]` | `tools/list` |
| `alchemy call <tool> --arg k=v [--json '{…}']` | any tool, verbatim |

`tools` + `call` are load-bearing, not a footnote: the server's tool list
grows every release, and a curated CLI that has to be edited to keep up
would be stale within two releases. This is the same curated-plus-escape-hatch
split the pi extension already ships (`alchemy_list_tools` /
`alchemy_call`), and it means a new MCP tool is reachable from the shell
the day it lands, with no CLI change.

Not exposed: chat. `send_message` is deliberately not an MCP tool
(RFC-mcp-server.md §"Rationale"), so `alchemy ask` returns *passages*, not
an answer, and its help text says so. That is the honest surface — and
synthesis is one pipe away for anyone who wants it.

**Notebook selection is the real ergonomics problem.** Every tool takes a
`notebook_id` UUID; nobody types a UUID. So `-n/--notebook` accepts an id,
an exact title, or an unambiguous case-insensitive prefix, resolved
client-side against a cached `list_notebooks` (cache keyed on the app pid,
so a restart invalidates it). Ambiguity is an error listing the candidates,
never a guess.

With no `-n` at all, `alchemy add` routes through **`suggest_notebook`** —
the app decides where the thing belongs and the CLI prints where it went.
That is the smart-defaults-ON convention applied to the shell: the
zero-flag form is the useful form. When `suggest_notebook` returns
`isNew: true` the CLI confirms on a TTY and requires `--yes` when piped, so
a cron job can never quietly manufacture notebooks.

### 4. Output conventions

Following cider where cider is right, diverging once and saying why.

- **stdout is data; stderr is everything else.** Progress, spinners,
  warnings, and the "filed into Home Maintenance" line all go to stderr, so
  `alchemy search … | jq` is always clean.
- **`--json`** forces machine output (compact). **`--json --pretty`**
  pretty-prints it, exactly as in cider. **`--envelope`** wraps in
  `{data, ok}` for scripts that want a uniform success flag.
- **The default is TTY-aware**: piped output is compact JSON; a terminal
  gets a human render (search hits as `source title · snippet`, lists as
  aligned columns). This is the one divergence from cider, and it is
  deliberate — cider is an agent-first extractor whose consumer is always a
  program, while the first consumer of `alchemy search "kiln warranty"` is a
  person squinting at a terminal. `--json` in a script makes the contract
  explicit rather than isatty-dependent, and the docs should use it.
- **`--dry-run`** prints the resolved tool call — tool name, arguments,
  and the notebook the name resolved to — and exits without sending it.
  Worth stating plainly: unlike cider's, this dry-run is *client-side*.
  MCP tools take no dry-run parameter, so the CLI can promise "I did not
  call the server", not "the server would have done X". It is still the
  right flag: notebook-name resolution is where the surprises live.
- **Exit codes**: `0` success, `1` the call failed (server error, import
  rejected), `2` usage (clap's default), `3` app not reachable. Empty
  results are `0` with `[]`; `alchemy search -q` gives grep semantics
  (exit 1 on no hits) for people writing conditionals.

### 5. Distribution

The binary is `alchemy`, built from a new **standalone crate at `cli/`**
(package `alchemy-cli`, `[[bin]] name = "alchemy"`), with its own
`Cargo.lock`. Deps: `clap`, `reqwest`, `serde_json`, `anyhow` — and a
hand-rolled JSON-RPC client rather than `rmcp`'s client feature, because
the whole protocol surface used is `initialize`, `tools/list`, and
`tools/call` against a stateless transport, and the pi extension already
demonstrated that shape. Seconds to compile, a few MB of binary, and
`rmcp`'s API churn stays a problem the app has and the CLI doesn't.

Options weighed:

- **A second `[[bin]]` in the app crate** — rejected. `alchemy_lib` links
  LanceDB, pdfium, model2vec, arrow, and cider; a "thin client" that drags
  that tree in and needs `libpdfium.dylib` beside it at runtime is neither
  thin nor a client.
- **A Cargo workspace rooted at the repo** — rejected for now. It would
  restructure the Tauri build, CI, and every `cargo` invocation in
  CLAUDE.md for one small crate's benefit. A standalone `cli/` costs one
  line in CI (`cargo build --release --manifest-path cli/Cargo.toml`) and
  disturbs nothing.
- **An npm-published binary or a shell script over `curl`** — rejected;
  the users of this are already installing a Mac app.

Shipping: build the CLI in `release.sh`, stage it into the bundle as
`Alchemy.app/Contents/Resources/alchemy-cli` (a Tauri `resources` entry,
alongside `binaries/alchemy-fm` — note the *file* cannot be named
`alchemy` next to `Contents/MacOS/Alchemy` on a case-insensitive volume),
codesign it in the same pass that signs the fm sidecar, and let the
existing cask link it:

```ruby
binary "#{appdir}/Alchemy.app/Contents/Resources/alchemy-cli", target: "alchemy"
```

That is the whole distribution story for `brew install --cask
thrashr888/tap/alchemy` users: no second formula, no installer step, no
`sudo` into `/usr/local/bin`. For people who installed the DMG by hand,
Settings → Agents grows an **Install CLI** row that symlinks into
`~/.local/bin` and prints the `PATH` line if it isn't there. The codesign
step is not optional: an unsigned extra Mach-O inside the bundle fails
notarization the same way the ad-hoc PDFium dylib did.

### 6. Scriptability

The examples that should be in `--help` and the README, because they are
the reason to build this:

```bash
# Everything that landed in Downloads today, filed by the app's own judgment
find ~/Downloads -mtime -1 -name '*.pdf' -exec alchemy add --yes {} \;

# A launchd agent: refresh a notebook's sources every morning
alchemy sources -n Research --json | jq -r '.[].id' | xargs alchemy refresh

# Hand slow work to the Night Shift instead of blocking the cron slot
alchemy commission -n Research --kind deep --prompt "What changed this week?"

# Post-merge hook: file release notes as a note
git log -1 --format=%B | alchemy note new -n Alchemy --title "$(git describe)" -

# Ambient capture from Raycast / Alfred / a hotkey
pbpaste | alchemy add - --title "Clipboard $(date +%F)"
```

Two honest constraints to document beside them:

- **cron implies the app is running.** `--launch never` plus `alchemy
  status` is the correct guard; the app's own scheduler (Night Shift) is
  the better home for anything that must run whether or not a person is at
  the machine, which is why `commission` is in the examples.
- **Imports serialize.** `IMPORT_GATE` admits two at a time by design, so
  `xargs -P8 alchemy add` buys nothing but a longer queue. `xargs -P2`, or
  just a loop.

### 7. Traces

`mcp/search.rs` hardcodes `"surface": "mcp"` in the retrieval trace. CLI
searches will therefore appear as `mcp` in `traces/retrieval.jsonl`, which
is *true* — the CLI is an MCP client — but flattens a distinction worth
having when reading traces later. Accept it for v1. MCP `initialize`
already carries `clientInfo.name`, so labelling traces by client is a
later refinement inside `mcp/`, invisible to the CLI.

## What v1 does not do

- **No headless or server mode.** The CLI is a client; with no app there
  is no LanceDB, no embedder, no config. Running Alchemy off macOS means
  extracting the Tauri-free half of the lib into a daemon — a real project,
  not a flag. The payoff of thin-client discipline is that when that day
  comes, **the CLI does not change**: it already speaks nothing but MCP,
  and would point at the daemon's port instead. Worth noting in the RFC so
  nobody re-litigates the client's shape for it.
- **No evals.** `ALCHEMY_EVALS=1 cargo test` needs the corpus fixtures and
  the built-in embedder in-process. Moving evals behind the CLI would mean
  first exposing an eval tool over MCP, which is a bigger question than
  this document (see RFC-judged-evals.md). Not blocked, just not v1.
- **No chat, no streaming.** No `send_message` tool to call, and one-shot
  request/response is the right shape for a CLI. `search`/`ask` return
  passages.
- **No interactive TUI.** The app is the interactive surface.
- **No writes the MCP server doesn't already permit.** If the CLI can do
  it, an agent can too — by construction, since they call the same tool.
  That is the point.

## Rationale & alternatives considered

- **A second stdio MCP shim (`alchemy mcp`) that proxies to the HTTP
  server** — RFC-mcp-server.md floated this as a future addition. It is
  orthogonal and cheap to add later as a hidden subcommand once this
  client exists, but it serves agents that can't do HTTP, not humans at a
  shell. Different problem.
- **Talking to Tauri IPC instead of MCP** — there is no IPC surface
  outside the webview, and inventing one would mean a second command
  catalog to keep in sync with `commands.rs` *and* `mcp/`. MCP is the
  already-maintained, already-documented one.
- **Generating the CLI from `tools/list` at runtime** (no curated
  subcommands, pure dynamic dispatch) — tempting, and `alchemy call`
  keeps the benefit. But `alchemy add ~/Downloads/report.pdf` reading
  better than `alchemy call add_source --arg file_path=…` *is* the
  feature. Curate the twenty that matter; dispatch the rest.
- **Shipping the CLI as its own Homebrew formula** — deferred, not
  rejected. The cask binary stanza covers everyone who has the app, which
  today is everyone who can use it. A formula becomes right the moment a
  daemon exists to point it at.

## Downsides & risks

- **A second front door on the same tools means a second place bad
  arguments arrive.** Mitigated by having no validation of its own: the
  CLI forwards and renders, and the server's existing `invalid_params`
  errors are the messages users see. Resisting the urge to add client-side
  cleverness is the discipline that keeps this thin.
- **`--launch auto` starting a GUI app from a shell will surprise
  someone.** `open -g` keeps it off-screen and the first line on stderr
  says what happened; `--launch never` and `ALCHEMY_LAUNCH=never` opt out.
- **Notebook-name resolution can be ambiguous** in a way UUIDs never are.
  Error-with-candidates rather than pick-the-first, always.
- **Version skew.** A CLI from one release calling an app from another
  will hit a missing tool. `tools/call` on an unknown name already errors
  cleanly; `alchemy status` should print both versions so the mismatch is
  visible when someone reports it.
- **Bundle weight and one more signed Mach-O** in the notarization path,
  with the known failure mode (ad-hoc signature → rejection).

## Open questions

- Does `alchemy add` with no `-n` really default to `suggest_notebook`, or
  to a fixed **Inbox** notebook? Suggestion is smarter and matches the
  app's defaults-ON convention; Inbox is predictable and never invents a
  notebook. Leaning suggestion, with `--inbox` for the other taste.
- Should the config file (`~/.config/alchemy/cli.toml`, written by
  `alchemy use <notebook>`) exist in v1, or is `$ALCHEMY_NOTEBOOK` enough?
  Env var first; a file the moment two people ask.
- Human rendering of `search` hits: snippet-per-line, or a bordered block
  with the source title? DESIGN.md governs pixels, not terminals — this
  needs its own small answer.
- Is `alchemy` the right binary name given the app binary is `Alchemy`?
  Case-insensitive filesystems make them collide inside the bundle
  (solved above by naming the staged file `alchemy-cli`), but it is worth
  confirming nothing else on a typical PATH claims `alchemy`.
- Shell completions (`clap_complete`) and a `man` page: ship in v1 or
  follow? Completions are nearly free and make the curated subcommands
  discoverable; leaning ship.
