---
name: alchemy
description: Use when the user mentions Alchemy, their research notebooks, or wants sources (URLs, files, pasted text) collected, searched, or written up in a local notebook. Alchemy is their local-first research notebook app; its MCP server exposes notebooks, sources, notes, and hybrid search.
---

# Alchemy — the user's local research notebook

Alchemy is a local-first NotebookLM-style app. A **notebook** holds **sources**
(fetched URLs, imported files, watched folders, pasted text — auto-chunked and
embedded on device) and **notes** (markdown the user or you write). Sources
can also be living Mac items the user connected (Apple Notes, Reminders
lists, Calendar windows, Stocks watchlists via `cider://` origins) — these
re-sync automatically. Everything runs on the user's machine; nothing you
store or search leaves it.

## Connecting

The MCP server runs inside the Alchemy app as streamable HTTP at
`http://127.0.0.1:41414/mcp` (default). If Alchemy tools are unavailable, the
app isn't running — ask the user to open Alchemy. Registration is one click
in Alchemy's **Settings → Agents** (it writes your client's own MCP config),
or manual, e.g. for Claude Code:

```
claude mcp add --transport http alchemy http://127.0.0.1:41414/mcp
```

## Workflow

1. `list_notebooks` to find the right notebook; `create_notebook` if the topic
   deserves its own. Prefer reusing an existing notebook over creating
   near-duplicates.
2. `add_source` for each URL, file path, or block of text worth keeping.
   Ingestion extracts, titles, chunks, and embeds automatically.
3. `search` to ground claims before writing — hybrid vector + keyword
   retrieval over the notebook's chunks. It runs on a local embedder and is
   effectively free; make several small queries rather than one broad one.
   When you don't know WHICH notebook holds something ("where did the user
   save X?"), use `ask_everything` — the same retrieval across the entire
   corpus, each passage tagged with its notebook. It returns raw passages;
   synthesize the answer yourself.
4. Write findings with `create_note` (markdown). Cite which sources informed
   each claim by title so the user can verify.
5. Mac-item write-back, when the user asks for it: `update_mac_note` replaces
   the body of an Apple Notes source (writes to the real note, then
   re-syncs); `add_reminder` appends to the Apple Reminders list behind a
   Reminders source. Both work only on sources the user already connected —
   find them in `list_sources` by a `url` starting with `cider://notes/note/`
   or `cider://reminders/list/`.

## Sharing notebooks

Notebooks travel as OKF bundles: the app exports a single `.okf.zip`
(File → Share Notebook as Zip…) and imports one — or a bundle folder — via
the home screen's Import… button, or by dropping the file anywhere on the
window. Import re-embeds locally and skips duplicates, so re-importing is
safe. If the user asks how to share a notebook with someone (or move it to
another machine), point them at this flow.

## Deep links

`alchemy://` URLs open the app from anywhere (Shortcuts, terminal `open`,
other apps): `alchemy://notebook/<id>`, `alchemy://note/<id>`, and
`alchemy://add?url=…|text=…&title=…[&notebook=<id>]`. Adds without a
`notebook` param ask the user to pick one; prefer passing ids you got from
`list_notebooks`.

## Sharp edges

- **Duplicates are rejected, not silently merged.** Adding the same URL or
  identical content errors with the existing source's title. Treat that as
  success and move on.
- **URL imports can fail soft.** Bot-walled or login-gated pages land as a
  source with `status: "error"` and a reason. Report it; don't retry the same
  URL blindly — try an alternate URL or paste the content as text instead.
- **`search` returns passages, not documents.** When you need full context,
  call `get_source` with the passage's `sourceId`.
- **Notes are shared with the user.** `update_note` replaces the whole note —
  `get_note` first, and preserve the user's edits. Never `delete_notebook` or
  delete notes/sources the user didn't explicitly ask to remove.
- **Mac write-back edits the user's real Apple Notes/Reminders.**
  `update_mac_note` replaces the entire note body — `get_source` first and
  preserve their content; the first line is the note's title, keep it there.
  Only write when the user asked for the change.
- The user sees changes live in the app as you work — no need to tell them to
  refresh.
