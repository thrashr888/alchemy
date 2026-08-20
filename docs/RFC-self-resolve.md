# RFC: Self-Resolve

**Status:** all four phases shipped
**Goal:** when something breaks, the app helps the user fix it — in place,
in plain language, with the cheapest loop that can do the job.

## Problem

Every backend error crosses IPC as a flattened string (`commands.rs::e`)
and lands in one of two places: a toast (pre-stream failures — embedding,
retrieval) or a durable `kind: "error"` transcript row (a chat/generation
stream that died). The transcript row was a real improvement — an
unanswered question is never a mystery — but the string inside it is
still mostly transport noise:

> ollama chat request failed: error sending request for url
> (http://localhost:11434/api/chat): error trying to connect: tcp connect
> error: Connection refused (os error 61)

The user who reads that has to already know what Ollama is, that it's a
separate process, and that `ollama serve` starts it. Meanwhile the app
knows all of it: which provider was configured, which model was asked
for, what the fix is. Some corners already act on that knowledge — the
gateway translates HTTP statuses into advice, agent CLIs attach sign-in
and model-pin hints, `friendly_error` rewrites the Lance schema-skew
dump — but the coverage is piecemeal and the most common local failure
(Ollama down, model not pulled) still surfaces raw.

## Error taxonomy

What actually fails, and how each class is recognizable:

| Class | Example raw shapes | Deterministic? | Fix |
|---|---|---|---|
| Provider down | `Connection refused` on `:11434`; gateway "Couldn't reach the provider" | yes | start Ollama (`ollama serve`) / check base URL |
| Model missing | ollama 404 `model "x" not found, try pulling it first`; gateway 404 naming a model | yes | `ollama pull x` / pick another model |
| Auth | 401/403, `invalid api key`; agent CLI signed out | yes | re-enter key in Settings → Models / CLI sign-in (existing `auth_fix_hint`) |
| Billing / rate limit | 402, 429 (+"credit"/"quota" body) | yes | top up, wait, or switch model (existing gateway advice) |
| Model busy / loading | `operation timed out` | yes | wait and retry; smaller model |
| Misconfiguration | bare 404 (base URL), wrong effort/param rejections | mostly | fix endpoint in Settings → Models |
| Ingestion failures | unreachable URL, "no longer serves a PDF", OCR model absent | partly | per-source status row; retry/refetch |
| Index / store problems | Lance schema skew, FTS not ready | partly | existing `friendly_error` arm; reindex |
| Everything else | provider-specific bodies, novel shapes | no | phase 2's diagnosis loop |

## Shape

Three loops, cheapest first. A dumber loop that matches always wins —
the model is the fallback, never the front door.

1. **Deterministic classifier** (phase 1, shipped). One function at the
   IPC boundary — `friendly_error` — recognizes the known shapes above
   and rewrites them into a sentence that names the fix. Two literal
   grammars in the output are load-bearing, because the chat error row
   already turns them into buttons: `` Fix: open Terminal, run `cmd`,
   then retry here. `` becomes a one-click Terminal launch (strictly
   allowlisted server-side), and the phrase `Settings → Models` becomes
   a jump to the right Settings tab. Toasts get the same treatment: an
   error toast that names Settings → Models is clickable and opens it.

2. **Diagnose-and-suggest** (phase 2). When the classifier doesn't
   match, a small on-device model turns the raw error plus a *redacted*
   config snapshot (provider kinds, labels, model names — never keys or
   URLs with credentials) into a two-sentence diagnosis and suggested
   fixes. Crucially, the diagnosing model must not be the failing one:
   route to the Small role, and when the failure *is* the local stack,
   fall back to the Apple FM sidecar; when neither is alive, skip the
   loop entirely (the cleaned raw error still shows). Fixes are picked
   from a fixed action vocabulary — open a Settings tab, switch a
   role's provider, retry, run an allowlisted terminal command — the
   model chooses verbs, it never authors shell or free-form config.
   Parse-or-skip: an unparseable diagnosis is dropped, never shown.

3. **Settings tool in chat** (phase 3). A `settings` tool reachable
   from the chat tool router (`try_tool_route`) and as an MCP tool, so
   "switch chat to Ollama" or a phase-2 fix button can apply the change
   in place. Get/set over a strict allowlist of `AiConfig` fields:
   `chatProvider`, `studioProvider`, per-provider `chatModel` /
   `effort` / `baseUrl`, `smallModel`, `embedder`. Never secrets:
   `apiKey` is neither readable nor writable through this tool, and
   reads redact anything key-shaped. Every change echoes into the
   transcript as a tool row ("Switched chat to Ollama · gemma3") so the
   config never moves silently.

4. **Fallback for this reply** (phase 4). When the chat provider fails
   mid-question, the error row offers "Answer with Ollama" / "Answer
   with Apple Intelligence" — a one-click rerun of *this* question on a
   local engine, config untouched. Same mechanism as the existing Retry
   button (delete the pair, resend through the normal pipeline), plus a
   one-shot provider override on the send. Offered only when the
   fallback engine is actually alive (readiness is already probed for
   the provider pill).

## Defaults and cost

Default-ON, per house rules. Phases 1, 3, and 4 are free — string
matching, a settings write, a rerun the user clicks — and get no toggle.
Phase 2 is the only loop that spends tokens on its own initiative; it
ships ON with one Settings toggle (`selfDiagnose`) as cost control, not
opt-in.

## Discipline

- No secret ever enters a prompt or leaves through the settings tool.
- Terminal commands are allowlisted server-side; the `ollama pull`
  model name is charset-validated (`[A-Za-z0-9._:/-]`, ≤ 64 chars) at
  both ends — extraction and execution — so error-text can't smuggle
  shell.
- Nothing auto-applies: every fix is a click. Retry never loops.
- The diagnosing model never equals the failing engine.
- Diagnosis is capped: one Small-role call, short output, parse-or-skip.

## Phases

- **Phase 1 (shipped, v0.42.0):** deterministic classifier in
  `friendly_error`, wired into the chat error rows (`send_message`,
  `send_message_agentic`) so streams and IPC rejections both benefit;
  `ollama serve` / `ollama pull <model>` join the Terminal-fix
  allowlist; error toasts that point at Settings → Models open it on
  click. No model calls.
- **Phase 2 (shipped):** diagnose-and-suggest in `selfheal.rs` —
  `diagnose` runs one capped call on `Ai::diagnosis_engine` (Small role
  unless Small IS the failing stack, then the FM sidecar, else skip),
  over a redacted error + `config_snapshot`; `parse_diagnosis` is
  parse-or-skip with per-action validation (terminal commands re-checked
  against the allowlist, providers against the config); `selfDiagnose`
  toggle in Settings → General, default ON. Renders through the same
  literal grammars the error row already parses, plus
  `` Fix: switch <role> to provider `<id>` `` for the switch button.
- **Phase 3 (shipped):** `settings` tool — `selfheal::settings_get` /
  `settings_set` over the safe `AiConfig` allowlist (chatProvider,
  studioProvider, chatModel/effort/baseUrl per provider, smallModel,
  embedder), reachable from the chat tool router
  (`ToolAction::Settings`), as an MCP tool (`mcp/settings.rs`), and
  from error-row fix buttons (`apply_settings_fix`, which echoes the
  applied change into the transcript as a tool row). `apiKey` is
  neither readable nor writable; reads redact key-shaped values and
  URL credentials; key-shaped values are refused on write.
- **Phase 4 (shipped):** per-reply local fallback from the error row —
  "Answer with Ollama" / "Answer with Apple Intelligence" reruns the
  failed question via the Retry mechanism plus a one-shot
  `provider_override` on `send_message` (resolved through
  `engine_for_provider`; config untouched), offered only when the
  engine's readiness probe answers alive and it isn't the provider
  that just failed.

## Non-goals

- Auto-repair without a click — the user always applies the fix.
- Automatic retry loops or fallbacks that spend money unprompted.
- A general agent with shell access "debugging the machine" — the fix
  surface is the app's own settings plus an allowlisted command set.
