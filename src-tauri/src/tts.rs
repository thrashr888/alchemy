//! Text-to-speech for Audio Overview episodes.
//!
//! The generator writes a `HOST:`/`GUEST:` dialogue script (a format even
//! small local models produce reliably); this module parses it, synthesizes
//! each line with a per-speaker voice, and assembles one `.m4a` episode.
//!
//! The first engine is macOS `say` — zero downloads, zero notarization risk.
//! A Kokoro/ONNX engine can slot in behind a trait when it lands (see
//! docs/RFC-audio-overview.md).

use anyhow::{Context, Result};
use std::path::Path;

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

/// macOS built-in synthesis via `say` + `afconvert`: renders one line to a
/// 22.05 kHz mono LEI16 WAV. Voices are per speaker; a missing voice falls
/// back to the system default rather than failing the episode. When a second
/// engine (Kokoro/ONNX) lands, extract the `synth` signature into a trait.
pub struct SayTts {
    pub host_voice: String,
    pub guest_voice: String,
}

impl Default for SayTts {
    fn default() -> Self {
        Self {
            // A warm US host and a distinct UK guest — both ship with macOS.
            host_voice: "Samantha".into(),
            guest_voice: "Daniel".into(),
        }
    }
}

impl SayTts {
    pub async fn synth(&self, speaker: Speaker, text: &str, out_wav: &Path) -> Result<()> {
        let voice = match speaker {
            Speaker::Host => &self.host_voice,
            Speaker::Guest => &self.guest_voice,
        };
        // `say` writes the assembler's exact PCM shape (mono LEI16 @22.05k
        // WAV) directly, and reads the text from stdin (`-f -`) so dialogue
        // can't be confused for flags. An unknown voice is the one common
        // failure — retry once with the system default voice.
        if !run_say(Some(voice), text, out_wav).await? {
            anyhow::ensure!(
                run_say(None, text, out_wav).await?,
                "`say` failed for a line"
            );
        }
        Ok(())
    }
}

async fn run_say(voice: Option<&str>, text: &str, out_wav: &Path) -> Result<bool> {
    let mut cmd = tokio::process::Command::new("say");
    if let Some(v) = voice {
        cmd.arg("-v").arg(v);
    }
    cmd.arg("--data-format=LEI16@22050")
        .arg("-o")
        .arg(out_wav)
        .arg("-f")
        .arg("-");
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to run `say`")?;
    use tokio::io::AsyncWriteExt;
    child
        .stdin
        .take()
        .context("no stdin for `say`")?
        .write_all(text.as_bytes())
        .await?;
    Ok(child.wait().await?.success())
}

/// Stitch per-line WAVs (22.05 kHz mono LEI16) into one AAC `.m4a`, with a
/// short beat of silence between turns so it breathes like conversation.
pub async fn assemble_episode(line_wavs: &[std::path::PathBuf], out_m4a: &Path) -> Result<()> {
    const GAP_MS: usize = 300;
    let episode_wav = out_m4a.with_extension("wav");
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 22050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(&episode_wav, spec).context("failed to create episode wav")?;
        let gap_samples = 22050 * GAP_MS / 1000;
        for (i, wav) in line_wavs.iter().enumerate() {
            if i > 0 {
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
