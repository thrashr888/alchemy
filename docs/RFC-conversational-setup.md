# RFC: Conversational setup — the settings tool grows onboarding verbs

Status: accepted · Builds on: RFC-self-resolve (the `settings` tool, its
allowlist discipline, the fix-action grammars) · Surfaces: chat tool
router + MCP `settings` tool, identically.

## Problem

The `settings` tool made config conversational, but only for config that
already works. A fresh install faces walls the tool can't address: no
model pulled yet, no way to trust a model before committing to it, a
personalization profile only the Settings dialog can write, agent
integration behind one button in one tab, and no narrative connecting
any of it. Post-install, the user's first questions are "what can answer
me, how do I get one, and is it any good?" — all currently unanswerable
from chat, and all invisible to agents (an agent asked to "set up
Alchemy" today can rename notebooks but can't pull a model).

## Design

Five verb groups, added to the same allowlist the settings tool already
enforces. Everything is readable and writable from BOTH surfaces (chat
router + MCP) unless marked otherwise; every mutation echoes a
transcript row exactly like the shipped `set` verb. Nothing here touches
secrets — the RFC-self-resolve refusals stay load-bearing and their
tests extend to each new verb.

### 1. Models: `test`, `pull`, `models`

- `models` — read: installed Ollama models (the OllamaModelPicker's
  list) plus each configured provider's active model and readiness, one
  compact roster.
- `test <provider|model>` — run one tiny chat ("Reply with OK") and,
  when the target can embed, one embed call; report alive/failed,
  first-token latency, and total time as a transcript row. This is
  `provider_readiness` grown into evidence. No config change.
- `pull <model>` — Ollama only. Reuses the existing charset-gated
  (`[A-Za-z0-9._:/-]`, ≤64) allowlisted-terminal machinery — the verb
  renders the same one-click Terminal affordance the error rows use;
  the app never shells out on its own. After a pull the natural follow
  is `test`, and the guided flow (§5) chains them.

### 2. Personalization: `profile`, `style`

- `profile` — get/set over `UserProfile` (name, profession, standing
  instructions). Every prompt already carries this via `persona_block`;
  this makes "call me Paul, keep answers short" a one-liner on day one.
  Free-text field, but never key-shaped values (same scrub as `set`).
- `style` — per-notebook `ChatConfig` style and length ("use the Google
  style here", "shorter answers in this notebook"). Chat-side this
  needs a notebook in scope; the MCP tool takes an explicit
  `notebook_id`.

### 3. Appearance: `theme`

Get/list/set over the theme roster in `themes.ts` (id or fuzzy label —
"the dark rust one"). Purely cosmetic, instantly visible, and the kind
of first-session delight that teaches the tool exists. Other toggles
(sounds, diagnosis) deliberately stay out of the allowlist for now —
settings sprawl is how a safe tool becomes an audit problem.

### 4. Agents: `connect`

- Read: which installed agent clients exist and which are already
  registered (what `connectors.rs` knows).
- Write: register the MCP server + `skills/alchemy` with a named client
  ("install alchemy into claude code"). This writes ANOTHER app's
  config, so it is the one verb that never auto-applies from either
  surface: chat renders a confirm-click (the fix-button pattern), and
  the MCP tool requires `confirm: true` in the call. The echo names the
  file it touched.

### 5. The guided flow: `setup`

Not a wizard window — a chat behavior. "Help me get set up" routes to a
`setup` verb that inspects state and answers with the next unmet step,
each rendered through the existing button grammars:

1. Provider alive? (readiness probe; if Ollama is installed but down —
   the `ollama serve` Terminal affordance; if no local engine exists,
   offer Apple FM or a gateway walkthrough)
2. Chat model present? (`pull` a starter, then `test` it)
3. Embedder chosen? (default builtin is fine — say so and move on)
4. Profile filled? (`profile` prompt-back)
5. Agents connected? (`connect` confirm-click, or skip)

Each invocation reports state and offers ONE next action — never a
checklist dump, never auto-advancing. Re-invoking after each click
walks the whole path. Because every step is a settings-tool verb, an
agent driving the MCP surface can perform the identical setup
headlessly (minus `connect`'s confirm, which it must pass explicitly).

## Discipline

Inherited whole from RFC-self-resolve: no secrets readable or writable,
terminal commands allowlisted + charset-gated at both ends, nothing
auto-applies, every mutation echoes. New here: `connect` is
confirm-only on both surfaces; `test` calls are capped (one chat + one
embed, short timeout) so a looping agent can't turn probing into spend;
`pull` never executes — it renders the click.

## Phases

- **Phase 1 (shipped):** model verbs — `models`, `test`, `pull` — on
  both surfaces (`ToolAction::Settings` ops in the chat router, the MCP
  `settings` tool's new ops). `test` is capped by construction at one
  chat + one embed under short timeouts; `pull` stages the charset-gated
  command through the error rows' Terminal grammar (MCP returns the
  validated command string); the refusal/redaction discipline extends to
  every new verb. A deterministic `settings_gate` fast path also routes
  tight imperative settings shapes in BOTH chat modes, since deep
  research never reaches the LLM router.
- **Phase 2:** `profile`, `style`, `theme`.
- **Phase 3:** `connect`, both surfaces, confirm-gated.
- **Phase 4:** router awareness for the already-shipped template and
  schedule tools ("make a weekly brief of this notebook" →
  `schedule_report`) — no new surface, just routing.
- **Phase 5:** `setup`, composing 1–4.

## Non-goals

Editing API keys or provider auth from chat (Settings dialog only,
forever). A separate onboarding window or checklist UI. Auto-pulling
models on install. Multi-step agent "autonomy" — the flow is
click-per-step by design.
