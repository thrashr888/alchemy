# RFC: Source Tags & Annotations — user metadata that retrieval can actually use

Status: accepted (answers the backlog question "Should we add tags and notes per source? This may help user organization, but does it help retrieval?")

## Summary

Yes to both — but asymmetrically, and the codebase already tells us how.

**Tags help retrieval**, and three of the four integration points already exist as shipped machinery: the self-healing router re-embeds a route the moment its summary string changes, the Obsidian ingest path already folds frontmatter tags into chunk embed context, and the source manifest already carries per-source lines to the model on every chat turn. Tags are ground truth from the user, so they skip the entire confabulation-gating apparatus that machine-generated gists need.

**Per-source notes are organization-first** — unless we index them, which costs almost nothing: the prefixed-owner chunk trick (`gist:`, `note:`) extends to `snote:<source_id>` with zero schema change to the chunks table. A human-written note about a source is strictly higher-trust than a gist; it deserves at least equal standing in the index.

[RFC-document-surface.md](RFC-document-surface.md) deliberately cut tags ("needs a data model + retrieval story — not chrome; deserves its own RFC if wanted"). This is that RFC.

## What exists today

- **Sources have no descriptive metadata.** The only fields beyond ingest mechanics are `author` (machine-extracted) and `parent_id` (folders). Organization is notebook membership plus notebook color.
- **Gists** (`gist.rs`) are machine metadata already living in the retrieval index as chunk rows under `gist:<source_id>`. Forty lines of gating (`gate()`, length bounds, identifier grounding) protect retrieval from the model's guess being wrong. That gate is the price of *generated* metadata; user metadata doesn't pay it.
- **Routes** (`router.rs`) embed `"{title} — {gist}"` per source and self-heal by string-diffing the summary. Any change to the summary string re-embeds automatically — a free insertion point.
- **Obsidian frontmatter tags already work as retrieval signal**: `chunk_source` builds `"{title} · {tags}"` as the chunk's embed context ([RFC-obsidian-notion.md](RFC-obsidian-notion.md) records the intent: tag vocabulary carries topical signal). Shipped proof of concept — but only for Obsidian markdown, only at ingest, never stored or shown.
- **BM25 is chunk-text only** (the FTS index covers the `text` column alone), and the `source_ids` whitelist filter (driven by the sources-panel checkboxes) is the one source-level filter in the search path.
- **Notes have no source link.** The only annotation-to-source shape in the app is `LedgerAnchor { source_id, quote }`.

## Design

### Tags (v1)

**Data.** One new `tags` column on `sources` (Utf8, space-separated normalized tokens — flat scalars match every other column in the table; a Lance `List<Utf8>` would be the table's first, for no query we actually run). Parse and normalize on write (`#foo` → `foo`, lowercase, dedupe).

> Operational note: this is a schema append. Per the shared dev/prod store policy, older binaries brick on appends after a column migration — land it at the start of a release cycle and release promptly.

**Retrieval integration, in cost order:**

1. **Routes** — summary becomes `"{title} [{tags}] — {gist}"`. The self-healing diff re-embeds changed routes on the next sweep; zero new machinery. This is the biggest win and it lands where retrieval is weakest today: cross-notebook routing for ask-everything.
2. **Prompt surfaces** — manifest lines gain tags (`- {title} · {tags} — {url}`), and excerpt headers stay as they are (titles only; headers are already budgeted tight).
3. **Filtering** — tag → source-ids resolution in front of the existing `source_ids` whitelist. UI: tag chips in the sources panel filter row, and the same chips in the **Gallery view** — a tag row above the masonry grid that narrows the cards, which is where visual browsing actually wants tags ([RFC-source-gallery.md](RFC-source-gallery.md)). Chat can grow `in:#tag` later.
4. **Deliberately not v1: baking tags into chunk embed text.** Retagging would mean re-embedding every chunk of the source. Tags affect the *route* and the *prompt*, not chunk vectors. (The Obsidian path keeps its ingest-time behavior — frontmatter is stable at ingest by definition.)

### Per-source notes (v1)

One editable annotation per source (not a Note entity — no relation, no curator interaction):

- **Store** the text in a new `note` column on `sources` (same migration window as `tags`).
- **Index** it as a chunk row under `snote:<source_id>` — the exact trick gists and notes already use — re-indexed on edit the way `index_note` works, labeled in the prompt as `(your note on "{title}")` so the model knows it's the user's judgment, not corpus evidence.
- No gate needed. The gist gate exists because the machine guesses; the user doesn't.

### UI

- **Reader `DocProperties`**: Tags row + Note row (the block is already excluded from find-in-source and citation anchoring via `data-doc-meta`, so nothing added here corrupts highlight matching).
- **Sources panel `RowMenu`**: "Edit tags…" / "Edit note…" using the existing inline-edit modal pattern; hover card shows tags.
- **MCP**: `set_source_tags` / `set_source_note` tools (+ tags/note in `get_source` output) — agent-reachable per house convention.

## What retrieval honestly gains

- **Cross-notebook routing** is where tags matter most: routes are short, so a few tag tokens meaningfully shift a 480-char summary's embedding. Per-notebook chat inside a hybrid index dominated by verbatim chunk text will feel tags mostly through the manifest and filtering, not through rank.
- **Indexed annotations are the sleeper win**: "why did I save this" is exactly the query a user's own note answers and no chunk of the source ever will.
- Extend the retrieval eval with a small tagged corpus before claiming numbers.

## Out of scope (v1)

- Auto-tagging (LLM-suggested tags) — natural v2; would need gist-style gating, suggest-then-confirm.
- Tag browse pages, nested tags, tag colors, notebook-level tags.
- Multiple notes per source, note threading.

## Alternatives considered

- **Tags in chunk embed text** — rejected for v1: retag forces per-source re-embed; route-level integration gets most of the value for none of the cost.
- **Separate tags table** — rejected: a flat column plus parse matches how every other entity here is stored; there's no join engine to please.
- **Annotations as Note entities with a `source_id`** — rejected: adds a relation, curator interactions, and lifecycle questions; a column plus an `snote:` chunk row is the whole feature.
- **BM25 over a tags column** — rejected: the FTS index is single-column by design; tag *filtering* is exact-match, not ranked search.

## Recommendation

Build both, small: `tags` + `note` columns in one migration, route integration + manifest line + `snote:` indexing + the two UI surfaces + two MCP tools. Defer auto-tagging until the manual vocabulary shows what users actually type.
