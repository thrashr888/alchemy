# RFC: Ablation — what Alchemy can lose

Status: reviewed 2026-09-21; items 1 (as a regroup, no tiles cut), 3 (as
a verb) and 6 built the same day. Decisions: the PM/engineering
generators and templates stay — newer users are product and engineering
people, and the app is for more than one person; the tile wall is shelved
by intent instead (Understand · Learn · Visualize · Write), most-used first
inside each shelf. Evidence is a verb on a chat turn ("Save as Evidence");
the automatic post-pass is retired. Notion stays: a second user has a key
and asked for the door. Item 6 was built and undone the same evening:
Paul prefers Personalization and Shortcuts as their own tabs, so the
twelve stay. Items 4 and 7 still want their counts.
Origin: Reminders item "Reduce visual, data, and feature bloat. Conduct
an ablation experiment, removing unnecessary abstractions and designs."
The Ledger and the Arrivals strip went on 2026-09-15 (main e2f089b) as
the first two cuts; this is the rest of the list, with the evidence.

## Product simplification: revised scope, 2026-10-09

Paul's correction: 104 lines of dead-code cleanup is too small. The goal is
an app that is easier to use and a codebase with fewer mechanisms to maintain.
This supersedes the narrow definition of success used by the first October
pass. The existing keep decisions still hold; the larger cuts below are
proposals for review, not implemented removals.

### The core job

Bring sources into notebooks, ask grounded questions, and keep useful answers
as editable, portable notes and documents. A person should need to understand
notebooks, sources, conversations, and saved work. Each additional durable
entity, automatic writing process, navigation destination, and alternate chat
path must justify the extra concept it introduces.

Usage is useful evidence but is not the only test. Overlapping workflows can
be simplified even when both are used. Conversely, low personal usage alone
cannot justify cutting a second user's integration. Hiding a feature in a menu
simplifies the screen; retiring its state and processing path simplifies code.
Measure both, and do not treat moving functions between files as an ablation.

### Larger candidates

Footprints below are the inspected files at `7ccddb1`, including tests and
comments. They describe the area to investigate, not promised deleted lines;
shared infrastructure and replacement code remain, and the rows overlap.

| Candidate | What gets simpler for users | What can actually leave the code | Inspected footprint and cost |
| --- | --- | --- | --- |
| Retire Registry as a separate product | No Entries/Suggested inbox, card kinds, entity approval, or second way to organize knowledge | Card suggestion/triage, fact enrichment, auto-attachment/rematching, registry UI and MCP CRUD, card-specific chat context | 5,295 lines across `commands/registry.rs`, `RegistrySection.tsx`, `mcp/registry.rs`, plus shared DB/store/command integration. Loses structured entity lookup and filing; original facts and provenance need a portable representation first. |
| Replace Brief, Staff, Reports, and Timeline with one Work view | One place to see scheduled work, refreshes, failures, and outputs | Separate feed/card layouts, timeline visualization, staff presentation, and automatic default-brief creation; consider retiring bespoke Brief synthesis/audio too | 2,897 lines across `HomeSections.tsx`, `HomeReportsFeed.tsx`, `TimelineSection.tsx`, `commands/brief.rs`. Scheduler, source events, receipts, requested schedules, and existing output notes stay. |
| One chat with selectable scope | Same conversation controls, drafts, history, cancellation, and citations everywhere; All notebooks or one notebook becomes scope | Parallel Home/notebook transcript and composer/run plumbing; duplicate per-surface settings and command routes where behavior permits | `HomeView.tsx` 2,331; `ChatPanel.tsx` 2,785; `homeChatRun.ts` 250. Most code is still useful. Share view and execution contracts without nesting agent loops or assuming every provider supports tools. |
| One retrieval backbone | Sources become searchable with less hidden preparation and fewer background stages | Challenge router index first; then section summaries and chunk re-embedding separately, retaining each only if it improves the relevant query families | `router.rs` 875; `gist.rs` 1,362 (also contains tag work); `outline_index.rs` 320. Citation text, long-document recall, exact identifiers, and large-corpus latency are gates. |
| One way to create and keep work | Create from an action in chat or the notebook; outputs live in Notes, with a schedule option when wanted | Separate generated-report/archive/wiki navigation and overlapping creation/schedule editors | `StudioPanel.tsx` 1,375 and `Reports.tsx` 488. Keep every generator, template, and specialist renderer previously agreed; simplify how they are reached and stored. |

### Recommended direction

