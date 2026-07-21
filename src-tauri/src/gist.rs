//! Source gists (docs/RFC-infinite-context.md Phase 1): one distilled
//! overview row per source, stored in the chunks table under
//! `source_id = "gist:<id>"` so it rides the same vector + FTS index and
//! joins fusion as its own capped evidence class.
//!
//! Modeled on `router::ensure_router`: a self-healing sweep diffs desired
//! state (every eligible source, keyed by content hash) against stored gist
//! rows and regenerates only what changed — no hooks in every write path,
//! and queue state is always re-derivable from the hashes. The sweep is
//! fire-and-forget, budgeted per batch, and every failure degrades to
//! "no gist", never to a broken import (RFC guardrails).
//!
//! Generated text is gated before it is stored (the Doc2Query-- lesson:
//! hallucinated expansions actively hurt retrieval): length bounds, a
//! degeneracy check, and every identifier-ish token in the gist must appear
//! verbatim in the source. A gist that fails the gate is dropped and the
//! (source, hash) pair is remembered for this app run so the sweep doesn't
//! spin on an unwilling model.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::Result;

use crate::ai::Ai;
use crate::db::{Db, GistRow, GIST_CHUNK_PREFIX};
use crate::inference::{ChatTurn, Role};

/// Gate bounds for a stored gist (RFC-infinite-context §1).
const GIST_MIN_CHARS: usize = 200;
const GIST_MAX_CHARS: usize = 1200;
/// Sources shorter than this are their own gist — distilling them adds a
/// worse duplicate, not signal.
const MIN_SOURCE_CHARS: i64 = 600;
/// How much source text the distillation prompt sees. Head-only is
/// deliberate: leads summarize, and a Small-role model with a tight window
/// must never be handed 3M chars.
const PROMPT_HEAD_CHARS: usize = 10_000;
/// Sources gisted per `ensure_gists` call — keeps one sweep batch short so
/// a cold-start backfill yields between batches instead of hogging the
/// engine for minutes.
const SWEEP_BUDGET: usize = 4;
/// Batches per spawned sweep — a runaway fence, not a target (4 × 50 = 200
/// sources per sweep; anything bigger finishes on the next trigger).
const MAX_SWEEP_BATCHES: usize = 50;

/// One sweep at a time, process-wide; a second trigger while one runs is a
/// no-op (the running sweep will pick up whatever the trigger saw).
static SWEEPING: AtomicBool = AtomicBool::new(false);

/// (source_id → content hash) pairs whose generation failed the gate this
/// app run — skipped until the content changes or the app restarts, so an
/// unwilling model doesn't get re-asked every sweep.
static REFUSED: Mutex<Option<HashMap<String, i32>>> = Mutex::new(None);

fn refused_matches(source_id: &str, hash: i32) -> bool {
    let guard = REFUSED.lock().unwrap();
    guard
        .as_ref()
        .and_then(|m| m.get(source_id))
        .is_some_and(|h| *h == hash)
}

fn remember_refusal(source_id: &str, hash: i32) {
    let mut guard = REFUSED.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(source_id.to_string(), hash);
}

/// FNV-1a over the source text, folded to a non-negative i32 so it fits the
/// chunk row's `ordinal` column. Stability across runs is the contract —
/// this is the staleness signal the sweep diffs, never a position.
pub fn content_hash(text: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in text.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) as i32
}

/// The distillation prompt. Plain text out — Small-role models (3–8B, Apple
/// FM) parse no JSON reliably, and the whole reply is the artifact.
fn build_messages(title: &str, source_type: &str, text: &str) -> Vec<ChatTurn> {
    let head: String = text.chars().take(PROMPT_HEAD_CHARS).collect();
    let truncated = if text.chars().count() > PROMPT_HEAD_CHARS {
        "\n[document continues beyond this excerpt]"
    } else {
        ""
    };
    vec![
        ChatTurn::system(
            "You distill documents for a retrieval index. Reply with ONLY the \
             distillation — no preamble, no markdown headings.",
        ),
        ChatTurn::user(format!(
            "Distill this {source_type} document titled \"{title}\":\n\
             1. Three to six sentences: what it contains, and what questions it can answer.\n\
             2. One final line starting exactly \"Key terms: \" listing the important \
             names, identifiers, and codes that appear verbatim in the document.\n\
             Use only words and identifiers that actually appear in the document.\n\n\
             Document:\n---\n{head}{truncated}",
        )),
    ]
}

