# WRITING.md

Source of truth for all user-facing words: the website, release notes,
in-app copy, README, and anything else a person who didn't build Alchemy
will read. The companion to DESIGN.md — that file governs how the product
looks; this one governs how it talks.

## The voice

Alchemy speaks like a careful engineer showing you something that works.
Quiet, concrete, and specific. It never sells, never winks at the reader,
and never claims more than it measured. When in doubt, say the plain thing
and stop.

## Register scales with the surface

Different zoom levels get different registers. Pick by where the words
live, not by mood:

| Surface | Register | Model | Example |
| --- | --- | --- | --- |
| Headlines, card titles | Two beats, then stop | Apple | "Search, benchmarked." / "Evidence you can open." |
| Body paragraphs | Plain declarative sentences, numbers inline | Google | "The built-in engine scores 0.737 on SciFact — higher than BM25 (0.665) — with everything running on your Mac." |
| Methodology, claims, footnotes | Sober and precise, zero personality | HashiCorp | "Each model is evaluated against the full pipeline: retrieval, citation of the known-correct evidence, and per-claim support." |
| Table cells, chips, micro-copy | Clipped fragments | Vercel | "Declines often. Perfect when it commits." |
| Release notes | Bold-led benefit, then the evidence | house (RELEASE.md) | "**Answers check themselves.** …faithfulness 0.69 → 0.84, measured." |
| In-app setting hints | One clipped line stating what On does; the toggle speaks for Off | Vercel | "Scheduled reports and syncs run with the window closed." |
| In-app empty states | One plain sentence; "appear here", never "land here" | Google | "Passages from your sources appear here as you write." |
| Epigraphs (hero, blank chat) | Alchemy flavor, quiet not grand, varied shapes | Linear | "The stone is refined, not found." |
| Error toasts | What failed in user terms, then one concrete fix | house | "No readable text in scan.pdf." |

Fragments are a micro-surface tool. In a paragraph, write sentences.

## Sentence mechanics

- **Em dashes**: at most one per paragraph, and never as a dramatic pivot
  ("—and see which provider answered"). If the dash is doing a reveal,
  use a period.
- **No em dashes in micro-copy** — tooltips, placeholders, toasts, and
  setting hints use a period, colon, or semicolon instead. (This rule
  resolved ~two dozen findings in the 2026-08 app copy review; the dash
  had become the house hinge.)
- **Safety reassurances repeat verbatim.** One canonical sentence
  ("Filing changes nothing in the document."), reused word-for-word
  wherever it applies. Identical copy is a label; four paraphrases of
  one reassurance are what reads generated.
- **Parallel items get different shapes.** Six table rows must not share
  one sentence template. Praise-pivot-caveat is fine once; repeated, it
  reads generated.
- **Tricolons**: allowed when the words are doing work ("Your sources,
  your machine, your models."), banned when the rhythm is the point
  ("real retrieval, real prompts, real answers").
- **No stacked adjectives** ("fastest accurate citations") and no
  dangling comparatives ("best two-source citations tested").
- One idiom per document, maximum. If a phrase already lives in the
  codebase comments, it does not also get to live on the website.

## Vocabulary

Internal names stay internal. Translate before publishing:

| Internal | Public |
| --- | --- |
| judged harness | our answer-quality tests / a reproducible test suite |
| gold evidence | the known-correct source |
| frontier-model judge | a stronger cloud model |
| multi-hop | two-source questions |
| grounded / grounding | cited / backed by the source |
| hard corpora | dense technical material |
| retrieval tier, shipping pipeline | the built-in search engine, the same pipeline the app ships |
| lands (as/in) | is saved / appears |
| embedder, embedding model | the search model |
| embed / re-embed (a source) | index / re-index |
| routing (a question) | searching / matching to notebooks |
| distilling (sources) | reading / summarizing |
| agentic retrieval, agentic mode | deep research |
| the curator / the sweep / Weave (job names) | Alchemy (the app just does it) |
| podcast voices, voice model | the Audio Overview voices |
| Embedded / Search-only (source tiers) | Indexed / Text only |

Banned outright in public copy: "vibes", "honest/honestly" as framing,
"seamless", "powerful", "supercharge", "leverage" (verb), "delve",
"game-changing", exclamation points.

## Claims

- Every number is one we measured, stated with its baseline and where to
  reproduce it (`cargo test --lib judged_`). No number, no claim.
- Name the comparison ("higher than BM25 (0.665)"), never the vague class
  ("state-of-the-art", "best-in-class").
- Report failures as plainly as wins. "A third of its cited claims don't
  hold up" is a sentence this product is proud to publish.
- Round to the precision the sample size supports. 25-question runs don't
  get decimal points of swagger.

## Search (the website)

The website (`docs/index.html`) is the app's only web page, so it carries
the whole search burden. The governing rule: **SEO lives in metadata and
structure, never in the voice.** No sentence gets longer to fit a keyword,
and keyword-stuffed prose is itself a tell.

The query vocabulary — the phrases a searcher actually types, each of
which appears at least once on the page in prose that would survive
review anyway:

- "local-first research notebook" (the category)
- "research notebook for macOS" / "macOS" (the platform)
- "NotebookLM" (the incumbent; factual mention only)
- "private" / "on-device" (the differentiator)

Mechanics:

- **One H1 per page.** The hero H1 stays brand-terse ("Your sources, your
  machine, your models.") only because the title tag, meta description,
  and lede carry the category phrase. Lose those and the H1 must become
  descriptive.
- **Title tag**: name + category phrase, under ~60 characters ("Alchemy —
  a local-first research notebook for macOS"). **Meta description**: one
  body-register sentence, ~155 characters, phrase up front. Both are copy
  surfaces; the register table and em-dash budget apply.
- Every page gets a **canonical URL** (we're on github.io, where duplicate
  paths resolve), **OG/Twitter tags** with a real image and alt text, and
  **JSON-LD** (`SoftwareApplication` on the product page). The claims
  rules apply inside structured data: no aggregate ratings or review
  counts we don't have.
- **Competitors by factual mention, once.** "Inspired by NotebookLM,
  built to stay private" is fine; "the best NotebookLM alternative" is a
  banned superlative.
- Sections keep **anchored ids** (#features, #models) so deep links land;
  every image gets alt text in the house voice.

## The tell check

Before publishing, scan for the patterns that read machine-written:

1. Em-dash pivots and em-dash density above ~1 per paragraph
2. Repeated-head-word triples ("real X, real Y, real Z")
3. The same sentence architecture stamped across parallel items
4. Winking at the reader ("data instead of vibes")
5. Internal vocabulary on a public surface (see table above)
6. Idioms recurring across documents ("earns its keep")
7. Encoding artifacts: `â` or `Ã` anywhere means a byte-level edit
   double-encoded the file — fix the encoding, not the strings

If a sentence could open a LinkedIn post, rewrite it.

## Precedents

The reworked benchmark sections (2026-08-18) are the reference example:
same numbers before and after, but the after reads like a person wrote
it. When adding new copy, match the register table above, then read it
aloud once. If you stumble, the reader will too.

The 2026-08-20 in-app copy pass set the app-surface precedents: the
epigraph lists in `src/lib/epigraph.ts` (Linear-voice alchemy flavor,
varied shapes — the generator prompt in `generate_epigraph` enforces the
same register), the settings hints (clipped, no off-clause), and
`friendly_error` in `src-tauri/src/commands.rs` as the model for every
user-facing error. Generated copy surfaces (epigraphs, briefs) get their
register from the prompt; when the register changes here, change the
prompt too.
