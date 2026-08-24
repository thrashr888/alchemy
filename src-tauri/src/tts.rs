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
const MAX_SYNTH_COST: usize = 300;

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
        for (i, chunk) in split_for_synthesis(text, MAX_SYNTH_COST).iter().enumerate() {
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
    use super::{speech_cost, split_for_synthesis};

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
    fn giant_unpunctuated_sentence_hard_splits() {
        let line = "word ".repeat(200); // 1000 chars, no terminators
        let chunks = split_for_synthesis(&line, 300);
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
    }
}
