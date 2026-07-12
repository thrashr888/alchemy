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
                    "downloading the Audio Overview voice model failed ({label}) — check \
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

/// Stay comfortably under Kokoro's 510-phoneme ceiling (phoneme tokens track
/// character count closely for English).
const MAX_SYNTH_CHARS: usize = 300;

/// Split a dialogue line into chunks short enough for Kokoro: pack whole
/// sentences up to the cap; hard-split on whitespace only when a single
/// sentence alone exceeds it.
pub(crate) fn split_for_synthesis(text: &str, max_chars: usize) -> Vec<String> {
    let text = text.trim();
    if text.chars().count() <= max_chars {
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
        if s.chars().count() > max_chars {
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            let mut piece = String::new();
            for w in s.split_whitespace() {
                if !piece.is_empty() && piece.chars().count() + 1 + w.chars().count() > max_chars {
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
        if !cur.is_empty() && cur.chars().count() + 1 + s.chars().count() > max_chars {
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
            .map_err(|e| anyhow::anyhow!("failed to load the Kokoro voice model: {e}"))?;
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
        for (i, chunk) in split_for_synthesis(text, MAX_SYNTH_CHARS)
            .iter()
            .enumerate()
        {
            if i > 0 {
                samples.extend(std::iter::repeat_n(
                    0.0f32,
                    (Self::SAMPLE_RATE / 8) as usize,
                ));
            }
            let (chunk_samples, _took) = self
                .tts
                .synth(chunk, voice)
                .await
                .map_err(|e| anyhow::anyhow!("Kokoro synthesis failed: {e}"))?;
            samples.extend(chunk_samples);
        }
        anyhow::ensure!(!samples.is_empty(), "Kokoro produced no audio for a line");
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
    use super::split_for_synthesis;

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

    #[test]
    fn giant_unpunctuated_sentence_hard_splits() {
        let line = "word ".repeat(200); // 1000 chars, no terminators
        let chunks = split_for_synthesis(&line, 300);
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
    }
}
