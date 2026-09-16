# RFC: Ablation — what Alchemy can lose

Status: draft, awaiting review (2026-09-15).
Origin: Reminders item "Reduce visual, data, and feature bloat. Conduct
an ablation experiment, removing unnecessary abstractions and designs."
The Ledger and the Arrivals strip went on 2026-09-15 (main e2f089b) as
the first two cuts; this is the rest of the list, with the evidence.

## Method

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

## Proposal

Do items 1, 2, 3 and 6 now — they are a day's work and remove the most
visible bloat (the tile wall) and the most dead code (Notion, the auto
passes). Measure 4 and 7 before deciding. Leave 5 and 8 as written
policy, not code changes.

## Open questions for Paul

- Are any of the four unused generators (PRD, PR/FAQ, RFC, Skill) or the
  ten unused templates ones you *want* to use and simply haven't?
  Templates keep them a click away; say which should stay as tiles.
- The Evidence pass: stop it, or keep it and make it visible (a "Save as
  evidence" verb on a chat turn) instead of automatic?
- Notion: gone, or is a token-bearing fetch something you plan to use?
