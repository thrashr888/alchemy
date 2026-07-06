# RFC: Audio Overview — local two-host podcast generation

## Summary

Add an **Audio overview** generator to the Studio: one click turns a notebook's
sources into a two-host conversational audio episode, generated and synthesized
**entirely on-device**. This is NotebookLM's signature feature; doing it 100%
locally is a differentiator no cloud product can copy.

## Background

Script generation is already solved in this codebase: `generate_content` budgets
the corpus across sources (waterfill + distillation) and streams from either
Ollama or an OpenAI-compatible gateway. The missing pieces are (1) a dialogue
script format, (2) text-to-speech, (3) audio assembly and storage, (4) playback UI.

## Proposal

### 1. Script generation (works on both providers today)

New artifact kind `audio_overview` in `rag.rs`. The prompt produces a
~800-1200 word dialogue:

```
HOST: <line>
GUEST: <line>
...
```

Two named speakers with distinct roles (curious host, expert guest), an
opening hook, natural handoffs, and a closing summary. The script is saved as
the note's `content` (so it's readable, editable, and rebuildable like any
other note); audio is derived from it.

### 2. TTS engine

| Option | Quality | Size | Integration |
|---|---|---|---|
| macOS `say` (AVSpeechSynthesizer) | OK with downloaded premium voices; robotic with defaults | 0 — built in | `std::process::Command`, zero deps |
| **Kokoro-82M via `ort` (ONNX)** | Very good, near-cloud | ~90 MB model + ~5 MB/voice, one-time download | `ort` crate; same download-on-first-use UX as the built-in embedder |
| Piper | Decent | ~60 MB/voice | C++ sidecar binary to ship & notarize |

**Recommendation:** implement the engine behind a small `Tts` trait.
First implementation is `say` — it proves the whole pipeline with zero
downloads and zero notarization risk. Kokoro via `ort` is the quality engine
behind the same trait, reusing the embedder's existing "download on first use
with progress events" pattern. Piper is rejected: shipping and notarizing
another binary is exactly the CI pain we already fought once with PDFium.

Per-speaker voices: two distinct voices per engine (e.g. `say -v` voice names,
or two Kokoro voice embeddings), configurable later in Settings if wanted.

### 3. Audio assembly & storage

- Synthesize per-line files (voice switches force per-line synthesis anyway).
- Concatenate PCM with `hound` (tiny WAV crate), then encode to `.m4a` with
  macOS's built-in `afconvert`. No ffmpeg dependency.
- Store at `<app-data>/audio/<note_id>.m4a`. Delete alongside the note.
- Emit `audio://progress` events (line N of M) so the UI can show synthesis
  progress; generation is cancellable via the existing `begin_generation` scope.

### 4. Playback UI

- Note kind `audio_overview` renders in the NoteViewer with an `<audio controls>`
  element (via `convertFileSrc` + asset-protocol scope for the audio dir) above
  the readable script.
- The note card gets a duration badge. Rebuild regenerates script + audio.

## Rationale & alternatives considered

- **Mermaid-style "let the model emit SSML/audio markup"** — rejected; same
  reason as the mind map: small local models break structured formats. Plain
  `HOST:`/`GUEST:` line format is nearly unbreakable, and the parser can skip
  malformed lines.
- **Cloud TTS via the gateway** — rejected as the primary path; it breaks the
  "nothing leaves your laptop" pitch. Could become an optional third engine.
- **Streaming playback while synthesizing** — deferred; complicates storage
  and cancellation for little gain on a 3-6 minute episode.

## Downsides & risks

- `say` default voices sound dated; the feature's wow factor depends on
  getting Kokoro in. Mitigation: trait boundary makes the swap mechanical.
- Long corpora → long scripts → minutes of synthesis. Mitigation: progress
  events + cancellation, and the script length is capped by the prompt.
- macOS-only assembly (`afconvert`, `say`). Acceptable: the app is currently
  built and notarized for macOS only; the `Tts` trait keeps the door open.

## Open questions

1. Ship `say` first and follow with Kokoro, or hold the feature until Kokoro
   is in so first impressions are strong?
2. Episode length target (default ~5 minutes?) and whether "Add instructions"
   should steer tone/length like other generators (proposed: yes, it's free).
3. Voice pair defaults per engine.

## Decision

Pending review.
