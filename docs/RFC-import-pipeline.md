# RFC: The Import Pipeline — extract once, land instantly, store lean

## Summary

Three complaints arrived in one afternoon and they are one design problem:
a CSV rendered as pipe-riddled prose, a file import that sat on a spinner
while a model thought about its title, and an 11 GB `lancedb/` directory on
an install whose live chunk data measures 69 MB. This RFC unifies the answer:

1. **One extraction contract** — every format extracts to GitHub-flavored
   Markdown, which is simultaneously the retrieval text and the faithful
   render. [firecrawl/anydoc](https://github.com/firecrawl/anydoc) (Rust,
   MIT) replaces our bespoke docx/pptx/epub/spreadsheet extractors.
2. **Imports land before intelligence runs** — the source row appears the
   moment extraction finishes; embedding, titling, tagging, and registry
   matching are background stages that update the row via events.
3. **The database maintains itself** — Lance is additive and never prunes;
   compaction + version cleanup run at startup and after index rebuilds.

Storage economy leg (§3) shipped with this RFC. §1 and §2 are the work.

## What we measured

The 11 GB was not content. Autopsy of the live install
(`~/Library/Application Support/com.thrashr888.alchemy/lancedb`):

| table | on disk | live data | versions | fragments |
|---|---|---|---|---|
| chunks.lance | 9.8 GB | **69 MB** | 11,605 | 4,085 |
| source_events.lance | 230 MB | small | — | — |
| sources.lance | 174 MB | 35 MB | 3,084 | 2,873 |
| notebooks.lance | 68 MB | ~28 rows | — | — |

8.6 GB of the chunks table is `_indices/`: every `rebuild_chunks_fts`
(`db.rs`) writes a complete new Tantivy index with `.replace(true)`, and the
superseded one stays on disk as a checked-out-able old version, forever.
Every row update does the same at smaller scale — 68 MB of notebooks is
thousands of dead `touch_notebook` versions. Separately, the same fragment
count taxes CPU: a 1276%-CPU sample of the app showed every hot frame in
DataFusion scan machinery, because background sweeps re-read source content
across 4,085 fragments per scan, in parallel.

The lesson that generalizes: **in this app the expensive thing is never the
model — it is unbounded background reads and unpruned writes.**

## §1 — anydoc: one extractor, markdown out

`ingest.rs` grew one bespoke extractor per format: `extract_docx` (zip +
XML), `extract_pptx`, `extract_epub`, `extract_spreadsheet` (calamine),
`delimited_to_rows` (csv crate). Each picked its own output shape, which is
how CSVs came to render as prose: the reader renders markdown faithfully
(`Markdown.tsx`, remark-gfm, scroll-contained tables) but the extractors
weren't producing it.

anydoc is the consolidation: Rust core, MIT, no models or services, converts
doc/docx/docm, ppt/pptx, xls/xlsx/xlsm/xlsb, odt/ods/odp, RTF, EPUB, CSV,
and PDF to GFM — headings, tables, lists, footnotes preserved — with median
conversion under 5 ms. It is built on pdf-inspector, which we already ship
for PDF extraction, so the dependency graph barely moves.

**The contract:** `extract()` returns markdown for every document format,
and the markdown IS both stores at once — chunked and embedded for
retrieval, rendered verbatim for the reader. No second "display" artifact,
no divergence between what the model cites and what the user sees.

What stays ours: URL capture (cookies, clip receiver), git sources, Notion
export, Apple integrations via cider, OCR fallbacks for scanned PDFs and
images, and the structure-aware chunker. anydoc replaces only the
file-parsing leg. The interim csv/xlsx→GFM-table extractors (shipped with
this RFC, `rows_to_markdown_table` in ingest.rs) become anydoc's job the day
it lands; behavior should not change, which makes those extractors the
acceptance test.

Adoption is behind `extract_any_file`: try anydoc for its formats, keep the
bespoke path as fallback for a release, delete the bespoke path when the
trace shows no fallbacks firing.

## §2 — async import: land the row, then get smart

