//! Text-to-speech for Audio Overview episodes.
//!
//! The generator writes a `HOST:`/`GUEST:` dialogue script (a format even
//! small local models produce reliably); this module parses it, synthesizes
//! each line with a per-speaker voice, and assembles one `.m4a` episode.
//!
//! The engine is Kokoro-82M via ONNX — near-cloud quality, fully on-device,
//! downloaded on first use (see docs/RFC-audio-overview.md). There is
//! deliberately no lower-quality fallback: a robotic episode is worse than
//! a clear "model unavailable" error.

use anyhow::{Context, Result};
use futures::FutureExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Host,
    Guest,
}

#[derive(Debug, Clone)]
pub struct ScriptLine {
    pub speaker: Speaker,
    pub text: String,
}

/// Parse a dialogue script into lines. Tolerant of the decorations models
/// sneak in (`**HOST:**`, `Host —`, lowercase); anything that isn't a
/// speaker line (headings, blanks, stage directions) is skipped.
pub fn parse_script(content: &str) -> Vec<ScriptLine> {
    let mut lines = Vec::new();
    for raw in content.lines() {
        let stripped = raw.trim().trim_start_matches(['*', '#', '-', '>', ' ']);
        let lower = stripped.to_lowercase();
        let (speaker, rest) = if let Some(rest) = lower.strip_prefix("host") {
            (Speaker::Host, &stripped[stripped.len() - rest.len()..])
        } else if let Some(rest) = lower.strip_prefix("guest") {
            (Speaker::Guest, &stripped[stripped.len() - rest.len()..])
        } else {
            continue;
        };
        // Require a separator right after the name so prose that merely
        // starts with the word "host" isn't misread as a cue.
        let text = rest.trim_start_matches(['*', ':', '—', '-', ' ']);
        if text.len() == rest.trim_start().len() || text.is_empty() {
            continue;
        }
        let text = text.replace(['*', '_', '`'], "");
        lines.push(ScriptLine {
            speaker,
            text: text.trim().to_string(),
        });
    }
    lines
}

// ---- Kokoro engine ----------------------------------------------------------

const KOKORO_REPO: &str = "onnx-community/Kokoro-82M-v1.0-ONNX";
const KOKORO_MODEL: &str = "model_quantized.onnx";
/// The default voice pair: warm US female host, US male guest.
pub const HOST_VOICE: &str = "af_heart";
pub const GUEST_VOICE: &str = "am_michael";

pub type DownloadProgress = std::sync::Arc<dyn Fn(&str, u64, u64) + Send + Sync>;

/// True when the model and both voice packs are on disk.
pub fn kokoro_files_present(dir: &Path) -> bool {
    let has = |p: PathBuf| std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false);
    has(dir.join(KOKORO_MODEL))
        && has(dir.join("voices").join(format!("{HOST_VOICE}.bin")))
        && has(dir.join("voices").join(format!("{GUEST_VOICE}.bin")))
}

/// Download the Kokoro model (~92 MB int8 ONNX) and the two voice packs into
/// `dir` if missing — same shape as the built-in embedder: stream to a
/// `.part` file, rename on completion, report byte progress per file.
pub async fn ensure_kokoro_files(
    dir: &Path,
    progress: Option<&DownloadProgress>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let voices = dir.join("voices");
    tokio::fs::create_dir_all(&voices).await.ok();
    let files: [(String, PathBuf); 3] = [
        (format!("onnx/{KOKORO_MODEL}"), dir.join(KOKORO_MODEL)),
        (
            format!("voices/{HOST_VOICE}.bin"),
            voices.join(format!("{HOST_VOICE}.bin")),
        ),
        (
            format!("voices/{GUEST_VOICE}.bin"),
            voices.join(format!("{GUEST_VOICE}.bin")),
        ),
    ];
    let http = reqwest::Client::new();
    for (remote, dest) in files {
        if tokio::fs::metadata(&dest)
            .await
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            continue;
        }
        let label = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let url = format!("https://huggingface.co/{KOKORO_REPO}/resolve/main/{remote}");
        let resp = http
            .get(&url)
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .with_context(|| {
                format!(
                    "Downloading the Audio Overview voices failed ({label}); check \
                     your network/proxy access to huggingface.co"
                )
            })?;
        anyhow::ensure!(
            resp.status().is_success(),
            "voice model download {label}: HTTP {}",
            resp.status()
        );
        let total = resp.content_length().unwrap_or(0);
        let tmp = dest.with_extension("part");
        let mut out = tokio::fs::File::create(&tmp)
            .await
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        let mut done: u64 = 0;
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            anyhow::ensure!(!cancel.is_cancelled(), "Generation stopped.");
            let bytes = chunk.context("voice model download interrupted")?;
            out.write_all(&bytes).await?;
            done += bytes.len() as u64;
            if let Some(cb) = progress {
                cb(&label, done, total);
            }
        }
        out.flush().await?;
        drop(out);
        tokio::fs::rename(&tmp, &dest).await?;
        if let Some(cb) = progress {
            cb(&label, total.max(done), total.max(done));
        }
    }
    Ok(())
}

