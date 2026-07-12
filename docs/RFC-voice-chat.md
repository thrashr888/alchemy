# RFC: Voice — one STT foundation, two features

## Summary

Add local speech-to-text via **whisper.cpp**, built once as a shared module
that unlocks two features in order: **audio/video files as sources**
(transcribe → ingest through the normal pipeline) and **voice chat** (talk to
the notebook, get short spoken answers via the Kokoro voices we already
ship). Everything runs on-device, matching the app's local-first line.

## Background

- TTS already exists: Kokoro-82M via ONNX (`tts.rs`), with a proven
  model-download UX (progress events, Settings → Models section,
  ~93 MB, verify-before-show).
- NotebookLM's file-type list is matched except audio/video — the missing
  formats (mp3, m4a, wav, aiff, mp4, …) all reduce to "get PCM, transcribe".
- macOS ships the decoders: `afconvert` for CoreAudio formats,
  `avconvert` for extracting audio tracks from video. Only ogg/opus need a
  Rust decoder crate (Symphonia) — or get cut from v1.
- whisper.cpp runs well on Apple Silicon via Metal; `whisper-rs` bindings
  are mature. `base.en`/`small` models are 74–244 MB — same order as Kokoro.

## Proposal

### 1. Shared foundation: `stt.rs`

Mirror `tts.rs`:

- `SttEngine` wrapping whisper-rs; model file in the app data dir next to
  Kokoro's; download-on-first-use with the existing `embedder://progress`-
  style events; status surfaced in Settings → Models ("Transcription").
- One entry point: `transcribe(path: &Path) -> Result<Transcript>` where
  `Transcript` carries text plus segment timestamps.
- Decode step before whisper: `afconvert -f WAVE -d LEI16@16000 -c 1` for
  audio; `avconvert --preset PresetAppleM4A` first for video containers.
  Both are stock macOS binaries — no bundled ffmpeg.
- Model choice in Settings (base ≈ fast / small ≈ better), default `base`.

### 2. Feature A: audio & video sources

- Extend the ingest dispatch (`extract_any_file`): audio/video extensions →
  `stt::transcribe` → `Extracted { source_type: "audio", text }` with
  `## [mm:ss]` timestamp headings every segment group, so citations can
  point into the recording.
- Folder scans and drag-drop inherit support automatically (same extension
  allowlists as everything else).
- Transcription is minutes-long for long files: run through the existing
  ingest queue with per-file progress (the `folder://progress` pattern);
  never inside a UI-blocking call.
- Formats v1: mp3, m4a, aac, wav, aiff/aif/aifc, mp4, mov, 3gp. Defer:
  ogg/opus (Symphonia), avi/mpeg (AVFoundation support is spotty), and the
  résumé-padding formats (mid, cda, ra, wma) permanently.

### 3. Feature B: voice chat

Interaction model — **push-to-talk, half-duplex** in v1:

- Mic button in the chat composer; hold (or tap-to-toggle) to talk.
  Release → whisper transcribes the utterance → the text lands in the
  composer and sends through the normal chat path (RAG, citations, history —
  no separate voice pipeline).
- The answer streams as text like today; simultaneously, completed sentences
  are spoken through a "brief spoken answer" register: a system-prompt
  addition when voice-initiated ("answer in 2–3 conversational sentences;
  details stay on screen"). Kokoro synthesizes per-sentence using the
  existing chunked-synthesis path, so speech starts before the full answer
  finishes.
- A speaking indicator with a stop button; sending a new utterance stops
  playback (barge-in by button, not by VAD).

Explicitly deferred: wake words, continuous listening/VAD, full-duplex
interruption, voice-only mode. Those are products, not features; push-to-talk
delivers the value with a fraction of the failure modes.

Latency budget (Apple Silicon, base model): ~0.5–1.5 s transcription for a
10 s utterance + first-token time + ~0.5 s first Kokoro sentence ≈
**2–3 s to first spoken word**. Acceptable for v1; `small` model users trade
a second for accuracy.

### 4. Privacy & failure notes

- Mic audio never leaves the process; transcription is local regardless of
  chat provider. The transcribed *text* goes wherever chat goes (gateway
  warning already exists).
- macOS mic permission: one TCC prompt, triggered by the first mic-button
  press, usage string in Info.plist.
- No STT model downloaded → mic button hidden (Kokoro precedent: absent
  until verified).

### 5. Phasing

1. `stt.rs` + Settings/Models wiring + model download.
2. Audio/video sources through ingest (ships NotebookLM parity).
3. Push-to-talk voice chat with spoken brief answers.
4. (Later) VAD auto-stop, voice for Studio ("read this briefing to me" —
   trivial: Kokoro over any note), ogg/opus via Symphonia.

## Open questions

- whisper-rs + Metal in a notarized bundle: any signing friction with the
  ggml Metal shaders? (Prototype phase 1 first.)
- One shared "Voice" Settings section for Kokoro + whisper, or keep them as
  separate Models rows? (Lean: one "Voice" section, two model rows.)
- Should voice-initiated turns be marked in the transcript (🎙 badge)?
  Cheap, probably yes — useful when reviewing why an answer was terse.