Today `store_new_source` (commands.rs) chunks, embeds, and classifies before
the source row exists, and callers awaited a Small-model title call before
even that. On a cold model the user stares at nothing for the whole chain.
The titling call is already off the critical path (`spawn_retitle`, shipped
alongside this RFC); embedding is the remaining inline stage.

**Pipeline:** extract (fast, local, anydoc) → **persist immediately** with
`status: "processing"` → background stages, each updating the row and
emitting the existing `mcp://changed` scope `"sources"`:

1. chunk + embed → insert chunk rows → mark FTS dirty → `status: "ready"`
2. retitle (already shipped, semaphore-bounded)
3. registry match (`spawn_registry_match`, already backgrounded)
4. gist/tag sweep kick (already backgrounded)

Failures in stage 1 set `status: "error"` with the message, exactly like
extraction failures today. Stages 2–4 stay best-effort.

`"processing"` is a new source status alongside
ready/error/placeholder/stale. UI: the source row renders immediately with a
subtle working indicator (the gallery/list already show status chips);
chat's source picker treats processing sources as not-yet-selectable, the
same way it treats errored ones. Search simply doesn't find the source until
its chunks land — acceptable, because the alternative is the current
nothing-at-all.

**Bounded concurrency is the design rule, not an option.** One embed worker
(a folder drop queues, it does not fan out), the retitle semaphore stays at
2, and every corpus-wide background reader coalesces re-requests the way
`spawn_rematch_all` now does (running+pending flags, yield between scans).
New background work must justify itself against the DataFusion sample above.

## §3 — storage economy (shipped)

- **`Db::maintain()`** (db.rs): per table, Compact then Prune versions older
  than an hour. Runs 30 s after launch and every 6 hours from the resident
  scheduler (ahead of the background-work gate — disk hygiene is not AI
  spend). One Lance caveat: files newer than 7 days are kept unless
  `delete_unverified` is set, which is only safe with a guaranteed single
  writer — and dev + prod builds share this store, so we don't set it.
  Reclaim is therefore progressive: everything older than a week frees
  immediately, the rest as it ages past the window. Growth stops on day one.
- **Prune after FTS rebuild** (`rebuild_chunks_fts`): each rebuild drops
  its predecessor once it is 10 minutes stale, so a chatty sweep session
  can no longer grow the table by gigabytes of dead Tantivy.

### On "just store pointers, not contents"

Considered and rejected as the default. The measured content cost is 69 MB
of chunks + 35 MB of sources — about 1% of the problem, and it buys the
reader, citations, find-in-source, re-embedding after model changes, and
refresh diffing, all offline. File-backed sources do keep a pointer (`url`
is the path) and the placeholder machinery already evicts folder children
that were never read; that remains the eviction lane. If a future corpus
makes content genuinely heavy (video transcripts, massive exports), the
lever is per-source eviction to `status: "placeholder"` with re-read on
open — never a global pointers-only mode, which would silently break every
offline surface for a rounding-error saving.

### Follow-ups in this lane

- **Incremental FTS** instead of whole-index `.replace(true)` rebuilds —
  Lance supports index optimization; the rebuild-the-world approach is why
  the versions were index-sized. Debouncing `flush_fts` is the cheap half.
- **source_events** (230 MB for a 30-day rolling window) should prune on the
  same maintenance pass — the table's own window logic deletes rows, but
  deleted rows live on in old versions until pruned. Covered by `maintain()`.
- A Settings row surfacing last-maintenance stats ("reclaimed 9.5 GB") the
  first time it runs, so the cleanup is visible rather than mysterious.

## Order of work

1. §2 processing status + background embed (one worker), behind the existing
   status machinery. Verify with a folder drop: rows land instantly, ready
   states trickle in, search finds them when ready.
2. §1 anydoc behind `extract_any_file` with fallback + trace line per
   fallback fired. Re-import fixtures; the CSV/XLSX table tests must pass
   unchanged.
3. Remove bespoke extractors once the fallback trace stays quiet for a
   release.
