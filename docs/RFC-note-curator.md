# RFC: Notes as memory — auto-created evidence notes and a curator

## Problem

Chat does real synthesis — cross-source conclusions, corrected approaches,
answered questions — and then the thread scrolls away. `save_note` exists
but requires the user to notice, mid-conversation, that an answer is worth
keeping; almost nobody does. Meanwhile notes are invisible to retrieval:
an evidence note written last week can't be found by hybrid search this
week, so notes are write-only memory.

The obvious fix — auto-save everything — silts notebooks up with mediocre
notes and erodes trust in the notes panel. Hermes-agent resolves this
tension the other way around: **create liberally on concrete triggers,
then curate ruthlessly in the background**. Creation is cheap and
recall-friendly; a curator, not a save-gate, controls quality.

## What we're borrowing from Hermes

- **Trigger-based auto-creation** — the agent writes a skill after a
  complex task succeeds, after an error → working-solution discovery, or
  after a user correction. Not after every turn.
- **Inactivity-triggered curation, not cron** — the curator runs at
  startup/idle ticks, gated by "≥7 days since last run" and "agent idle
  ≥2 hours". Active work is never interrupted.
- **Two phases** — phase 1 is deterministic and free (unused 30 days →
  stale, 90 days → archived); phase 2 is optional LLM consolidation
  (merge overlapping items), off by default because it costs tokens.
- **Usage telemetry drives staleness** — a per-item sidecar counts views
  and uses; the curator acts on data, not vibes.
- **Everything is recoverable** — archive instead of delete, snapshots
  before each run, pinning to opt items out, a human-readable report of
  what happened.

## Design

### 1. Notes join the retrieval index (prerequisite)

Embed note bodies into the existing LanceDB chunk table, tagged with
`note_id` (mutually exclusive with `source_id`) and carried through
`search_chunks` / `search_chunks_all` results. Citations render with a
distinct note badge — *what the corpus said* and *what we previously
concluded* must stay visually different classes, or agents end up citing
their own prior summaries as if they were documents. Re-embed on note
update; drop chunks on delete. MCP `search` results grow a `noteId`
field; agents get note recall for free.

### 2. Auto-create evidence notes from chat

After an assistant answer, a cheap post-pass decides whether the exchange
produced a durable conclusion. Triggers (any one):

- the answer synthesized across **2+ sources** and drew a conclusion,
- the thread hit a dead end and then found the working answer,
- the user corrected the assistant and the correction stuck.

When triggered, draft an evidence note (the kind that just shipped):
claim as title, supporting passages with source titles, the search
queries used, confidence, open questions. The chat pipeline holds all of
these in hand at answer time — no extra retrieval. Mark it
`origin: "auto"` so the curator knows it may touch it. The notes panel's
existing unread dot announces it; no modal, no interruption.

Dedup at creation: before writing, FTS the claim against existing
evidence notes in the notebook; on a strong match, update that note
instead of creating a sibling (mirrors Hermes preferring `patch` over
`create`).

### 3. Usage telemetry

Move note-read tracking from localStorage into the DB and extend it:
`reads`, `retrieval_hits` (a search returned one of its chunks),
`cited` (a chat answer actually cited it), `last_used_at`. Cheap counters
bumped where the events already flow. localStorage `noteReads` stays as
the UI's unread-dot cache; the DB is the curator's ground truth.

### 4. The curator

Rides the existing once-a-minute frontend tick (the one that runs report
schedules and folder rescans), self-throttled to at most **weekly**.
Scope: notes with `origin: "auto"` only — user-authored and user-edited
notes are implicitly pinned (editing an auto note flips it to user-owned
and revives it).

**Phase 1 — deterministic, always on.** Staleness counts **app-open
days** (days the app actually ran, tracked in `curator.json` next to the
config), not wall days — a month away from the machine must not archive
everything. An auto note unused for 30 open days is marked `stale`
(dimmed in the panel, badge explains itself); at 90 it is archived —
chunks dropped from the retrieval index, card collapsed into an Archived
section, never deleted. Any use since the last mark revives it (archived
notes get re-embedded). No idle gate: the pass is milliseconds of DB
work with zero model calls, so there is nothing to protect the user
from. Each run updates **one living "Curator report" note per affected
notebook** (updated in place — the curator must not generate its own
silt) listing what was staled/archived/revived and why.

**Phase 2 — LLM consolidation, on by default.** Settings → General →
"Consolidate auto notes weekly" (the toggle is cost control, not a safety
valve — the pass is idle-gated, capped, and fully recoverable, so the
smart behavior is the default). Candidate pairs come from title-embedding
cosine similarity (≥0.75) among a notebook's active auto evidence notes;
the chat model judges each pair with KEEP as the instructed default and
writes the merged record when they state the same claim. The older note
wins (stable id — existing citations keep resolving); the newer is
archived, never deleted. At most 3 merges per notebook per run, so a bad
week stays small and the next run catches the rest. This pass IS gated on
idle (≥30 min since the last user-initiated generation) and keeps its own
weekly stamp — a busy week defers it to the next quiet tick rather than
skipping it. Merges appear in the same living Curator report.

## Non-goals (v1)

- Procedural memory / skills. Hermes curates *how-to* knowledge; Alchemy
  notes are *conclusions*. The skill generator already covers the how-to
  export case.
- An approval queue. Single-user local app: archive + report + undo
  beats staged writes needing review.
- Cross-notebook curation. Notebook boundaries are the user's mental
  model; the curator respects them.
- Auto-creating from MCP agent sessions. Agents already decide when to
  write evidence notes; double-writing would fight them.

## Phasing

All five phases shipped 2026-07-14:

1. Notes into the retrieval index with labeled citations (+ MCP `noteId`
   in search results). Standalone value: evidence notes become findable.
2. Telemetry columns + bump sites.
3. Chat post-pass auto-creating/updating evidence notes.
4. Curator deterministic pass (stale/archive/revive) + living report note.
5. Curator LLM consolidation behind Settings → "Consolidate auto notes
   weekly", default on.

## Open questions

- Post-pass model: the local model is free but drafts worse evidence
  notes; the gateway model writes better ones but costs per answer.
  Leaning: same model the answer used — the marginal context is tiny.
- Should retrieval down-weight note chunks vs source chunks, or is the
  citation label enough? Leaning: label only in v1; measure before
  weighting.
- ~~Stale/archive thresholds: 30/90 days suit Hermes' always-on daemon;
  notebooks are opened in bursts. Maybe clock in *app-open days* rather
  than wall days so a month away doesn't archive everything.~~ Settled:
  app-open days (see §4).
- Does the palette/⌘K need an "auto notes" filter, or is the origin
  badge on cards enough?