/// Identifier-ish tokens: the strings that would mislead retrieval if
/// hallucinated — anything carrying a digit, an underscore/hyphen compound,
/// or mixed-case beyond a leading capital. Plain prose words are the
/// model's to paraphrase; identifiers are not.
fn identifier_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| ch.is_whitespace() || ",;()[]{}\"'`".contains(ch))
        .map(|t| t.trim_matches(|ch: char| ".:!?".contains(ch)))
        .filter(|t| t.chars().count() >= 4)
        .filter(|t| {
            let has_digit = t.chars().any(|c| c.is_ascii_digit());
            let compound = t.contains('_') || (t.contains('-') && !t.ends_with('-'));
            let mixed_case = t
                .chars()
                .skip(1)
                .any(|c| c.is_uppercase() && t.chars().any(|c2| c2.is_lowercase()));
            has_digit || compound || mixed_case
        })
        .map(str::to_string)
        .collect()
}

/// Accept or reject a generated gist. `None` means "store nothing" — the
/// caller falls back to prefix-only retrieval, which is today's behavior.
pub fn gate(candidate: &str, raw: &str) -> Option<String> {
    let gist = candidate.trim();
    let n = gist.chars().count();
    if !(GIST_MIN_CHARS..=GIST_MAX_CHARS).contains(&n) {
        return None;
    }
    // Degeneracy: a looping model repeats lines; real prose doesn't.
    let lines: Vec<&str> = gist
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let distinct: HashSet<&&str> = lines.iter().collect();
    if lines.len() >= 4 && distinct.len() * 2 < lines.len() {
        return None;
    }
    // Every identifier in the gist must appear in the source (case-blind).
    // One invented error code poisons exact-match retrieval forever.
    let raw_lower = raw.to_lowercase();
    for tok in identifier_tokens(gist) {
        if !raw_lower.contains(&tok.to_lowercase()) {
            return None;
        }
    }
    Some(gist.to_string())
}

/// Bring gist rows in line with the corpus, at most `SWEEP_BUDGET`
/// generations per call. Returns (written, deleted); (0, 0) means fully
/// converged. Mirrors `ensure_router`'s shape: list desired, diff stored,
/// touch only the difference.
pub async fn ensure_gists(db: &Db, ai: &Ai) -> Result<(usize, usize)> {
    let stored: HashMap<String, i32> = db
        .list_gists()
        .await?
        .into_iter()
        .map(|g: GistRow| (g.source_id, g.hash))
        .collect();

    // Desired: every eligible source, with the hash its gist should carry.
    // Code sources keep their path-prefix embedding (the RFC's per-type
    // policy); unembedded repo children have no retrieval presence to
    // improve; short sources are already their own summary.
    struct Want {
        notebook_id: String,
        source_id: String,
        hash: Option<i32>, // None = hash needs full content (listing had none)
    }
    let mut desired: Vec<Want> = Vec::new();
    for nb in db.list_notebooks().await? {
        for s in db.list_sources(&nb.id).await? {
            if s.source_type == "code" || s.chunk_count == 0 || s.char_count < MIN_SOURCE_CHARS {
                continue;
            }
            let hash = if s.content.is_empty() {
                None
            } else {
                Some(content_hash(&s.content))
            };
            desired.push(Want {
                notebook_id: nb.id.clone(),
                source_id: s.id,
                hash,
            });
        }
    }

    // Stale rows: gists whose source vanished (delete_source also removes
    // gists inline; this catches anything that slipped past, e.g. rows
    // written by an older build).
    let desired_ids: HashSet<&str> = desired.iter().map(|w| w.source_id.as_str()).collect();
    let mut deleted = 0usize;
    for sid in stored.keys() {
        if !desired_ids.contains(sid.as_str()) {
            db.delete_gist_row(sid).await?;
            deleted += 1;
        }
    }

    let mut written = 0usize;
    for want in desired {
        if written >= SWEEP_BUDGET {
            break;
        }
        // Cheap staleness check first; fetch full content only for work.
        let source = match want.hash {
            Some(h) if stored.get(&want.source_id) == Some(&h) => continue,
            _ => match db.get_source(&want.source_id).await? {
                Some(s) => s,
                None => continue,
            },
        };
        let hash = content_hash(&source.content);
        if stored.get(&want.source_id) == Some(&hash) || refused_matches(&want.source_id, hash) {
            continue;
        }

        let messages = build_messages(&source.title, &source.source_type, &source.content);
        let reply = match ai.chat_role(Role::Small, &messages).await {
            Ok(out) => out.text,
            Err(err) => {
                // Engine trouble ends the batch — the next sweep retries.
                eprintln!("gist: generation failed for \"{}\": {err:#}", source.title);
                break;
            }
        };
        let Some(gist) = gate(&reply, &source.content) else {
            eprintln!(
                "gist: gate rejected output for \"{}\" ({} chars)",
                source.title,
                reply.trim().chars().count()
            );
            remember_refusal(&want.source_id, hash);
            continue;
        };

        // Same two-text scheme as regular chunks: verbatim gist in `text`
        // (it IS the snippet), title-context prefix on the embedded form.
        let embed_input = format!("[{} — overview]\n{gist}", source.title);
        let embeddings = ai.embed(&[embed_input]).await?;
        db.delete_gist_row(&want.source_id).await?;
        db.add_chunks(
            &want.notebook_id,
            &format!("{GIST_CHUNK_PREFIX}{}", want.source_id),
            &[(crate::commands::new_id(), hash, gist)],
            &embeddings,
        )
        .await?;
        written += 1;
    }
    Ok((written, deleted))
}

