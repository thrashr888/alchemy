# RFC: inference providers — three families, one router

Alchemy's inference story is at a fork. Bob's gateway died by vendor
policy, pushing everything onto local Ollama — and the laptop feels it.
macOS 27 just opened Foundation Models to third-party backends. Agent CLIs
(claude, codex, opencode) carry paid subscriptions that no API key can
reach. And most prospective users have none of the above installed — not
even Ollama. The naive path is a new `AiConfig` field per option until the
Models tab has fifty setups. This RFC takes the other path: **providers
collapse into three adapter families, jobs declare roles, and a
capability-detected router assigns one to the other.** User choice stays
(a preference order, per-role overrides); user *plumbing* goes.

The design is built in-tree as a cleanly bounded module with an eye to
extraction — a reusable crate has real potential (nothing on crates.io
does agent-CLI providers or role routing), but an abstraction extracted
before its second consumer exists gets the API wrong. Alchemy beats on it
first; QDOS or rust-helper pulls it out later, mechanically.

## 1. The landscape collapses

Sort providers not by vendor but by **how the credential travels**:

| Family | Members | Credential |
|---|---|---|
| **Local engines** | builtin embedder (model2vec, shipped), **builtin chat via mlx-rs**, Ollama, Apple Foundation Models (macOS 27) | none — the machine |
| **Agent CLIs** | claude (Max), codex (ChatGPT Pro), opencode, hermes, copilot, gemini-cli | the vendor's own CLI holds the subscription |
| **HTTP gateways** | any OpenAI-compatible URL+key (Gemini API, OpenRouter, …) | API key |

Paul's nine daily providers all land in these three. The insight that
makes family B load-bearing: consumer subscriptions (Claude Max, ChatGPT
Pro, Copilot) have **no API key at all** — the sanctioned CLI *is* the
credential. It's the same fact that killed Bob's key while bobshell kept
working. One adapter family absorbs six providers.

## 2. Roles, not models

Every inference call site in the app today declares (implicitly) what it
needs. Make it explicit — jobs request a **role**; the router picks the
provider:

| Role | Call sites today | Needs | Default ladder |
|---|---|---|---|
| `Chat` | send_message, meta-chat | streaming, quality, citations | pinned → agent CLI → Ollama → builtin MLX → gateway |
| `Agent` | deep research, curator consolidation | tool use, multi-step | agent CLI → Chat's provider |
| `Generate` | studio artifacts, reports, audio scripts | long-form quality | same as Chat |
| `Small` | friendly_title, summaries, router hints | speed, cheap | builtin MLX / FM on-device → Ollama → Chat's provider |
| `Embed` | ingest, retrieval, semantic router | consistency (index-coupled) | **builtin always**, unless user opted into Ollama |
| `Vision` | image OCR, scanned PDFs | image input | Ollama vision → gateway vision → skip (today's behavior) |

TTS stays fixed on Kokoro. `Embed` never routes dynamically — vectors are
coupled to the index, and switching embedders already has its own
deliberate re-embed flow.

**Streaming is a trait invariant, not a feature.** Every chat-shaped role
streams to the user on every provider: mlx-lm generates token-by-token,
Ollama and gateways speak SSE, agent CLIs emit incremental stream-json.
`Engine::chat` returns a stream or the engine doesn't ship — there is no
blocking variant in the trait (Argos's `--output-format text` mode is
bootstrap prior art, not an output mode we accept).

**Retrieval tunes to the resolved model, not just the query.** A 4B model
at 16 GB and a 120B MoE at 128 GB shouldn't receive the same context: the
small model needs fewer, better-ranked passages in a tight budget; the
big one can afford breadth. So the router resolves not just a provider
but a **context profile** — retrieval k, per-source diversity caps,
assembled-context char budget (today's fixed constants in rag.rs become
profile fields) — attached per model tier. The eval suite
(evals.rs / retrieval_eval.rs) runs **per tier × per profile**, and
"decent results for everything" is the gate: a tier ships when its
profile holds eval quality, and the 16 GB messaging is whatever the
numbers earn.

The immediate performance win hides in `Small`: titles and summaries
currently queue behind the big chat model on Ollama. Routing them to a
builtin 4B-class model (or the free FM on-device model) clears most of
the ingest latency this laptop feels.

## 3. The boundary

A `src-tauri/src/inference/` module (workspace-crate-shaped: no Tauri
imports, no app types):

```rust
pub enum Role { Chat, Agent, Generate, Small, Embed, Vision }

pub trait Engine: Send + Sync {
    fn id(&self) -> ProviderId;
    fn probe(&self) -> Availability;          // Missing | Installed | Ready(detail)
    fn supports(&self, role: Role) -> bool;
    async fn chat(&self, req: ChatRequest) -> Result<ChatStream>;  // streaming events
}

pub trait EmbedEngine: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

pub struct Router { /* prefs + probed availability → per-role assignment */ }
```

`ChatStream` yields normalized events — `Text`, `ToolUse`, `ToolResult`,
`Usage`, `Done` — which is exactly the shape the chat UI already renders
(bubbles + quiet tool rows). The current `Ai` struct becomes the module's
first consumer and keeps its public face (`ai.chat`, `ai.embed`) during
migration. Extraction trigger, stated now so it isn't relitigated: **a
second consumer in another repo.** Until then it's a module, not a crate.

## 4. Family A — local engines

- **Builtin embedder** — shipped (model2vec potion-base-8M). The
  download-on-first-use pattern here and in Kokoro is the template.
- **Builtin chat via MLX** — the new piece, and the zero-setup story.
  *Ecosystem reality check (2026-07-20, phase-2 implementation):* mlx-rs
  0.25.3 (bindings) is solid, but the generation layer above it is not
  there yet — upstream `mlx-lm`'s generate module is commented out, and
  the greenfield `mlx-lm-rs` (0.0.1) streams real Qwen3 inference but
  **bf16 only, no quantized models** — ~8 GB of weights for a 4B model,
  which defeats the 16 GB tier this engine exists to serve. So the
  builtin-MLX rung *waits for quantized support* (or we contribute it
  upstream — a well-shaped, high-leverage OSS contribution that fits the
  fix-upstream ethos), and **phase 2's vehicle becomes the first-party
  path below**, whose system model is quantized, pre-downloaded, and
  Apple-managed. Two rules keep MLX from forking the ecosystem further
  when it does land:
  - **The model store is the shared Hugging Face hub cache**
    (`HF_HOME`, `~/.cache/huggingface/hub`) — never an Alchemy-private
    folder. mlx-community models are HF repos; mlx-lm and the
    swift-transformers tooling already resolve there. If another app
    (or the user's own mlx-lm) has pulled the model, Alchemy reuses it;
    what Alchemy pulls, everything HF-aware reuses. We will not be the
    fifth copy of Qwen on disk. (LM Studio/Jan/Ollama keep private
    stores — that's their fork, not one we join.) Verify at build time
    exactly which cache path Apple's stack reads and match it.
  - **First-party convergence**: on macOS 27+, prefer the Foundation
    Models path (`MLXLanguageModel` via a small Swift sidecar — there's
    no Rust surface) so Apple owns the model lifecycle, and PCC/Core AI
    arrive on the same API; mlx-rs in-process is the rung for
    pre-Golden-Gate Apple silicon, reading the same shared cache. One
    model on disk, two loaders, converging to Apple's as OS adoption
    catches up.
- **RAM-tiered defaults** — one default model is wrong when the fleet
  spans 16 GB to 128 GB. Detect physical RAM and tier the default:
  ~16 GB → 4B-class 4-bit (honest messaging: local will be serviceable,
  not spectacular); ~36 GB → 8–14B class; 64 GB+ → big MoE class
  (gpt-oss-120b-4bit fits in 128 GB; 30B-A3B class at 64 GB) — exploit
  the hardware the user actually has. The 16 GB story has a testable
  out: Alchemy's retrieval quality and deliberately small contexts may
  make 4B genuinely sufficient — run the existing eval suite
  (evals.rs / retrieval_eval.rs) per tier and let the numbers say so
  before the messaging does.
- **Vision and audio on MLX too** — the same engine family should grow
  `Vision` (mlx-vlm-class models for OCR, replacing the Ollama-vision
  dependency) and STT for the voice-chat RFC (whisper-class on MLX), so
  the pure-local story has no daemon dependencies anywhere. Phased
  behind text, same shared-cache rule.
- **Ollama** — unchanged, stays the power-user local choice (any model,
  any size, own scheduler, own store).

## 5. Family B — agent CLIs

Shell out to the agent headless and speak its structured stream —
**never embed a terminal**. This family is grounded in two shipped
wrappers of Paul's (audited 2026-07-20): **Argos**
(`crates/argos-core/src/claude_cli.rs` — direct one-shot CLI) and
**tradr** (`app/src-tauri/src/commands/agent.rs` + `tradr/cli/agent.py`
— long-lived streaming session via the claude-agent-sdk). Neither parses
`stream-json` directly (Argos uses `--output-format text`; tradr lets
the SDK own the wire) — so the stream-json decoder is acknowledged new
work; everything around it is proven code to port.

- **Bootstrap (from Argos)**: `find_claude_binary()`-style discovery
  (`~/.local/bin`, Homebrew paths, then `$SHELL -l -c "which …"`);
  `load_shell_env()` — macOS GUI apps don't inherit dotfile exports, so
  build the child env from a login shell; install-hint baked into the
  not-found error; `tokio::timeout` around the call — **plus the fix
  for Argos's one gap**: `kill_on_drop(true)` so a timed-out child
  can't linger.
- **Auth (both repos agree — the family's load-bearing rule)**:
  `env_clear()` and **strip `ANTHROPIC_API_KEY`** so the CLI uses its
  own OAuth/Max session — the subscription token is non-transferable
  and a stray API key conflicts with it. No secrets stored, ever.
- **Invocation**: v1 is **Argos's lifecycle with streaming output** —
  one process per message, `claude -p --output-format stream-json`
  (codex: `codex exec --json`), context replayed in the prompt exactly
  as Argos's chat does and as Alchemy's RAG does anyway. The long-lived
  tradr-style client (interrupt support, persistent session) is the
  `Agent`-role upgrade, not the v1 base.
- **Event architecture (from tradr)**: a typed event enum
  (`Ready | Text | ToolUse | ToolResult | Done{cost} | Error`,
  `#[serde(tag, content)]`), streamed to the UI over a Tauri Channel;
  text chunks append to the trailing bubble part, tool events render as
  the existing quiet tool rows, `Done`'s cost lands in the message
  footer. Port tradr's scars wholesale: stderr drained on a background
  task (pipe-deadlock), `Error` events don't terminate the read loop (a
  `done` may follow), `Value::to_string()` double-quotes strings —
  `as_str()` first, and stale "running" tool rows normalize to done on
  reload.
- **The loop is pre-built**: these CLIs already have Alchemy registered
  as an MCP server (connectors.rs). A spawned agent connects back and
  retrieves with `search`, `grep_sources`, `ast_search` — grounded in
  the notebook by our own tools.
- **Containment (tradr's model)**: explicit `allowed_tools` pinned to
  the MCP tool set — no filesystem, no Bash — plus tradr's budget rails
  (`max_turns`, `max_budget_usd`) surfaced as router config. And its
  best pattern, kept for anything side-effectful: **the agent stages,
  only the human commits.**
- **Detection**: binary + version probe, surfaced like cider
  ("claude · signed in" / "codex · not installed").

Embeddings never come from agents. Bob stays out entirely — its block is
vendor policy, and bobshell-wrapping would be evading it.

## 6. Family C — gateways

Today's OpenAI-compatible config, unchanged: base URL + key + model
pickers. Gemini API, OpenRouter, LM Studio, and anything else
OpenAI-shaped lives here. No per-vendor SDKs.

## 7. The router

- **Probing**: at startup and on Settings-open, probe availability —
  binary checks and one-RTT HTTP pings, cached with a short TTL. Never
  block a chat on probing; stale-good beats slow.
- **Assignment**: per-role ladders (§2 table) filtered by availability,
  reordered by the user's preference list, overridable per role in
  advanced settings. Zero-config result on a bare Mac: builtin embedder +
  builtin MLX chat — everything works, nothing was configured.
- **Failure fallthrough**: a provider error at call time drops to the
  next rung with one toast ("Ollama unreachable — answered with Claude
  Code"). No modal, no dead chat.
- **Telemetry**: per-provider tok/s already exists (`ModelStat`); the
  router records it per rung so "why is this slow" has an answer and the
  ladder can be tuned with evidence.

## 8. Settings

The Models tab becomes a **provider list, not a form**: each detected
provider as a row with its availability chip and its models; undetected
ones collapsed under "available if installed" with a one-line hint. One
drag-ordered preference list for Chat; per-role overrides behind an
"Advanced routing" disclosure. The fifty-setups failure mode is dead on
arrival because setup is detection, not entry.

## 9. Migration

`AiConfig.provider`/`embedder`/model fields stay serde-compatible and
seed the router's preference list on first launch (ollama-primary users
see zero change). No index migration — embeddings are untouched unless
the user changes embedders, which keeps its existing deliberate re-embed
flow.

## 10. Alternatives considered

| Option | Verdict |
|---|---|
| **Extract the crate now** | No second consumer yet; API would fossilize guesses. Module-with-clean-boundary now, crate at second consumer. |
| **genai / rig** | Good gateway multiplexers; no agent-CLI family, no role routing, no local engines. The novel 2/3 of this design is exactly what they lack. |
| **llama.cpp / mistral.rs in-process** | Viable builtin-engine alternative and cross-platform — but this app is macOS-first on Apple silicon, where MLX is the native answer (and Paul's stack). Revisit if Windows/Linux ever matters. |
| **Foundation Models sidecar as the foundation** | macOS 27-only + helper process for what mlx-rs does in-process on any Apple-silicon macOS. It's a later rung, not the base. |
| **Local proxy daemon (LiteLLM-style)** | A second process to manage and another 50-setups surface. The router *is* the proxy, in-process. |
| **claude-agent-sdk sidecar (tradr's bet)** | Proven in tradr — but it drags a Python/uv runtime into a pure-Rust app and only covers Claude; codex/opencode need direct adapters regardless. Parse stream-json in Rust instead; if that wall is ever real, tradr proves the SDK fallback works. |
| **Status quo (one provider dropdown)** | Already creaking at two families; breaks outright at three. |

## 11. Phases

1. **The seam** — `inference/` module, `Engine`/`Router`/roles, existing
   providers (builtin embed, Ollama, gateway) rehomed behind it. Pure
   refactor: zero behavior change, `AiConfig` seeds prefs. Proves the
   boundary before anything new plugs in.
2. **Foundation Models sidecar for `Small`** *(reordered on evidence —
   see §4's ecosystem check)*: a zero-dependency Swift sidecar speaking
   NDJSON over stdio to the on-device `SystemLanguageModel` (base API is
   macOS **26**+, wider than Golden Gate), `FmEngine` in `inference/`
   with probe + streaming + fallthrough, and `Role::Small` routed to it
   (`friendly_title` first — the ingest-latency hot path). Zero download:
   the system model is already on disk. Measure ingest latency before/
   after. **2b, gated on upstream quantization**: the mlx-rs builtin
   engine as originally scoped — still the zero-setup story for
   pre-macOS-26 and non-Apple-Intelligence Macs.
3. **Agent CLIs** — claude + codex adapters first (the two with proven
   wrappings), event normalization, allowlist containment, provider
   attribution in the message footer. opencode/hermes/copilot/gemini-cli
   follow as detection rows, cheap once the family exists.
4. **Foundation Models sidecar** — *(shipped for Small + On-this-Mac
   chat with the k=4 profile; eval_context_profiles gates the tight
   profile at ≥75% recall retention — 100% on current datasets. The PCC
   rung is blocked on the macOS 27 SDK: typecheck probes confirm SDK
   26.5 exposes only the base SystemLanguageModel API — revisit at the
   next Xcode.)* — the first-party convergence path:
   `MLXLanguageModel` for the same shared-cache models, the free
   on-device model for `Small`, PCC as an opt-in rung. macOS 27-gated
   with fallbacks (per the Golden Gate policy: ship gated, don't wait
   for GA). Vision/STT MLX rungs follow here too.
5. **Extraction** — when a second repo consumes it, split the crate and
   take the API lessons with it.

## Explicitly skipped

- **An embedded terminal** — never. Structured streams only; that's the
  whole reason family B is tractable inside a notebook app.
- **Bob workarounds** — vendor policy, respected. If IBM ships a
  sanctioned third-party route it slots into family C in an afternoon.
- **Image generation routing** — still blocked upstream on local model
  quality (backlog P11); the router grows a role when that unblocks.
- **TTS routing** — Kokoro is the voice; not a preference surface.
- **Keychain for gateway keys** — real, pre-existing, and orthogonal;
  bundling it here would smuggle a security migration into a routing
  refactor.

## Open questions

- The RAM-tier cut points and per-tier model picks (4B / 8–14B / big
  MoE) — and the eval evidence for the 16 GB claim: do our retrieval
  quality and small contexts make 4B genuinely sufficient? The eval
  suite decides, per tier.
- Exact cache-path compatibility between mlx-rs downloads and Apple's
  swift-transformers/Foundation Models resolution — verify one shared
  path in phase 2, before any model ships.
- Agent containment: MCP-tools-only, or also the repo grep/ast tools for
  code notebooks? (Leaning MCP-only in v1 — the MCP server already
  exposes grep_sources/ast_search, so the capability arrives anyway.)
- Session continuity for agent chat: one-shot per message (v1, the
  Argos pattern) vs a long-lived tradr-style client for the `Agent`
  role — does context reconstruction suffice at long conversation
  lengths, and when does interrupt-during-stream justify the two-lock
  lifecycle?
- Does `Vision` grow an MLX rung (mlx-vlm-class models) so OCR also
  works with nothing installed, or stay Ollama/gateway-only?
- Per-notebook provider pins (a research notebook pinned to PCC for
  privacy, a code notebook pinned to claude)? Defer unless asked-for.
