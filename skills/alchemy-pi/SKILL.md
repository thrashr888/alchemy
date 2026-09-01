---
name: alchemy
description: Use when the user mentions Alchemy, their research notebooks, or wants sources (URLs, files, pasted text) collected, searched, or written up in a local notebook. Alchemy is their local-first research notebook app; the alchemy_* tools (from the alchemy extension) expose its notebooks, sources, notes, and hybrid search.
---

# Alchemy — the user's local research notebook

Alchemy is a local-first NotebookLM-style app. A **notebook** holds **sources**
(fetched URLs, imported files, watched folders, pasted text — auto-chunked and
embedded on device) and **notes** (markdown the user or you write). Everything
runs on the user's machine; nothing you store or search leaves it.

## The tools

The `alchemy` extension registers the core tools natively:
`alchemy_list_notebooks`, `alchemy_search`, `alchemy_ask_everything`,
`alchemy_list_sources`, `alchemy_get_source`, `alchemy_add_source`,
`alchemy_create_note`. The app exposes ~46 tools in total (notes CRUD,
ledger, registry, schedules, Apple Notes/Reminders write-back, …) —
`alchemy_list_tools` lists them all with schemas, and `alchemy_call`
invokes any of them by name with a JSON arguments object.

If a call errors with "is the Alchemy app running?", the app is closed —
ask the user to open Alchemy. (No Alchemy at all? It's a free macOS app:
https://github.com/thrashr888/alchemy/releases.) If the alchemy_* tools
are missing entirely, run `/reload` to pick up the extension.

## Workflow

1. `alchemy_list_notebooks` to find the right notebook. Prefer reusing an
   existing notebook over creating near-duplicates (`create_notebook` via
   `alchemy_call` when the topic truly deserves its own).
2. `alchemy_add_source` for each URL or block of text worth keeping.
3. `alchemy_search` to ground claims before writing — it runs on a local
   embedder and is effectively free; make several small queries rather than
   one broad one. When you don't know WHICH notebook holds something, use
   `alchemy_ask_everything` — passages arrive tagged with their notebook.
   Both return raw passages; synthesize the answer yourself.
4. Write findings with `alchemy_create_note` (markdown). Cite which sources
   informed each claim by title so the user can verify.

## Filling a notebook from a list

When the user hands you a list of things to cover, the work is finding the
right page for each. Prefer each item's official page and check unsure
URLs first (a quick search, or a HEAD request) instead of guessing paths.
Read every result: a `title` like "Page Not Found", "Access Denied", or
"Attention Required", or a `charCount` of a few hundred, is not content
even when `status` says ready — delete it and try an alternate page, a
support-center article, or a `text` source you write and label as your
own summary. A two-segment path on an unfamiliar host can be mistaken for
a git repo; if an add comes back with a clone error, re-add the URL with a
`#fragment`. Add three to five at a time, then finish with an index note
mapping each item the user named to its sources and gaps. Covers, tags,
and notes on sources are yours to set (`set_source_image`,
`set_source_tags`, `set_source_note` via `alchemy_call`).

## Sharp edges

- **Duplicates are rejected, not silently merged.** Adding the same URL or
  identical content errors with the existing source's title. Treat that as
  success and move on.
- **URL imports can fail soft.** Bot-walled pages land as a source with
  `status: "error"` and a reason. Report it; don't retry the same URL
  blindly — try an alternate URL or paste the content as text instead.
- **Search returns passages, not documents.** When you need full context,
  call `alchemy_get_source` with the passage's `sourceId`.
- **Notes are shared with the user.** `update_note` (via `alchemy_call`)
  replaces the whole note — `get_note` first and preserve the user's edits.
  Never delete notebooks, notes, or sources the user didn't explicitly ask
  to remove.
- The user sees changes live in the app as you work — no need to tell them
  to refresh.