Aim for three Home destinations: **Library, Chat, Work**. Inbox, Shared,
Built in, and Archived become Library filters. Work shows existing schedules,
run receipts, source changes, and links to saved outputs using ordinary rows.
Keep notebook sources and notes as the durable editing surfaces. Stop exposing
implementation vocabulary such as Staff and commission as separate concepts.
This is a proposal to replace twelve Home menu destinations, not a claim that
all twelve current destinations are equally prominent or independent features.

Registry is the largest product-removal candidate because it introduces an
entire parallel knowledge model. Before retiring it, materialize every kept
card's facts, identifiers, notes, and source links into ordinary editable
content, preserving its original record and refusal/attachment provenance in
a portable backup. Verify every kept record is recoverable. Never silently
promote Suggested records into the user's notes. If structured entity lookup
is essential, a smaller alternative is manual Entries only, with the automatic
suggestion/triage/enrichment machinery removed; that preserves more code and
one extra user concept, and must be counted honestly.

Work consolidation can begin independently of a Registry decision. Reuse the
existing schedule and receipt models; do not invent a task framework. Existing
requested schedules and generated notes remain inspectable. Removing default
Brief creation is distinct from disabling an existing schedule: existing
schedules need an explicit, reviewable migration or continued compatibility.
Preserve pause/cancel, failures, attribution, backups, and source-change history.
The historical Timeline visualization need not survive for history to survive.

### First retrieval ablation, actually run

Command: `ALCHEMY_EVALS=1 cargo test --lib retrieval_eval::eval_router -- --nocapture`
(with the existing shared build cache). This uses temporary fixture databases
and the built-in embedder, with no live model calls or changes to the user's
corpus. On October 9, the suite ran rather than skipping:

- 70 dataset queries over three notebooks.
- Routing accuracy@1: 0.79; accuracy@2: 0.97.
- Reported recall@10: **flat 0.43; routed top-two 0.43** (rounded output).
- Two top-two misroutes, involving the badge-system port and ERR-429 retry wait.
- The test passed its existing routing and recall fences.

This gives no measured recall advantage for the router in this fixture. It
supports testing removal of a separate routing index; it does not establish
large-corpus latency, equality beyond the printed precision, or performance on
Paul's actual corpus. The existing gist and enrichment fixtures use handwritten
model outputs, so their passing cannot establish that the production Small
model's ongoing synthesis is worth its cost.

### Acceptance tests for the larger experiment

For each candidate, remove one mechanism in an isolated branch and compare the
same tasks against the current app: import, ask across notebooks, find an exact
identifier, answer a long-document question, save/edit/export an output, inspect
a background failure, stop work, and recover existing data. Inspect the live
app's flows and compare the choices needed to finish them. Measure destination
and concept count, owned state/models and IPC/MCP routes, source lines removed
net of replacement code, query-family recall, first-answer and retrieval
latency, and background model calls. Each change must reduce a user decision
or a maintained mechanism; retain attribution, portability, and recoverability.

This is the revised PAUL-26 direction. The first 104-line PR remains a useful
cleanup prerequisite, not completion of the product-simplification goal.

## Review: 2026-10-09 (PAUL-26)

The decisions above still govern this pass: keep Notion, every generator
and template, the separate Personalization and Shortcuts tabs, themes,
meta-chat, MCP, Grow, OKF, and capture. The proposals below are the original
September candidates, not approval to reverse those decisions.

The current code at `a9e5dcb3` has Home place tracing and a single notebook
shelf. Registry appears as Entries plus Suggested. This review separates
code that is no longer reached from capabilities whose use needs evidence.

### Code cuts

- **Retired automatic evidence prompt and merge branch:**
  `build_auto_evidence_messages` has one caller, `build_evidence_messages`,
  which always passes `None` for the prior record and immediately replaces
  the automatic system prompt. Build the requested record directly and
  remove the old prompt and unreachable prior-record branch. The system
  prompt and user message sent by Save as Evidence stay identical, including
  excerpt order and text. Keep the shared parser and the curator's separate
  consolidation prompt: both still have live callers.
- **Collapsed Home rail:** `SidebarRail` has no callers after the Library
  restructure. Remove it and its two exclusive icon imports. Active Brief
  and Staff sections keep their current controls and markup.

Together these cuts remove 119 lines and add 15: **104 net source lines**,
with no data changes or user-visible feature removal. This measures code
reduction; it does not claim faster startup or smaller built assets.

### Feature evidence

Read-only snapshot of `traces/ui.jsonl`, taken October 9 in America/Los_Angeles:
44 `home.place` records from September 28 through October 9, on four local
calendar dates (September 28, October 7, October 8, October 9).