/// Stay comfortably under Kokoro's 510-phoneme ceiling.
///
/// The budget is in estimated phonemes, not characters. Counting characters
/// was the original bug: phonemes track characters closely for ordinary
/// prose and not at all for what a Brief is made of, since a phonemizer
/// speaks "2026" as "twenty twenty six" and "%" as "percent". An
/// expansion-heavy line therefore clears 510 phonemes while still looking
/// short, and kokoro-en indexes past its style pack and panics.
///
/// 240 is main's figure, kept: for prose a phoneme costs about a character,
/// so ordinary lines break where they did before rather than into
/// noticeably more breaths. Dense lines now break earlier, which is the
/// point.
const MAX_SYNTH_COST: usize = 240;

/// Rough phoneme count for a stretch of text.
///
/// The earlier version of this budget counted characters, on the assumption
/// that phonemes track characters closely in English. They do for prose —
/// and not at all for the things a Brief is full of. A phonemizer speaks
/// "2026" as "twenty twenty six" and "%" as "percent", so a date- and
/// figure-heavy line blows through 510 phonemes while still looking like a
/// short line, and kokoro-en indexes past its style pack and panics. Hence
/// weights: a letter is worth about one phoneme, a digit about four, and the
/// symbols that expand into whole words rather more.
///
/// Deliberately an over-estimate. Guessing high costs a slightly earlier
/// split, which nobody can hear; guessing low panics inside a dependency.
pub(crate) fn speech_cost(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '0'..='9' => 4,
            '%' | '$' | '£' | '€' | '#' | '=' | '+' | '@' | '&' | '/' => 6,
            _ => 1,
        })
        .sum()
}

