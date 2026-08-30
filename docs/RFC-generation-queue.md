# RFC: The Generation Queue

**Status:** Draft, awaiting review
**Owner:** Paul
**Prior art:** NotebookLM's studio queue; docs/RFC-diagnostics.md (Staff events); docs/RFC-living-notebook.md (background sweeps)

## Problem

Document generation blocks the studio. `generateArtifact` refuses to start
while `generatingKind` is set, so one generation locks every tile in every
notebook; the result only exists when the full pipeline returns, so an app
restart mid-run loses the work invisibly; and when the chat role points at
Ollama while Ollama is down, the run dies with an error the user discovers
only after waiting.

NotebookLM's answer is the right shape: the artifact appears **immediately**
as a pending item, progress lives on the item, several can cook at once, and
the studio stays interactive throughout.

## Design

### 1. Jobs, persisted

A generation becomes a **job** in a queue the backend owns:

```
GenJob { id, notebook_id, kind, template_id?, prompt?, source_ids,
         note_id, status, error, progress_chars, created_at, updated_at }
status: queued | running | waiting-engine | done | error | cancelled
```

Jobs persist to `<app-data>/generation-queue.json` — a sidecar, not a Lance
column, for the same reason as the growth flags: the store is shared
dev/prod and schema changes brick older binaries. The file is small (jobs
prune once `done`/`cancelled` and older than a day).

### 2. The pending note is the artifact

Enqueueing **creates the note immediately** with `status: "pending"` and a
placeholder body. It renders in Studio at the top of the notes list with a
progress indicator and a Stop verb — the same place the result will land,
so there is no separate "queue UI" to learn. Progress streams over
`generation://progress` events keyed by job id (chars so far, stage label).
On completion the note flips to `ready` in place; on error it shows the
message with Retry/Remove verbs. Deleting a pending note cancels its job.

### 3. A backend worker owns execution

Today the webview awaits the whole pipeline, which is why reload kills it.
Instead the enqueue command returns as soon as the job and pending note
exist; a worker task inside the backend drains the queue. The webview is a
spectator — reload, navigate, or close the window and the run continues.

Concurrency: **one job per engine at a time** — local engines (Ollama, the
built-in, MLX) serialize because parallel decodes thrash the same GPU/RAM,
while distinct engines (a cloud gateway beside Ollama) run in parallel.
This gives "more than one at a time" exactly when it actually helps.

### 4. Engine-down means paused, not dead

Before running a job the worker probes the role's engine (the existing
health-check path). Unreachable → the job parks as `waiting-engine`, the
pending note says "Waiting for Ollama — it isn't running", and a Staff
event announces it once. The worker re-probes every 30s and auto-resumes
when the engine answers; models loading cold are just a slow first token,
already handled. Cancel remains available while parked.

### 5. Restart resume

At boot the worker reloads the queue file. Jobs found `running` (the app
died mid-run) re-enter as `queued` and restart from scratch — generation is
cheap relative to the complexity of mid-stream checkpoints, and the pending
note is still there to show it. `waiting-engine` jobs resume waiting.
(Checkpointed partial content is a possible v2; not in scope here.)

### 6. Agent-reachable

MCP gains `list_generations`, plus enqueue via the existing generate tools
(they now return a job id immediately) and `cancel_generation`. The Staff
feed shows queue activity the same way wiki/growth events land today.

## What changes for the user

- Click three generator tiles in a row: three pending notes appear, each
  with live progress; the studio never locks.
- Restart the app mid-generation: the pending notes are still there,
  running again on their own.
- Ollama off: the note says so plainly and the run starts by itself once
  Ollama is back — no silent failure, no lost click.

## Out of scope

- Mid-stream content checkpoints (v2).
- Cross-notebook queue UI — the pending notes in each notebook are the UI.
- Priorities/reordering: FIFO per engine is enough at this scale.

## Implementation notes

- `begin_generation(scope)`'s per-scope cancellation tokens generalize to
  per-job tokens; the global `generatingKind` gate in the store shrinks to
  "is *this notebook's* studio streaming a preview".
- The audio path (podcast) joins the same queue with `kind: "audio"`; its
  progress events already exist.
- `justCreatedNoteId` auto-open behavior moves to job completion, keeping
  the "result appears where you acted" rule when the notebook is open.