| Home destination | Recorded transitions |
| --- | ---: |
| Notebooks | 31 |
| Chat | 4 |
| Inbox | 4 |
| Staff | 2 |
| Suggested | 2 |
| Reports | 1 |
| Brief | 0 |
| Entries (`registry`) | 0 |

These are navigation records, not people, sessions, completed reads, or
exposure time. A restored view, Back/Forward navigation, and access through
notes, search, or agents can bypass this trace. Four observed dates are not
the two-week usage sample called for by this RFC. Zero records therefore
does not justify removing Brief or Entries. The installed MCP endpoint was
unreachable during this review, so September's 229 Registry entries are a
historical count, not a current inventory.

The maintenance footprint helps prioritize inspection, not decide removal:
Registry has 2,954 lines in its command module and 1,928 in its UI; Brief has
511 in its command module and shares Home surfaces. These counts include
tests and comments and omit shared DB, model, and MCP code. Registry also
feeds chat context, source filing, and portability: deleting only its Home
section would leave most of its machinery. Brief uses ordinary report
schedules and notes, so removing it would not remove the report engine.

The feature decision remains open until broader usage and a current corpus
inventory support a reversible cut. Follow-up tracking belongs to PAUL-26
in Linear.

## Original method and September evidence

An ablation removes one thing and measures. Here the measurement is
usage, and the store is the instrument: the 47 notebooks on Paul's Mac
(22 active, 24 archived, 1 system), their 3,760 sources and 566 notes,
the retrieval trace, and the code's own surface counts. A feature nobody
used in six months of daily use, that also costs code, is a candidate.
A feature used twice that costs nothing stays. The rule: **cut what
nobody reached for; keep what is cheap; never cut what the invariant
needs** (inspectable, attributed, portable, recoverable).

Surface today: 172 IPC commands, 62 MCP tools, 26 Studio generators plus
11 built-in templates (the 37 tiles), 12 Settings sections, 31 themes, 67
React components, 116 Rust modules (104k lines Rust, 54k lines
TypeScript).

## Evidence

**Notes by kind, whole store (566):**

| kind | count | | kind | count |
| --- | --- | --- | --- | --- |
| note (hand-written) | 468 | | quiz | 4 |
| report (scheduled) | 20 | | briefing | 4 |
| summary | 15 | | flashcards | 3 |
| evidence (auto) | 12 | | infographic | 3 |
| process map | 5 | | journey, study guide, faq, data table, timeline | 2 each |
| relationship map | 5 | | architecture, data model, uml, problems, round table, audio overview, insights, key themes | 1 each |
| slide deck | 4 | | mind map | 4 |

Four of the 26 generators have never produced a note that survived —
PRD, PR/FAQ, RFC, Skill — and of the 11 built-in templates (SWOT, Press
release, Meeting agenda, User stories, SOP, Memo, Blog post, Tech spec,
Category taxonomy, Key entities, Key themes) only Key themes was ever
run, once. Twenty-two generators account for 96 notes; the top five
(summary, process, relationship, slide deck, mind map) for a third of
those.

**Notes by origin:** 534 user, 25 by the app's own passes across nine
versions (second-look, auto evidence, curator), 4 by agents over MCP.
The machine writes a note about once a week; the person writes daily.

**Retrieval trace (last 204 entries):** chat 65, chat timing 58, meta-
chat 33, verify 33, MCP 15. Meta-chat ("ask everything") is a real
surface. Deep research does not appear as its own surface (it rides
chat).

**Sources by type:** url 1,121 · markdown 1,112 · text 854 · code 446 ·
pdf 117 · image 62 · html 18 · mac 9 · git 7 · folder 7 · feed 6 ·
obsidian 1 · notion 0. Folder, git and feed parents are few but each
fronts hundreds of children (the markdown and code counts are mostly
theirs). Notion: zero.

**Capture (2,844 renders):** rendered-first 1,352, not-better 527,
still-blocked 505, rescued 232, failed 228. The hidden render pass earns
its keep; the domain memory does most of the work.

## Candidates, in cut order

Each line: what, the evidence, what it costs to keep, the proposal.

1. **The *More (33)* wall.** Four generators (PRD, PR/FAQ, RFC, Skill)
   and ten of the eleven built-in templates have never been run. Each is
   a prompt plus a tile, and together they bury the five people use.
   Proposal: the four unused generators become built-in templates (the
   user-editable prompt list already exists, `templates.rs`), and the
   ten unused templates stop shipping as tiles — they stay in the repo
   as a starter set a person can add from Settings → Studio. The grid
   shows the 22 generators with a note to their name; "More" becomes
   the template picker. No capability lost; 37 tiles become 22 plus
   whatever the person keeps.