/// Break one over-budget word on character boundaries. Nothing about this
/// sounds good — but it is the difference between a garbled second of audio
/// and a panic that loses the whole Brief.
fn split_word(word: &str, max_cost: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in word.chars() {
        if !cur.is_empty() && speech_cost(&cur) + speech_cost(&ch.to_string()) > max_cost {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Split a dialogue line into chunks short enough for Kokoro: pack whole
/// sentences up to the cap; hard-split on whitespace only when a single
/// sentence alone exceeds it.
pub(crate) fn split_for_synthesis(text: &str, max_cost: usize) -> Vec<String> {
    let text = text.trim();
    if speech_cost(text) <= max_cost {
        return vec![text.to_string()];
    }
    let mut sentences: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '…' | ';') {
            sentences.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        sentences.push(cur);
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for s in sentences {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if speech_cost(s) > max_cost {
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            let mut piece = String::new();
            for w in s.split_whitespace() {
                // A single "word" can outrun the budget alone — a long digit
                // run, a hash, an identifier. Splitting on whitespace can't
                // help there, so break it on characters rather than emit the
                // one chunk guaranteed to panic the model.
                if speech_cost(w) > max_cost {
                    if !piece.is_empty() {
                        chunks.push(std::mem::take(&mut piece));
                    }
                    chunks.extend(split_word(w, max_cost));
                    continue;
                }
                if !piece.is_empty() && speech_cost(&piece) + 1 + speech_cost(w) > max_cost {
                    chunks.push(std::mem::take(&mut piece));
                }
                if !piece.is_empty() {
                    piece.push(' ');
                }
                piece.push_str(w);
            }
            if !piece.is_empty() {
                chunks.push(piece);
            }
            continue;
        }
        if !cur.is_empty() && speech_cost(&cur) + 1 + speech_cost(s) > max_cost {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(s);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// The real ceiling, in phoneme characters.
///
/// kokoro-en tokenizes a chunk as `chars + 2` — one leading and one trailing
/// marker — and then indexes its 510-entry style pack by that token count
/// (`synth_v10`, kokoro-en 0.1.5). 508 phoneme characters is therefore the
/// largest input that cannot run off the end of the pack. The crate chunks
/// its own phonemes at 510, which is the off-by-two that panics: a full
/// 510-character chunk becomes 512 tokens and asks for `pack[511]`.
const MAX_PHONEME_CHARS: usize = 508;

/// How deep the backstop will keep halving before it gives up and lets the
/// panic guard at the call site have it. Ten halvings turn any real line
/// into single words.
const MAX_SPLIT_DEPTH: usize = 10;

/// The phoneme length kokoro-en will actually see, measured with the crate's
/// own grapheme-to-phoneme pass rather than estimated.
///
/// `speech_cost` is a weighted guess, and the guess is what kept failing: it
/// was rewritten from characters to weighted phonemes in 1e3fb6f, and a
/// dense line still ran past the cap two hours later. Estimating how a
/// phonemizer will expand arbitrary text is not a solvable problem — symbols
/// outside the weight table each become a whole spoken word — so this asks
/// the function that will do the expanding.
///
/// `KokoroTts` keeps its `is_v11` flag private, so both phoneme sets are
/// measured and the longer wins: being wrong about which model is loaded is
/// the class of failure this exists to remove, not to reintroduce.
///
/// `None` means g2p itself failed. That is not a measurement of zero, so the
/// caller keeps whatever the estimate gave it and lets the panic guard stand.
fn phoneme_len(text: &str) -> Option<usize> {
    let measure = |v11: bool| kokoro_en::g2p(text, v11).ok().map(|p| p.chars().count());
    match (measure(false), measure(true)) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(n), None) | (None, Some(n)) => Some(n),
        (None, None) => None,
    }
}

/// Re-split any chunk whose *measured* phoneme length still exceeds the cap.
///
/// `split_for_synthesis` breaks lines where they sound best; this makes the
/// result correct rather than merely likely. It runs after, not instead, so
/// ordinary prose still breathes where it always did — only a chunk that
/// would actually have panicked gets divided further.
fn enforce_phoneme_budget(chunks: Vec<String>) -> Vec<String> {
    enforce_budget_with(chunks, &|t| phoneme_len(t))
}

/// The splitting itself, over an injectable measurement so it can be tested
/// without a phonemizer.
fn enforce_budget_with(
    chunks: Vec<String>,
    measure: &dyn Fn(&str) -> Option<usize>,
) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in chunks {
        divide(chunk, measure, 0, &mut out);
    }
    out
}

fn divide(
    chunk: String,
    measure: &dyn Fn(&str) -> Option<usize>,
    depth: usize,
    out: &mut Vec<String>,
) {
    if measure(&chunk).is_none_or(|n| n <= MAX_PHONEME_CHARS) {
        out.push(chunk);
        return;
    }
    let words: Vec<&str> = chunk.split_whitespace().collect();
    // A single word that still measures over cannot be divided on a boundary
    // anyone would want to hear. It goes out whole: the panic guard at the
    // call site is the last line, and silently dropping speech would be a
    // worse answer than a line that fails loudly.
    if words.len() < 2 || depth >= MAX_SPLIT_DEPTH {
        out.push(chunk);
        return;
    }
    let mid = words.len() / 2;
    divide(words[..mid].join(" "), measure, depth + 1, out);
    divide(words[mid..].join(" "), measure, depth + 1, out);
}

/// Kokoro-82M via ONNX (`kokoro-en`/ort): near-cloud-quality speech, fully
/// on-device, roughly 2× realtime on Apple Silicon CPU. 24 kHz output.
pub struct KokoroEngine {
    tts: kokoro_en::KokoroTts,
}

impl KokoroEngine {
    pub const SAMPLE_RATE: u32 = 24_000;

    pub async fn load(dir: &Path) -> Result<Self> {
        let tts = kokoro_en::KokoroTts::new(dir.join(KOKORO_MODEL), dir.join("voices"))
            .await
            .map_err(|e| anyhow::anyhow!("Couldn't load the Audio Overview voices: {e}"))?;
        Ok(Self { tts })
    }

    pub async fn synth(&self, speaker: Speaker, text: &str, out_wav: &Path) -> Result<()> {
        let voice = match speaker {
            Speaker::Host => HOST_VOICE,
            Speaker::Guest => GUEST_VOICE,
        };
        // Kokoro's voice packs hold one style vector per phoneme count, capped
        // at 510 — a long monologue line indexes past the pack and panics
        // inside kokoro-en. Synthesize in sentence-packed chunks instead, with
        // a small breath between chunks so the join stays conversational.
        let mut samples: Vec<f32> = Vec::new();
        let chunks = enforce_phoneme_budget(split_for_synthesis(text, MAX_SYNTH_COST));
        for (i, chunk) in chunks.iter().enumerate() {
            if i > 0 {
                samples.extend(std::iter::repeat_n(
                    0.0f32,
                    (Self::SAMPLE_RATE / 8) as usize,
                ));
            }
            // kokoro-en indexes its style pack by phoneme count and panics
            // rather than erroring when the count runs past 510. The budget
            // above is an estimate, and an estimate can be wrong — so a bad
            // guess costs this line, not the whole Brief and not the process.
            let attempt = std::panic::AssertUnwindSafe(self.tts.synth(chunk, voice)).catch_unwind();
            let (chunk_samples, _took) = match attempt.await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => anyhow::bail!("Voice synthesis failed: {e}"),
                Err(_) => anyhow::bail!(
                    "Voice synthesis failed on an unusually dense passage                      (too many phonemes for the model)"
                ),
            };
            samples.extend(chunk_samples);
        }
        anyhow::ensure!(!samples.is_empty(), "No audio was produced for a line");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: Self::SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(out_wav, spec).context("failed to create line wav")?;
        for s in samples {
            writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        writer.finalize()?;
        Ok(())
    }
}

/// Stitch per-line WAVs (mono LEI16 at `sample_rate`) into one AAC `.m4a`,
/// with a short beat of silence between turns so it breathes like
/// conversation.
pub async fn assemble_episode(
    line_wavs: &[std::path::PathBuf],
    gaps_ms: &[u32],
    out_m4a: &Path,
    sample_rate: u32,
) -> Result<()> {
    let episode_wav = out_m4a.with_extension("wav");
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(&episode_wav, spec).context("failed to create episode wav")?;
        for (i, wav) in line_wavs.iter().enumerate() {
            if i > 0 {
                let gap_ms = gaps_ms.get(i - 1).copied().unwrap_or(300);
                let gap_samples = sample_rate * gap_ms / 1000;
                for _ in 0..gap_samples {
                    writer.write_sample(0i16)?;
                }
            }
            let mut reader = hound::WavReader::open(wav)
                .with_context(|| format!("failed to read line wav {wav:?}"))?;
            for sample in reader.samples::<i16>() {
                writer.write_sample(sample?)?;
            }
        }
        writer.finalize()?;
    }
    let status = tokio::process::Command::new("afconvert")
        .args(["-f", "m4af", "-d", "aac"])
        .arg(&episode_wav)
        .arg(out_m4a)
        .status()
        .await
        .context("failed to run afconvert for the episode")?;
    let _ = std::fs::remove_file(&episode_wav);
    anyhow::ensure!(status.success(), "afconvert failed to encode the episode");
    Ok(())
}

#[cfg(test)]
mod split_tests {
    use super::{
        enforce_budget_with, enforce_phoneme_budget, phoneme_len, speech_cost, split_for_synthesis,
        MAX_PHONEME_CHARS, MAX_SYNTH_COST,
    };

    #[test]
    fn short_lines_pass_through() {
        assert_eq!(
            split_for_synthesis("Hello there.", 300),
            vec!["Hello there."]
        );
    }

    #[test]
    fn long_lines_split_at_sentences_under_cap() {
        let line = "First sentence here. ".repeat(40); // ~840 chars
        let chunks = split_for_synthesis(&line, 300);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
        assert!(chunks.iter().all(|c| c.ends_with('.')));
        // Nothing lost: same words in, same words out.
        let rejoined: Vec<&str> = chunks.iter().flat_map(|c| c.split_whitespace()).collect();
        assert_eq!(rejoined.len(), line.split_whitespace().count());
    }

    /// The regression: a Brief line is dates and figures, and those speak
    /// far longer than they read. Under the old character budget this line
    /// looked short enough to pass whole, then panicked inside kokoro-en.
    #[test]
    fn figure_dense_line_is_budgeted_by_speech_not_length() {
        let line = "Revenue hit $1,284,300 on 2026-08-23, up 47% from                     2025-08-23, across 1,024 accounts and 512 regions. "
            .repeat(3);
        assert!(
            line.chars().count() < 600,
            "fixture should look short by character count"
        );
        let chunks = split_for_synthesis(&line, 300);
        assert!(
            chunks.iter().all(|c| speech_cost(c) <= 300),
            "every chunk must fit the phoneme budget: {:?}",
            chunks.iter().map(|c| speech_cost(c)).collect::<Vec<_>>()
        );
        // Nothing lost on the way through.
        let rejoined: Vec<&str> = chunks.iter().flat_map(|c| c.split_whitespace()).collect();
        assert_eq!(rejoined.len(), line.split_whitespace().count());
    }

    #[test]
    fn digits_cost_more_than_letters() {
        assert!(speech_cost("2026") > speech_cost("abcd"));
        assert_eq!(speech_cost("abcd"), 4);
    }

    /// A single unbroken digit run has no whitespace to split on, so the
    /// word-level fallback is the only thing standing between it and a panic.
    #[test]
    fn one_oversized_word_still_fits_the_budget() {
        let line = format!("id {} end", "9".repeat(400));
        let chunks = split_for_synthesis(&line, 300);
        assert!(
            chunks.iter().all(|c| speech_cost(c) <= 300),
            "oversized word must be broken: {:?}",
            chunks.iter().map(|c| speech_cost(c)).collect::<Vec<_>>()
        );
        assert!(!chunks.is_empty());
    }

    #[test]
    fn single_token_longer_than_the_cap_is_split() {
        // A URL or unbroken run has no whitespace to break on. Pushing it
        // whole is what put a 500+ phoneme chunk into Kokoro and panicked
        // the episode; every chunk must honour the cap the caller asked for.
        let line = format!("Start. https://example.com/{} end.", "a".repeat(400));
        let chunks = split_for_synthesis(&line, 300);
        assert!(
            chunks.iter().all(|c| c.chars().count() <= 300),
            "chunk over cap: {:?}",
            chunks.iter().map(|c| c.chars().count()).collect::<Vec<_>>()
        );
        // Nothing is dropped on the way through.
        let rejoined: String = chunks.join("");
        assert!(rejoined.contains(&"a".repeat(50)), "long token lost");
    }

    #[test]
    fn a_lone_giant_word_still_splits() {
        let line = "z".repeat(1000);
        let chunks = split_for_synthesis(&line, 300);
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
        assert_eq!(chunks.join("").chars().count(), 1000, "characters lost");
    }

    #[test]
    fn giant_unpunctuated_sentence_hard_splits() {
        let line = "word ".repeat(200); // 1000 chars, no terminators
        let chunks = split_for_synthesis(&line, 300);
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
    }

    /// The estimate is allowed to be wrong; the measurement is not. A chunk
    /// that measures over the cap must come back divided, however innocent it
    /// looked to `speech_cost`.
    #[test]
    fn a_chunk_that_measures_over_is_divided() {
        // Pretend every word phonemizes to 100 characters - the shape of the
        // real failure, where a symbol expands into a whole spoken word.
        let measure = |t: &str| Some(t.split_whitespace().count() * 100);
        let line = "alpha bravo charlie delta echo foxtrot golf hotel".to_string();
        assert!(
            measure(&line).unwrap() > MAX_PHONEME_CHARS,
            "the fixture must start over budget or it proves nothing"
        );

        let out = enforce_budget_with(vec![line.clone()], &measure);
        assert!(out.len() > 1, "an over-budget chunk must be split");
        for piece in &out {
            assert!(
                measure(piece).unwrap() <= MAX_PHONEME_CHARS,
                "every piece must land under the cap: {piece:?}"
            );
        }
        // Splitting is a re-packing, never an edit: every word survives.
        assert_eq!(out.join(" "), line);
    }

    /// Ordinary prose must pass through untouched, or every line breathes in
    /// a new place for no reason.
    #[test]
    fn a_chunk_under_budget_is_left_alone() {
        let measure = |t: &str| Some(t.chars().count());
        let chunks = vec!["Short enough.".to_string(), "Also fine.".to_string()];
        assert_eq!(
            enforce_budget_with(chunks.clone(), &measure),
            chunks,
            "under-budget chunks must not be re-cut"
        );
    }

    /// A failed measurement is not a measurement of zero. When g2p cannot
    /// answer, the estimate stands and the panic guard remains the last line -
    /// the alternative is shredding every line whenever the phonemizer is
    /// unavailable.
    #[test]
    fn an_unmeasurable_chunk_is_passed_through() {
        let line = "alpha bravo charlie delta".to_string();
        let out = enforce_budget_with(vec![line.clone()], &|_| None);
        assert_eq!(out, vec![line]);
    }

    /// One indivisible word over the cap must terminate rather than recurse
    /// forever, and must not be dropped on the floor.
    #[test]
    fn one_huge_word_terminates_and_survives() {
        let word = "supercalifragilisticexpialidocious".to_string();
        let out = enforce_budget_with(vec![word.clone()], &|_| Some(100_000));
        assert_eq!(out, vec![word], "an indivisible word goes out whole");
    }

    /// The integration the unit tests above deliberately fake: run the real
    /// phonemizer and prove nothing survives the pipeline over the cap.
    ///
    /// The fixture is the shape that beat the estimate. With
    /// `default-features = false` kokoro-en has no bundled espeak, so a word
    /// its lexicon does not know is *letter-spelled* - every character
    /// becomes its own run of phonemes - and a line of identifiers and
    /// symbols expands far past what `speech_cost` predicts.
    ///
    /// Skips rather than fails when g2p cannot answer: a machine without the
    /// phonemizer should not fail the suite, and `phoneme_len` returning None
    /// is the documented "keep the estimate" path.
    #[test]
    fn real_phonemes_stay_under_the_cap() {
        // Figures and symbols, not nonsense words: each expands into whole
        // spoken words through the pure-Rust normalizer, which is the same
        // blowup with none of the per-word espeak subprocesses that would
        // make this test slow.
        let nasty = "In 2026 revenue rose 47.5% to $1,284,999 against 2025. ".repeat(8);

        let Some(measured) = phoneme_len(&nasty) else {
            eprintln!("g2p unavailable; skipping the live phoneme check");
            return;
        };
        assert!(
            measured > MAX_PHONEME_CHARS,
            "fixture must actually exceed the cap to prove anything (got {measured})"
        );

        let chunks = enforce_phoneme_budget(split_for_synthesis(&nasty, MAX_SYNTH_COST));
        for chunk in &chunks {
            if let Some(n) = phoneme_len(chunk) {
                assert!(
                    n <= MAX_PHONEME_CHARS || chunk.split_whitespace().count() < 2,
                    "chunk of {n} phonemes would index past the style pack: {chunk:?}"
                );
            }
        }
    }
}
