//! Prompt token budgeting for tight-window engines.
//!
//! Apple's on-device Foundation Models model has a hard 8192-token context
//! window shared between the prompt and the response; a prompt over that
//! ceiling doesn't degrade, it hard-errors ("Content contains N tokens, which
//! exceeds the maximum allowed context size of 8192"). Every other engine
//! (Ollama, gateways, agent CLIs) carries a far larger window, so this budget
//! is applied ONLY on the Foundation Models path — at prompt assembly where the
//! structure is known (`crate::ai::Ai::fm_input_budget` gates it), and again as
//! an unconditional backstop right before the sidecar call in
//! `crate::inference::fm`, which only ever runs when the active engine IS the
//! on-device model.
//!
//! Trimming is structure-aware: the leading system instructions and the final
//! turn (which carries the user's actual question/instruction) always survive;
//! only the expendable middle — older conversation turns, then the retrieved
//! context/documents body of the final turn — is trimmed, preserving that
//! turn's head (the instruction) and tail (a trailing question).

use std::borrow::Cow;

use super::ChatTurn;

/// Apple's on-device Foundation Models context window, in tokens. The prompt
/// and the response share it; this is the hard ceiling the framework enforces.
pub const FM_CONTEXT_TOKENS: usize = 8192;

/// Tokens held back from the window for the model's own response, so a prompt
/// that exactly fills the input budget still leaves room to answer.
pub const FM_OUTPUT_RESERVE_TOKENS: usize = 1536;

/// The most prompt (input) tokens we will hand the on-device model: the window
/// minus the response reserve. ~6656 — comfortably inside the 6500–7000 target
/// band, and the estimator over-counts prose, so the true count runs lower.
pub const FM_INPUT_BUDGET_TOKENS: usize = FM_CONTEXT_TOKENS - FM_OUTPUT_RESERVE_TOKENS;

/// Small fixed cost charged per message for the chat template's role markers
/// and separators, which the raw content length does not capture.
const PER_MESSAGE_OVERHEAD: usize = 4;

/// Fixed priming cost for the whole request (BOS / template scaffold).
const PRIMING_OVERHEAD: usize = 3;

/// Chars kept from the tail of an over-long final turn, enough to preserve a
/// trailing question ("…\n\nQuestion: <q>") even when it is long or multi-part.
const TAIL_CHARS: usize = 4_000;

/// Conservative token estimate for a string: `ceil(chars / 3.5)`.
///
/// English prose is ~4 chars/token, so dividing by 3.5 deliberately
/// over-estimates — better to slightly under-fill the window than to overflow
/// it. Counts Unicode scalar values, not bytes, so multibyte text is not
/// double-counted. `chars / 3.5 == chars * 2 / 7`, done in integer math.
pub fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() * 2).div_ceil(7)
}

fn message_tokens(m: &ChatTurn) -> usize {
    estimate_tokens(&m.content) + PER_MESSAGE_OVERHEAD
}

/// Estimated total tokens for an assembled message list, template overhead
/// included.
pub fn messages_tokens(messages: &[ChatTurn]) -> usize {
    messages.iter().map(message_tokens).sum::<usize>() + PRIMING_OVERHEAD
}

/// Keep the head and tail of an over-long string, eliding the middle with a
/// visible marker so the model knows context was dropped. Returns at most
/// `target_chars` characters. Slicing is on `char` boundaries (byte slicing
/// panics on multibyte text). For the two prompt shapes this sees — a chat turn
/// ending in "Question: <q>" and an artifact turn beginning with the
/// instruction — preserving both ends keeps whichever end holds the
/// instruction/question intact.
fn elide_middle(content: &str, target_chars: usize) -> String {
    let total = content.chars().count();
    if total <= target_chars {
        return content.to_string();
    }
    const MARKER: &str = "\n\n…[context trimmed to fit the on-device model's context window]…\n\n";
    let marker_len = MARKER.chars().count();
    // Degenerate budget (the system turn alone nearly fills the window): keep a
    // sliver of each end. Only reachable for pathological prompts; the goal is
    // still "never overflow", so we accept a marker-dominated result.
    if target_chars <= marker_len + 32 {
        let head: String = content.chars().take(16).collect();
        let tail: String = content.chars().skip(total.saturating_sub(16)).collect();
        return format!("{head}{MARKER}{tail}");
    }
    let avail = target_chars - marker_len;
    // Protect the tail (a trailing question) but never let it eat the whole
    // budget — the head carries the instruction in the artifact shape.
    let tail_chars = (avail * 2 / 5).min(TAIL_CHARS);
    let head_chars = avail - tail_chars;
    let head: String = content.chars().take(head_chars).collect();
    let tail: String = content.chars().skip(total - tail_chars).collect();
    format!("{head}{MARKER}{tail}")
}

