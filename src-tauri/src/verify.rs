//! Runtime answer verification (docs/RFC-judged-evals.md §5) — the Mellea
//! payoff: the same checks the judged harness scores offline run against
//! live chat answers, and a caught defect triggers ONE repair pass. The
//! verifier is the shipping cross-encoder; its tripwire threshold comes
//! from the codex calibration (2026-08-11: unsupported-detection F1 peaks
//! at t≈0.51; at that point it catches roughly half the bad claims with
//! few false alarms — a tripwire, not a guarantee).
//!
//! Shared by `judged_eval.rs` (offline scoring) and the chat pipeline
//! (post-stream verify-and-repair in `commands.rs`).

use crate::inference::rerank::CrossEncoder;
use crate::models::Citation;

/// The unsupported-claim tripwire. MEASURED lesson (2026-08-18, the
/// `repaired` harness variant): the F1-optimal 0.51 flagged so many good
/// claims that repair REGRESSED weak generators (bonsai gold-cited
/// 72%→56%) — half the flags were false positives and models dutifully
/// weakened good answers. Repair wants PRECISION: flag only what the
/// verifier is confident is ungrounded. -0.5 sits at the calibration's
/// accuracy optimum, where flags are few and mostly real.
pub const REPAIR_THRESHOLD: f32 = -0.5;

/// Extract `[n]` markers from one sentence and return the sentence with
/// markers stripped. Hand-rolled — not worth a regex dependency.
pub fn strip_markers(s: &str) -> (String, Vec<usize>) {
    let (mut clean, mut markers) = (String::new(), Vec::new());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let (before, bracketed) = rest.split_at(open);
        clean.push_str(before);
        match bracketed[1..].find(']') {
            Some(close) if bracketed[1..close + 1].chars().all(|c| c.is_ascii_digit()) => {
                if let Ok(n) = bracketed[1..close + 1].parse::<usize>() {
                    markers.push(n);
                }
                rest = &bracketed[close + 2..];
            }
            _ => {
                clean.push('[');
                rest = &bracketed[1..];
            }
        }
    }
    clean.push_str(rest);
    (clean.trim().to_string(), markers)
}

/// Sentences with the 1-based excerpt markers each one carries. Markers
/// straddle punctuation styles: bonsai writes "claim [1]." and gpt-oss
/// writes "claim.[1][2]" — a sentence break absorbs any marker groups
/// that immediately follow it, so both attribute to the right sentence.
pub fn cited_sentences(answer: &str) -> Vec<(String, Vec<usize>)> {
    let chars: Vec<char> = answer.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        cur.push(ch);
        i += 1;
        if matches!(ch, '.' | '!' | '?' | '\n') {
            while i < chars.len() && chars[i] == '[' {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 && j < chars.len() && chars[j] == ']' {
                    cur.extend(&chars[i..=j]);
                    i = j + 1;
                } else {
                    break;
                }
            }
            let s = cur.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
    }
    let s = cur.trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    out.into_iter().map(|s| strip_markers(&s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_attach_across_punctuation_styles() {
        // gpt-oss style: markers AFTER the period.
        let after = cited_sentences("A 1:2 ratio yields 36g in 28 seconds.[4][5] Grind finer.[7]");
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].1, vec![4, 5]);
        assert_eq!(after[1].1, vec![7]);
        // bonsai style: markers BEFORE the period.
        let before = cited_sentences("The ratio is 1:2 [1][2]. Timing runs 28 seconds [3].");
        assert_eq!(before.len(), 2);
        assert_eq!(before[0].1, vec![1, 2]);
        assert_eq!(before[1].1, vec![3]);
        // Non-marker brackets survive as text.
        let (clean, markers) = strip_markers("See [appendix] and [2].");
        assert_eq!(markers, vec![2]);
        assert!(clean.contains("[appendix]"));
    }
}

/// What verification found in one answer.
#[derive(Debug, Default)]
pub struct AnswerCheck {
    /// Citation markers pointing outside the shown excerpt range.
    pub invalid_markers: usize,
    /// Cited sentences whose best xenc score against every excerpt they
    /// cite fell at or below the tripwire.
    pub unsupported: Vec<String>,
    /// Cited sentences scored in total.
    pub scored: usize,
}

impl AnswerCheck {
    pub fn defects(&self) -> usize {
        self.invalid_markers + self.unsupported.len()
    }

    /// Whether `repaired` is an acceptable replacement for the answer
    /// this check described. Fewer defects alone is NOT enough — an
    /// answer that deletes its citations has zero checkable claims and
    /// "wins" trivially (measured: bonsai's repairs stripped citations
    /// and gold-evidence rates collapsed). The repaired answer may drop
    /// the flagged sentences, but must keep the rest of its grounding.
    pub fn accepts(&self, repaired: &AnswerCheck) -> bool {
        repaired.defects() < self.defects()
            && repaired.scored >= self.scored.saturating_sub(self.unsupported.len())
    }
}

/// Run L0 + L1 over a finished answer. Cheap: one cross-encoder batch per
/// cited sentence, ~30 ms each on the loaded model; any scoring failure
/// counts the sentence as fine (verification must never invent defects).
pub async fn check_answer(
    xenc: &CrossEncoder,
    answer: &str,
    hits: &[Citation],
    threshold: f32,
) -> AnswerCheck {
    let mut check = AnswerCheck::default();
    for (sentence, markers) in cited_sentences(answer) {
        if markers.is_empty() {
            continue;
        }
        let valid: Vec<usize> = markers
            .iter()
            .copied()
            .filter(|&m| m >= 1 && m <= hits.len())
            .collect();
        check.invalid_markers += markers.len() - valid.len();
        let cited: Vec<String> = valid.iter().map(|&m| hits[m - 1].snippet.clone()).collect();
        if cited.is_empty() || sentence.split_whitespace().count() < 4 {
            continue;
        }
        if let Ok(scores) = xenc.scores(&sentence, &cited).await {
            check.scored += 1;
            let max = scores.iter().cloned().fold(f32::MIN, f32::max);
            if max <= threshold {
                check.unsupported.push(sentence);
            }
        }
    }
    check
}

/// One repair pass: the original grounded prompt plus the draft and the
/// SPECIFIC defects. The model revises; it does not start over.
pub fn build_repair_messages(
    original: &[crate::ai::ChatTurn],
    draft: &str,
    check: &AnswerCheck,
) -> Vec<crate::ai::ChatTurn> {
    let mut problems = String::new();
    if check.invalid_markers > 0 {
        problems.push_str(&format!(
            "- {} citation marker(s) point outside the numbered excerpts; fix or remove \
             them.\n",
            check.invalid_markers
        ));
    }
    for s in &check.unsupported {
        problems.push_str(&format!(
            "- This claim is not supported by the excerpt(s) it cites: \"{s}\" — revise \
             it to only state what an excerpt says, cite an excerpt that actually \
             supports it, or remove it.\n"
        ));
    }
    let mut out = original.to_vec();
    out.push(crate::ai::ChatTurn::assistant(draft));
    out.push(crate::ai::ChatTurn::user(format!(
        "Your answer has grounding problems:\n{problems}\
         Rewrite the answer with these fixed. Keep everything that was already \
         well-supported, keep the same citation style, and do not add new claims."
    )));
    out
}