/// Fire-and-forget sweep. Takes owned snapshots (the shared `Arc<Db>`; `Ai`
/// via the momentary-read-guard snapshot pattern) so no Tauri handle is
/// needed. Config changes mid-sweep apply from the next trigger.
pub fn spawn_sweep(db: std::sync::Arc<Db>, ai: Ai) {
    if !ai.config().source_gists {
        return;
    }
    if SWEEPING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        for _ in 0..MAX_SWEEP_BATCHES {
            match ensure_gists(&db, &ai).await {
                Ok((0, 0)) => break,
                Ok(_) => {
                    // Yield between batches so imports and chat stay snappy.
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                Err(err) => {
                    eprintln!("gist sweep failed: {err:#}");
                    break;
                }
            }
        }
        SWEEPING.store(false, Ordering::SeqCst);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_stable_and_positive() {
        let a = content_hash("The vendor payment runbook, net-45 wires.");
        assert_eq!(a, content_hash("The vendor payment runbook, net-45 wires."));
        assert!(a >= 0);
        assert_ne!(a, content_hash("The vendor payment runbook, net-45 wires!"));
    }

    #[test]
    fn gate_rejects_out_of_bounds_lengths() {
        let raw = "Anything at all.";
        assert!(gate("too short", raw).is_none());
        let long = "word ".repeat(400);
        assert!(gate(&long, raw).is_none());
    }

    #[test]
    fn gate_rejects_hallucinated_identifiers() {
        let raw = "Retries use ERR-500-RETRY with a ten second wait. \
                   The loader is CheckpointLoader in checkpoint_loader.cc.";
        let good = format!(
            "This runbook explains retry behavior for the loader and when waits apply. \
             It covers how ERR-500-RETRY is issued and what CheckpointLoader does when \
             a manifest stalls during restore. It can answer how long the retry wait is \
             and which component performs loading. {}Key terms: ERR-500-RETRY, CheckpointLoader",
            ""
        );
        assert!(gate(&good, raw).is_some());
        let bad = good.replace("ERR-500-RETRY", "ERR-404-RETRY");
        assert!(gate(&bad, raw).is_none(), "invented code must be rejected");
    }

    #[test]
    fn gate_rejects_looping_output() {
        let line = "It covers the vendor payment process end to end.\n";
        let looped = format!(
            "This document describes vendor payments in detail for the team.\n{}",
            line.repeat(8)
        );
        assert!(gate(&looped, "vendor payments").is_none());
    }

    #[test]
    fn identifier_tokens_skip_prose_but_catch_codes() {
        let toks = identifier_tokens(
            "The Vendor payment runbook covers ERR-500-RETRY, checkpoint_loader.cc \
             and CheckpointLoader for net-45 terms.",
        );
        assert!(toks.contains(&"ERR-500-RETRY".to_string()));
        assert!(toks.contains(&"checkpoint_loader.cc".to_string()));
        assert!(toks.contains(&"CheckpointLoader".to_string()));
        assert!(toks.contains(&"net-45".to_string()));
        assert!(!toks.iter().any(|t| t == "Vendor" || t == "runbook"));
    }
}