/// Fit an assembled prompt within `budget` input tokens for a tight-window
/// engine, trimming only the expendable parts.
///
/// Returns the input borrowed and untouched when it already fits (the common
/// case, and the only outcome for larger-window engines, which never call
/// this). When it does not fit:
///
/// 1. Older conversation turns are dropped oldest-first — every `system` turn
///    and the final turn are exempt.
/// 2. If the surviving turns still overflow, the final turn's body is elided in
///    the middle, preserving its head (instruction) and tail (question).
///
/// The system instructions and the user's actual question/instruction always
/// survive, intact and in position.
pub fn fit_messages<'a>(messages: &'a [ChatTurn], budget: usize) -> Cow<'a, [ChatTurn]> {
    if messages.is_empty() || messages_tokens(messages) <= budget {
        return Cow::Borrowed(messages);
    }

    let last = messages.len() - 1;
    let is_protected = |i: usize| i == last || messages[i].role == "system";

    // Drop droppable history oldest-first, tracking the running total so we
    // stop as soon as the prompt fits.
    let mut total = messages_tokens(messages);
    let mut dropped = vec![false; messages.len()];
    for (i, m) in messages.iter().enumerate() {
        if total <= budget {
            break;
        }
        if is_protected(i) {
            continue;
        }
        total -= message_tokens(m);
        dropped[i] = true;
    }

    let mut result: Vec<ChatTurn> = messages
        .iter()
        .enumerate()
        .filter(|(i, _)| !dropped[*i])
        .map(|(_, m)| m.clone())
        .collect();

    // Still over budget after shedding history: the bulk is the retrieved
    // context/documents inside the final turn. Elide its middle to fit.
    if messages_tokens(&result) > budget {
        let final_idx = result.len() - 1;
        let others_total: usize = result[..final_idx]
            .iter()
            .map(message_tokens)
            .sum::<usize>()
            + PRIMING_OVERHEAD;
        let content_budget_tokens = budget
            .saturating_sub(others_total)
            .saturating_sub(PER_MESSAGE_OVERHEAD);
        // tokens ≈ chars / 3.5, so chars ≈ tokens * 3.5 == tokens * 7 / 2.
        let target_chars = content_budget_tokens.saturating_mul(7) / 2;
        result[final_idx].content = elide_middle(&result[final_idx].content, target_chars);
    }

    Cow::Owned(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(messages: &[ChatTurn]) -> String {
        messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn estimate_tokens_is_a_ceiling_over_estimate() {
        assert_eq!(estimate_tokens(""), 0);
        // 7 chars / 3.5 == 2 exactly.
        assert_eq!(estimate_tokens("1234567"), 2);
        // 8 chars / 3.5 == 2.28 → ceil 3.
        assert_eq!(estimate_tokens("12345678"), 3);
        // Multibyte counts scalar values, not bytes (no double-count).
        assert_eq!(estimate_tokens("héllo"), estimate_tokens("hello"));
    }

    #[test]
    fn within_budget_is_borrowed_and_untouched() {
        let messages = vec![
            ChatTurn::system("short system rules"),
            ChatTurn::user("Question: what is x?"),
        ];
        let fitted = fit_messages(&messages, FM_INPUT_BUDGET_TOKENS);
        assert!(matches!(fitted, Cow::Borrowed(_)), "small prompt untouched");
        assert_eq!(fitted.len(), messages.len());
    }

    /// The chat shape: system preamble, prior turns, and a final turn whose
    /// middle is a huge retrieved-context block with the real question at the
    /// tail. Trimming must land under budget while keeping the system rules and
    /// the question verbatim, and shedding the old history.
    #[test]
    fn over_budget_chat_trims_context_keeps_system_and_question() {
        let system = "SYSTEM_SENTINEL: answer only from the excerpts and cite them.";
        // A large filler context stands in for retrieved excerpts + manifest.
        let filler = "lorem ipsum dolor sit amet ".repeat(8_000); // ~216k chars
        let final_turn = format!(
            "Sources in this notebook (2 total):\n- A\n- B\n\n\
             Source excerpts (top matches for this question only):\n\n{filler}\n---\n\n\
             Question: WHAT_IS_THE_ANSWER_SENTINEL"
        );
        let messages = vec![
            ChatTurn::system(system),
            ChatTurn::user("OLD_HISTORY_SENTINEL: an earlier turn"),
            ChatTurn::assistant("an earlier answer"),
            ChatTurn::user(final_turn),
        ];
        assert!(
            messages_tokens(&messages) > FM_INPUT_BUDGET_TOKENS,
            "fixture must start over budget"
        );

        let fitted = fit_messages(&messages, FM_INPUT_BUDGET_TOKENS);
        assert!(
            matches!(fitted, Cow::Owned(_)),
            "over-budget prompt trimmed"
        );

        // Now within budget.
        assert!(
            messages_tokens(&fitted) <= FM_INPUT_BUDGET_TOKENS,
            "trimmed to {} tokens, budget {FM_INPUT_BUDGET_TOKENS}",
            messages_tokens(&fitted)
        );

        // System instructions survive intact and in position 0.
        assert_eq!(fitted[0].role, "system");
        assert_eq!(
            fitted[0].content, system,
            "system preamble is never modified"
        );

        // The user's question survives verbatim in the final turn.
        let text = joined(&fitted);
        assert!(
            text.contains("WHAT_IS_THE_ANSWER_SENTINEL"),
            "the question must survive trimming"
        );
        // The instruction head of the final turn is kept too.
        assert!(
            text.contains("Sources in this notebook"),
            "final turn head is preserved"
        );
        // Old history was shed and the bulk context elided.
        assert!(
            !text.contains("OLD_HISTORY_SENTINEL"),
            "older history is dropped under pressure"
        );
        assert!(
            text.contains("context trimmed to fit"),
            "the elision marker signals dropped context"
        );
    }

    /// The artifact/summary shape: one system turn plus a single user turn that
    /// opens with the instruction and is followed by a huge source corpus.
    /// The instruction (head) and system must survive; the corpus is trimmed.
    #[test]
    fn over_budget_artifact_keeps_instruction_head() {
        let system = "ARTIFACT_SYS: generate faithful Markdown from the sources.";
        let instruction = "INSTRUCTION_SENTINEL: write a study guide of the sources below.";
        let corpus = "source paragraph number seven. ".repeat(10_000); // ~310k chars
        let user = format!("{instruction}\n\n--- SOURCES ---\n\n{corpus}");
        let messages = vec![ChatTurn::system(system), ChatTurn::user(user)];
        assert!(messages_tokens(&messages) > FM_INPUT_BUDGET_TOKENS);

        let fitted = fit_messages(&messages, FM_INPUT_BUDGET_TOKENS);
        assert!(messages_tokens(&fitted) <= FM_INPUT_BUDGET_TOKENS);
        assert_eq!(fitted[0].content, system, "system preamble untouched");
        let text = joined(&fitted);
        assert!(
            text.contains("INSTRUCTION_SENTINEL"),
            "the instruction at the head must survive"
        );
        assert!(
            text.chars().count() < messages[1].content.chars().count(),
            "the corpus was actually trimmed"
        );
    }

    /// A single over-long turn with no separate system turn still gets fitted
    /// rather than passed through — the backstop's guarantee is unconditional.
    #[test]
    fn single_oversized_turn_is_fitted() {
        let messages = vec![ChatTurn::user("x".repeat(100_000))];
        let fitted = fit_messages(&messages, FM_INPUT_BUDGET_TOKENS);
        assert!(messages_tokens(&fitted) <= FM_INPUT_BUDGET_TOKENS);
    }
}
