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
app isn't running — ask the user to open Alchemy. (No Alchemy at all? It's a
free macOS app: https://github.com/thrashr888/alchemy/releases — this skill
is useless without it.) Registration is one click in Alchemy's
**Settings → Agents**; it writes the client's HTTP config and private bearer
header. Do not replace that entry with a bare URL, which will be rejected.

## Workflow

1. `list_notebooks` to find the right notebook; `create_notebook` if the topic
   deserves its own. Prefer reusing an existing notebook over creating
   near-duplicates.
2. `add_source` for each URL, file path, or block of text worth keeping.
   Ingestion extracts, titles, chunks, and embeds automatically. A list of
   URLs goes in one call as `urls` — you get one `{url, ok, source, error}`
   per entry, and one bad page never fails the rest. Check `status` (or
   `ok`) before trusting a result: a 404 page or a bot wall lands as
   `status: "error"` with the reason, not as content.
   `grow` tells you what a notebook is missing — questions it answered
   thinly, links its own sources keep pointing at, matching files on this
   Mac — each proposal ready to hand back to `add_source`.
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

## Filling a notebook from a list

When the user hands you a list of things to cover (programs, papers,
vendors, places), the work is finding the right page for each, not just
adding what comes to mind. Habits that keep the notebook clean:

- **Prefer the official page for each item** — the program's own benefits or
  membership page, not a news story about it. When you're unsure of the
  URL, check it first (`curl -sI`, or a quick web search for the current
  page) rather than guessing a path; guessed paths 404 or land on a
  homepage.
- **Read every result before moving on.** A source whose `title` says "Page
  Not Found", "Access Denied", "Attention Required", or "Just a moment", or
  whose `charCount` is a few hundred, is not content even when `status`
  says ready. Delete it (`delete_source`) and try an alternate: a different
  page on the same site, a sitemap, a support-center article, a partner's
  page that mirrors the terms. A site that fetches nothing at all
  (Cloudflare, Akamai) gets a `text` source you write from what you know,
  clearly labeled as your summary, with the items the user should confirm.
- **Two-segment paths on unfamiliar hosts** (`site.org/page/Join`) can be
  mistaken for a git owner/repo and probed as a repository. If an add
  comes back with a clone error, re-add with a `#fragment` on the URL —
  the shape parser skips it and the page fetches normally.
- **Add in small groups** (three to five at a time) and check the batch
  before the next; the importer serializes heavy work and a wall of calls
  just stretches every call's clock.
- **Finish with an index note**: one table per category mapping each item
  the user named to its source titles, what each page covers, what it
  doesn't (login-gated perks, thin marketing pages), and which URLs would
  not import and why. That note is how the user judges coverage.
- **Covers are yours to set**: `set_source_image` puts a picked image on
  the source's gallery card and reader header; `set_source_tags` and
  `set_source_note` carry the user's own labels into retrieval.
- **Keep the user's own lists in step.** If they track the same things in
  an Apple Reminders list, update it as well — through `add_reminder` when
  the list is a connected source, or the `cider` CLI otherwise — and say
  which items you added.

## Sharing notebooks

Notebooks travel as OKF bundles: the app exports a single `.okf.zip`
(File → Share Notebook as Zip…) and imports one via the home screen's
Import… button, or by dropping the file anywhere on the window. Import
re-embeds locally and skips duplicates, so re-importing is safe. If the user
asks how to share a notebook with someone (or move it to another machine),
point them at this flow.

A **bundle folder** is different from a zip: dropped on a notebook it becomes
an `okf` source that stays in sync, the way an Obsidian vault does. Its
`index.md` and `log.md` are listings, not knowledge, and never become
sources; concepts marked `status: deprecated` start deselected.

## A notebook you can edit as files

`list_notebooks` reports `okfPath` for every notebook kept on disk as an OKF
bundle. **When a notebook has an `okfPath`, the folder is the editing
surface** — `cat`, `sed`, and `git` beat any tool call here:

- A note is `notes/<slug>.md`; a source is `sources/<slug>.md`. Edit the
  markdown below the frontmatter and Alchemy picks the change up within
  seconds. Delete a file to delete the entity.
- `index.md` and `log.md` are generated. Do not hand-edit them; the next
  write regenerates the listings, and `log.md` is the bundle's history.
- Frontmatter keys Alchemy does not write (`verified`, `stale_after`, your
  own) survive every rewrite. `title:` names the concept.
- The `alchemy:` block holds what the spec has no field for — a source's
  real type and the user's tags, a note's `kind`, the notebook's colour and
  icon on the root `index.md`. Edit it only when you mean to change those.
- Only `sources/**.md` and `notes/**.md` are read. Anything else you put in
  the folder — config, scripts, a README, whatever `ok init` adds — is
  yours and is never ingested.
- The folder holds no Alchemy bookkeeping at all — that record lives outside
  it, so one folder can be shared over iCloud or Dropbox and each Mac keeps
  its own. Everything in the bundle is knowledge or listings.
- `generated.by` says who made a version: `human:<account>` for a person,
  `alchemy/<version>` for the app. Put your own name there when you edit a
  file and the attribution sticks.

`bind_notebook_okf(notebook_id, path)` starts this: an empty folder is
seeded from the notebook; a folder that already holds a bundle is imported
first, then bound. `unbind_notebook_okf(notebook_id)` stops it and leaves
every file in place. Offer binding when the user wants a notebook in git, on
another machine, or edited by hand — not by default.

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
