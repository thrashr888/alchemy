//! Outline-guided retrieval as silent escalation (docs/RFC-outline-index.md
//! Phase 3). Hybrid search stays the fast default. When its result is thin,
//! or lands on look-alike sections of a long structured source, the Small
//! role reads the notebook's outline — one line per section, the summary
//! Phase 2 wrote — picks up to two sections, and their best passages join
//! the pool ahead of the rerank. One extra small-model call, invisible to
//! the user, traced as `outlinePick` on the retrieval line.

use anyhow::Result;

use crate::ai::{Ai, ChatTurn};
use crate::db::{Db, OutlineEntry};
use crate::inference::Role;
use crate::models::Citation;

/// Outline lines the model reads at most — a notebook with more sections
/// than this keeps the closest by source title order; long enough for a
/// couple of manuals, short enough to answer in one breath.
const MAX_OUTLINE_LINES: usize = 80;
/// Sections the model may pick.
const MAX_PICKS: usize = 2;
/// Passages pulled per picked section.
const PASSAGES_PER_PICK: usize = 2;
/// The flat search is "thin" below this — the same bar `rag::build_chat_messages`
/// uses to ask a clarifying question instead of guessing.
const THIN_CITATIONS: usize = 3;
const THIN_CHARS: usize = 700;

/// Does this pool call for the outline? Thin, or the top hits are the same
/// subsection of different chapters (their chains end alike) — the exact
/// shape of a look-alike miss in a structured document. Never when the
/// question quotes an identifier and the flat leader carries it verbatim:
/// a literal match is stronger evidence than a summary can give, and an
/// outline guess against it measured exact-kind MRR 1.00 → 0.33.
pub fn should_escalate(question: &str, pool: &[Citation]) -> bool {
    if let Some(top) = pool.first() {
        if literal_hit(question, &top.snippet) {
            return false;
        }
    }
    let chars: usize = pool.iter().map(|c| c.snippet.chars().count()).sum();
    if pool.len() < THIN_CITATIONS || chars < THIN_CHARS {
        return true;
    }
    let mut tails: std::collections::HashMap<(&str, &str), usize> =
        std::collections::HashMap::new();
    for c in pool.iter().take(5) {
        if c.section.is_empty() || !c.note_id.is_empty() {
            continue;
        }
        let Some((_, tail)) = c.section.rsplit_once(" › ") else {
            continue;
        };
        *tails.entry((c.source_id.as_str(), tail)).or_default() += 1;
    }
    tails.values().any(|n| *n >= 2)
}

/// Does the question quote an identifier — a token with a digit in it,
/// `FM-2041`, `SK-1305`, `v2.3`, `10.0.0.1` — that the snippet contains
/// verbatim?
fn literal_hit(question: &str, snippet: &str) -> bool {
    let hay = snippet.to_lowercase();
    question
        .split(|c: char| {
            c.is_whitespace() || matches!(c, ',' | ';' | '?' | '!' | '(' | ')' | '"' | '\'')
        })
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|t| t.len() >= 3 && t.chars().any(|c| c.is_ascii_digit()))
        .any(|t| hay.contains(&t.to_lowercase()))
}

fn build_messages(question: &str, outline: &[OutlineEntry]) -> Vec<ChatTurn> {
    let mut lines = String::new();
    for (i, e) in outline.iter().enumerate() {
        let summary: String = e
            .summary
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(220)
            .collect();
        lines.push_str(&format!("{}. {} — {}\n", i + 1, e.chain, summary));
    }
    vec![
        ChatTurn::system(
            "You are choosing where to read in a document collection. Given a question \
             and a numbered outline (one line per section, with a one-line summary), reply \
             with the numbers of the sections most likely to contain the answer — at most \
             two, most likely first, comma-separated — or the word NONE if no section \
             fits. Numbers only, nothing else.",
        ),
        ChatTurn::user(format!("Question: {question}\n\nOutline:\n{lines}")),
    ]
}

/// A reply naming more sections than this has no opinion — bonsai answers
/// "1,2,3,…,14" when the summaries cannot say — and reads as NONE.
const SHOTGUN: usize = 4;

/// Parse "2, 7" / "7" / "NONE" into outline indexes (0-based), in order,
/// at most `MAX_PICKS`; a shotgun reply yields nothing.
pub fn parse_picks(reply: &str, len: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for tok in reply.split(|c: char| !c.is_ascii_digit()) {
        if tok.is_empty() {
            continue;
        }
        if let Ok(n) = tok.parse::<usize>() {
            if n >= 1 && n <= len && !out.contains(&(n - 1)) {
                out.push(n - 1);
            }
        }
    }
    if out.len() > SHOTGUN {
        return Vec::new();
    }
    out.truncate(MAX_PICKS);
    out
}

