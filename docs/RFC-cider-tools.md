# RFC: Cider tools — Mac apps as chat context and actions

## Summary

Embed [cider](https://github.com/thrashr888/cider) so Alchemy's agentic chat
can see and touch the Mac: **read Calendar, Reminders, Notes, Contacts, and
Mail as grounding context; take actions (create a reminder, draft an email)
with per-action confirmation**. "What's on my calendar this week?" becomes a
notebook-adjacent question, and "turn this briefing's action items into
Reminders" becomes one turn of chat.

The design problem is not technical — cider is our Rust crate, embedding is
easy. It's the **permission model**: reads and writes have very different
blast radii and need different consent shapes.

## Background

- cider (`cider-cli` on crates.io) wraps 30+ Apple apps behind a uniform
  JSON-out interface, built for both humans and agents. It shells out to
  Apple frameworks (EventKit, Contacts, Notes via ScriptingBridge, …).
- Alchemy's agentic chat (`agent.rs`) already runs a plan → tool → answer
  loop with tools like source search and reads; adding tools is a matter of
  extending the tool registry and prompt.
- macOS TCC is the outer permission wall: the *app binary* must hold
  entitlements/usage strings (Calendars, Reminders, Contacts, …), and the
  OS prompts the user per data class on first touch. Our inner permission
  model layers on top of that, it doesn't replace it.

## Proposal

### 1. Integration shape: embed the crate, not the binary

Depend on cider as a library (split a `cider-core` crate if the CLI wrapper
isn't cleanly importable — we own the repo). In-process calls mean typed
results, no PATH/install dance for users, and TCC prompts attributed to
Alchemy.app rather than a helper binary.

Fallback if the crate split fights us: bundle the `cider` binary as a Tauri
sidecar and speak its JSON. Same tool surface either way; the RFC's
permission model is independent of this choice.

### 2. Tool inventory — start read-heavy

Phase 1 (read-only):

| Tool | Backing | Example ask |
|---|---|---|
| `calendar_events(range)` | EventKit | "what's on my calendar this week?" |
| `reminders_list(list?)` | EventKit | "what's still open on my Shopping list?" |
| `notes_search(query)` / `notes_read(id)` | Notes | "pull my 'Vendor calls' note in as context" |
| `contacts_search(query)` | Contacts | "what's Sarah's email?" |
| `mail_search(query, mailbox?)` | Mail | "find the thread about the Q3 invoice" |

Phase 2 (writes, each gated — see §3):

- `reminders_create(title, list?, due?)`
- `calendar_create_event(...)`
- `notes_append(id, text)`
- `mail_draft(to, subject, body)` — **draft only**, never send.

Deliberately out: sending mail/messages, deleting anything, Keychain,
Music/media control (fun but off-mission).

### 3. Permission model — the point of this RFC

Three tiers, enforced in the tool dispatcher (not the prompt):

1. **Off by default.** A "Mac apps" section in Settings → Agents lists each
   app (Calendar, Reminders, Notes, Contacts, Mail) with a toggle,
   mirroring the TCC grants. Nothing is callable until toggled on. The chat
   tool registry only includes enabled apps' tools, so disabled tools don't
   even exist as far as the model knows.
2. **Reads: allowed once enabled.** Read results flow into context like
   source chunks do today. Every read is logged to the chat transcript as a
   visible tool step ("Read 12 calendar events"), so nothing is silent.
3. **Writes: per-action confirmation.** A write tool call renders an inline
   confirmation card in chat — the exact payload, human-readable ("Create
   reminder 'Renew registration' in list 'Car', due Friday") with
   Confirm/Cancel. The tool blocks on the user's click; Cancel returns a
   "declined" result to the model. No "always allow" in v1 — per-action
   friction is the feature until trust is earned.

Cross-cutting rules:

- Tool results are **data, not instructions**: wrap them in the same
  provenance framing chat uses for sources, so a calendar event named
  "ignore previous instructions" stays an event title.
- Mac-app context is **never embedded or persisted** into the notebook
  unless the user explicitly converts it to a source; by default it lives
  only in the turn's context window.
- Chat privacy note in Settings: when a remote gateway (OpenAI-compatible)
  is configured, Mac-app reads leave the machine like any other context.
  Worth a one-line warning under the toggles; local Ollama has no such leak.

### 4. UI

- Settings → Agents → "Mac apps": per-app toggles + a "test" button per app
  that runs a benign read and surfaces the TCC prompt at a predictable
  moment (rather than mid-chat).
- Chat: tool steps already render; write-confirmation cards are the one new
  surface. Reuse the existing tool-message kind.
- MCP: expose the same tools over the embedded MCP server **only for reads**
  in v1 — a remote agent clicking a confirmation card is unresolved UX;
  writes stay chat-only until that's designed.

### 5. Phasing

1. Crate integration + Calendar/Reminders reads + settings toggles.
2. Notes/Contacts/Mail reads; convert-to-source affordance.
3. Writes with confirmation cards.
4. (Later) MCP read exposure, "always allow" per tool, more apps.

## Open questions

- Does `cider`'s current crate layout expose a callable API, or do we do
  the `cider-core` split first? (Owner: us; small either way.)
- TCC entitlements: which usage-string keys does the app need in
  `Info.plist`, and does notarization care? (Prototype phase 1 and find out.)
- Should reminders/calendar reads be schedulable into Reports ("include my
  week in the Monday briefing")? Natural extension; defer until reads land.