2. **Notion source.** Zero sources, a 700-line module, a token in
   Settings, an export tree cache, its own refresh branch. Proposal:
   remove. Notion pages are a URL source with a token-bearing fetch, and
   nobody has one. Keep the RFC as history.
3. **Second Look and the auto Evidence pass.** 12 evidence notes and 1
   second-look note in six months; the pass spends a model call after
   chat turns to decide whether to write one. Proposal: keep the *capability* as an MCP/CLI verb
   ("write up this thread as evidence") and stop running it unasked. The
   invariant prefers proposals; this one was writing notes on its own.
4. **The Registry (V12 pillar 4).** Not measured here — cards live in
   their own table — but the Home Registry section and the auto-file
   sweep on every import are worth a count before the next pass.
   Proposal: measure (`list_registry` size, cards opened from the palette
   trace) and decide next round. Flagged, not cut.
   *2026-09-28:* Paul's store holds 229 entries and a queue of 0–10
   suggestions per day. Opens are now traced: every Home place change
   writes `traces/ui.jsonl` (`home.place` with section, scope, tag, card),
   so `registry` / `suggested` visits and card opens can be counted after
   two weeks. In 0.66.0 the Registry became two Home sections (Entries,
   Suggested) rather than a card.
5. **Themes: 31.** Cost is one file and the harness; each shader mode is
   real work to keep alive. Proposal: keep the 31 — they are cheap at rest
   and the reminder is about *feature* bloat — but stop adding shader
   modes per theme; new themes reuse an existing backdrop.
6. **Settings: 12 sections.** Personalization and Shortcuts are the two
   a person opens once. Proposal: fold Shortcuts into About (it is a
   reference), Personalization into Chat. Ten sections. Cosmetic; cheap.
7. **Home: Chats / Staff / Brief / Latest Reports.** Staff is the Night
   Shift roster. With the Ledger gone the Brief has fewer things to say.
   Proposal: measure Brief opens (add a trace line) for two weeks before
   touching Home; if the Brief is read, it stays.
   *2026-09-28:* the trace exists — `traces/ui.jsonl`, `home.place` records
   with `section: "brief"` — and 0.66.0 already moved the four cards into
   sidebar rows (Chats, Staff, Nightly Reports) and a Brief row above the
   Library. Count from mid-October.
8. **Two web-clip paths.** The Chrome extension and the in-app capture
   both exist; the extension's logged-in capture shipped in 1.2.0.
   Proposal: keep both — they answer different pages — but the assisted
   capture (RFC-page-capture §5, in-app login) is not built and should be
   dropped from the roadmap in favor of the extension.

Not candidates, and why: **meta-chat** (33 traces, a real surface);
**folder/git/feed sources** (few parents, most of the corpus); **MCP**
(62 tools is the agent-native rule, and agents wrote 4 notes and 15
searches without being asked to); **Grow** (the hygiene and Edit URL
paths were used this week); **OKF live** (the invariant's portability
clause lives there); **the capture pass** (numbers above).

## Visual bloat

Separate from features, same discipline. The 2026-09-14 UI-consistency
reminder (e986bca9) is the audit; this RFC only names what the data
already shows: the Studio tile wall (item 1), the Settings sidebar
(item 6), and the notebook header's doubled menus (fixed 2026-09-15, one
menu now). A pass over `src/components` for raw `<button>` / `<input>`
elements outside `ui.tsx` primitives is the next measurable step; it is
a grep, and it belongs to the consistency item.

## How to run the experiment

Each cut is one commit, one release note line, and one number to watch
for two weeks: for generators, template use; for Notion, a support
question that never comes; for the auto passes, whether Paul asks for
evidence notes by hand. A cut that anyone misses is reverted by
reverting the commit — the invariant's *recoverable* clause applies to
features too, which is why every cut here is code-only and leaves data
(the Ledger's table, the Notion cache dir) in place.

## Original proposal (superseded by the reviewed decisions)

Do items 1, 2, 3 and 6 now — they are a day's work and remove the most
visible bloat (the tile wall) and the most dead code (Notion, the auto
passes). Measure 4 and 7 before deciding. Leave 5 and 8 as written
policy, not code changes.

## Original questions (answered in the reviewed decisions)

- Are any of the four unused generators (PRD, PR/FAQ, RFC, Skill) or the
  ten unused templates ones you *want* to use and simply haven't?
  Templates keep them a click away; say which should stay as tiles.
- The Evidence pass: stop it, or keep it and make it visible (a "Save as
  evidence" verb on a chat turn) instead of automatic?
- Notion: gone, or is a token-bearing fetch something you plan to use?