/// The escalation. Returns what was picked ("Manual › Chapter 2 …; …") for
/// the trace, or None when nothing fired or nothing changed. `pool` is
/// edited in place: picked passages move to the front, duplicates behind
/// them drop, and the length is preserved so callers' caps still hold.
pub async fn escalate(
    db: &Db,
    ai: &Ai,
    notebook_id: &str,
    question: &str,
    query_vec: &[f32],
    pool: &mut Vec<Citation>,
) -> Result<Option<String>> {
    if !should_escalate(question, pool) {
        return Ok(None);
    }
    let mut outline = db.notebook_outline(notebook_id).await?;
    if outline.is_empty() {
        return Ok(None);
    }
    outline.truncate(MAX_OUTLINE_LINES);
    let reply = ai
        .chat_role(Role::Small, &build_messages(question, &outline))
        .await?;
    crate::freshness::record_outcome(&reply);
    let picks = parse_picks(&reply.text, outline.len());
    if picks.is_empty() {
        return Ok(None);
    }
    let mut promoted: Vec<Citation> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for i in picks {
        let e = &outline[i];
        let passages = db
            .section_passages(
                notebook_id,
                query_vec,
                question,
                (&e.source_id, e.start, e.end),
                PASSAGES_PER_PICK,
            )
            .await?;
        if !passages.is_empty() {
            names.push(e.chain.clone());
        }
        for p in passages {
            if !promoted.iter().any(|q| q.chunk_id == p.chunk_id) {
                promoted.push(p);
            }
        }
    }
    if promoted.is_empty() {
        return Ok(None);
    }
    #[cfg(test)]
    if std::env::var("ALCHEMY_TRACE_OUTLINE").is_ok() {
        eprintln!(
            "    outline: {question:?} → {:?} → {} → {:?}",
            reply.text.trim(),
            names.join("; "),
            promoted
                .iter()
                .map(|c| c.chunk_id.as_str())
                .collect::<Vec<_>>()
        );
    }
    merge(pool, promoted);
    Ok(Some(names.join("; ")))
}

/// Reciprocal-rank fusion of the flat pool with the outline's passages.
/// A passage both the flat search and the outline vouch for outranks
/// either alone; a vouched passage the flat search never surfaced lands
/// behind the flat leader, not ahead of it. Measured against prepending
/// the picks (bonsai): topic MRR 0.88 vs 0.82, exact 0.33 vs 0.20; gemma
/// lands the same either way. The pool keeps its length so callers' caps
/// still hold.
fn merge(pool: &mut Vec<Citation>, promoted: Vec<Citation>) {
    const K: f64 = 60.0;
    let cap = pool.len().max(promoted.len());
    // (score, flat rank, outline rank, citation); ranks are 1-based, usize::MAX when absent.
    let mut scored: Vec<(f64, usize, usize, Citation)> = Vec::with_capacity(cap + 4);
    for (i, c) in pool.drain(..).enumerate() {
        scored.push((1.0 / (K + (i + 1) as f64), i + 1, usize::MAX, c));
    }
    for (j, p) in promoted.into_iter().enumerate() {
        let bonus = 1.0 / (K + (j + 1) as f64);
        if let Some(row) = scored.iter_mut().find(|r| r.3.chunk_id == p.chunk_id) {
            row.0 += bonus;
            row.2 = j + 1;
        } else {
            scored.push((bonus, usize::MAX, j + 1, p));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    scored.truncate(cap);
    *pool = scored.into_iter().map(|r| r.3).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cite(id: &str, source: &str, section: &str, snippet: &str) -> Citation {
        Citation {
            chunk_id: id.into(),
            source_id: source.into(),
            source_title: "Doc".into(),
            source_path: String::new(),
            note_id: String::new(),
            gist: false,
            snote: false,
            ordinal: 0,
            snippet: snippet.into(),
            distance: 0.0,
            section: section.into(),
        }
    }

    /// Thin pools escalate; so do look-alike subsections of one source;
    /// a healthy varied pool does not.
    #[test]
    fn escalation_trigger() {
        let long = "x".repeat(400);
        let q = "hinge bolt torque";
        assert!(
            should_escalate(q, &[cite("a", "s", "", &long)]),
            "thin: too few"
        );
        let varied: Vec<Citation> = (0..5)
            .map(|i| {
                cite(
                    &format!("c{i}"),
                    "s",
                    &format!("Doc › Ch {i} › Part {i}"),
                    &long,
                )
            })
            .collect();
        assert!(!should_escalate(q, &varied), "varied sections stay flat");
        let alike: Vec<Citation> = (0..5)
            .map(|i| {
                cite(
                    &format!("c{i}"),
                    "s",
                    &format!("Doc › Ch {i} › Torque Values"),
                    &long,
                )
            })
            .collect();
        assert!(
            should_escalate(q, &alike),
            "same subsection of many chapters"
        );
        let mut literal = alike.clone();
        literal[0].snippet = format!("Part FM-2041 is the hinge bolt. {long}");
        assert!(
            !should_escalate("what is part FM-2041?", &literal),
            "the leader carries the quoted identifier"
        );
        assert!(
            should_escalate("what is part FM-2041?", &alike),
            "identifier quoted but not in the leader"
        );
    }

    /// Fusion: a passage both sides vouch for leads; an outline-only
    /// passage lands behind the flat leader; the pool keeps its length.
    #[test]
    fn merge_fuses_rather_than_prepends() {
        let mut pool: Vec<Citation> = (0..5)
            .map(|i| cite(&format!("f{i}"), "s", "", "x"))
            .collect();
        merge(
            &mut pool,
            vec![cite("o1", "s", "", "x"), cite("f3", "s", "", "x")],
        );
        let ids: Vec<&str> = pool.iter().map(|c| c.chunk_id.as_str()).collect();
        assert_eq!(ids, vec!["f3", "f0", "o1", "f1", "f2"]);
    }

    #[test]
    fn picks_parse_in_order_and_bounded() {
        assert_eq!(parse_picks("7, 2", 10), vec![6, 1]);
        assert_eq!(parse_picks("Sections 3 and 3 and 12", 10), vec![2]);
        assert_eq!(parse_picks("NONE", 10), Vec::<usize>::new());
        assert_eq!(parse_picks("1,2,3", 10), vec![0, 1]);
        assert_eq!(
            parse_picks("1,2,3,4,5,6,7,8,9,10,11,12,13,14", 14),
            Vec::<usize>::new(),
            "shotgun reads as no opinion"
        );
    }
}
