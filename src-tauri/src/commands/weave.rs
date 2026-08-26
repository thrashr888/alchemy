//! The Weave (RFC-v12-steward pillar 3): judgment on arrival, and again
//! nightly over whatever changed while nobody was looking.
//! When a source's content changes — or a top-level source lands — the new
//! text is weighed against the notebook's active ledger entries, and the
//! Small role renders a verdict per close pair: corroborates / contradicts /
//! supersedes / extends / unrelated. Status-driving verdicts move the
//! ledger's lifecycle, with provenance appended to the entry's why.
//!
//! Gist discipline throughout: cosine floor before any model call, hard
//! caps, strict parse-or-skip, and asymmetric transitions — corroborate
//! only lifts `asserted`, contradicted never auto-clears, supersede touches
//! facts and decisions only. A wrong verdict must never destroy user state,
//! so the Weave only ever moves statuses and appends whys.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::ai::{Ai, ChatTurn};
use crate::db::Db;
use crate::inference::Role;
use crate::models::LedgerEntry;

use super::{cosine, now};

/// A folder sync can fire many reingests at once; past this many concurrent
/// judgments, arrivals skip (the next change retries — nothing is owed).
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
const MAX_IN_FLIGHT: usize = 3;
/// Below this similarity a pair isn't worth a model call.
const COSINE_FLOOR: f32 = 0.45;
/// At most this many entries judged per arrival.
const MAX_PAIRS: usize = 4;
/// The changed text handed to the judge, capped.
const TEXT_CAP: usize = 4_000;
/// Entries considered per pass (newest first) — the active working set.
const MAX_ENTRIES: usize = 20;

/// Fire-and-forget: weigh `changed_text` against the notebook's ledger.
/// Takes owned handles (the `gist::spawn_sweep` shape) so callers without a
/// Tauri handle — reingest — can spawn it.
pub(crate) fn spawn_weave(
    db: Arc<Db>,
    ai: Ai,
    notebook_id: String,
    source_title: String,
    changed_text: String,
) {
    let changed: String = changed_text.chars().take(TEXT_CAP).collect();
    if changed.trim().chars().count() < 80 {
        return; // too little signal to judge
    }
    if IN_FLIGHT.load(Ordering::Relaxed) >= MAX_IN_FLIGHT {
        return;
    }
    tauri::async_runtime::spawn(async move {
        IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = weave_pass(&db, &ai, &notebook_id, &source_title, &changed).await {
            crate::note!("weave: {err:#}");
        }
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    });
}

/// At most this many changed sources re-judged per night. The cap is the
/// point: a night that re-judges everything is a night that spends its whole
/// budget on the corpus's least interesting corners.
const MAX_NIGHTLY_SOURCES: usize = 8;

/// Nightly re-judgment (freshness.rs stage 2, "verification").
///
/// Arrival-time weaving catches a source the moment it lands, which misses
/// the case that actually matters: a page the user is watching changed at
/// 3 AM, and the conclusion it undermines was written in March. This walks
/// what changed since `since` and weighs each one against the ledger again.
///
/// Budget-checked per source rather than once up front, because the cost is
/// per judgment and a night can run out halfway through the list.
pub(crate) fn spawn_nightly(db: Arc<Db>, ai: Ai, since: i64, budget: String) {
    if IN_FLIGHT.load(Ordering::Relaxed) >= MAX_IN_FLIGHT {
        return;
    }
    tauri::async_runtime::spawn(async move {
        IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = nightly_pass(&db, &ai, since, &budget).await {
            crate::note!("weave nightly: {err:#}");
        }
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    });
}

async fn nightly_pass(db: &Db, ai: &Ai, since: i64, budget: &str) -> anyhow::Result<()> {
    let events = db.source_events_since(since).await.unwrap_or_default();
    if events.is_empty() {
        return Ok(());
    }
    // Newest change per source: a page that changed three times tonight is
    // one judgment, against its current text.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut targets: Vec<&crate::models::SourceEvent> = Vec::new();
    for e in &events {
        if seen.insert(e.source_id.as_str()) {
            targets.push(e);
        }
        if targets.len() >= MAX_NIGHTLY_SOURCES {
            break;
        }
    }

    let archived = db.archived_notebook_ids().await.unwrap_or_default();
    let mut judged = 0;
    for event in targets {
        // Archiving is the user saying they are done with a notebook; its
        // conclusions are not re-litigated overnight.
        if archived.contains(&event.notebook_id) {
            continue;
        }
        if !crate::freshness::has_budget(budget) {
            crate::note!("weave nightly: budget spent after {judged} sources");
            break;
        }
        // Prefer the diff the watcher already computed - it is the part that
        // changed, which is the part worth judging. Fall back to the source
        // text when the change was not textual.
        let text = if event.diff.trim().chars().count() >= 80 {
            event.diff.clone()
        } else {
            db.source_content(&event.source_id)
                .await
                .unwrap_or_default()
        };
        let changed: String = text.chars().take(TEXT_CAP).collect();
        if changed.trim().chars().count() < 80 {
            continue;
        }
        if let Err(err) =
            weave_pass(db, ai, &event.notebook_id, &event.source_title, &changed).await
        {
            crate::note!("weave nightly: {} failed: {err:#}", event.source_title);
        }
        judged += 1;
    }
    if judged > 0 {
        crate::note!("weave nightly: re-judged {judged} changed sources");
    }
    Ok(())
}

async fn weave_pass(
    db: &Db,
    ai: &Ai,
    notebook_id: &str,
    source_title: &str,
    changed: &str,
) -> anyhow::Result<()> {
    // Judgeable rows: active statuses only, logs never (a log is what
    // happened — nothing to corroborate).
    let entries: Vec<LedgerEntry> = db
        .list_ledger(notebook_id)
        .await?
        .into_iter()
        .filter(|e| {
            matches!(
                (e.kind.as_str(), e.status.as_str()),
                ("assertion", "asserted")
                    | ("assertion", "corroborated")
                    | ("fact", "current")
                    | ("decision", "decided")
                    | ("question", "open")
            )
        })
        .take(MAX_ENTRIES)
        .collect();
    if entries.is_empty() {
        return Ok(());
    }

    // One embed call covers the entries and the arrival.
    let mut inputs: Vec<String> = entries.iter().map(|e| e.text.clone()).collect();
    inputs.push(changed.to_string());
    let vectors = ai.embed(&inputs).await?;
    let (entry_vecs, changed_vec) = vectors.split_at(entries.len());
    let Some(changed_vec) = changed_vec.first() else {
        return Ok(());
    };
    let mut scored: Vec<(usize, f32)> = entry_vecs
        .iter()
        .enumerate()
        .map(|(i, v)| (i, cosine(v, changed_vec)))
        .filter(|(_, score)| *score >= COSINE_FLOOR)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(MAX_PAIRS);

    let mut moved = 0u32;
    for (idx, _score) in scored {
        let entry = &entries[idx];
        let Some((verdict, reason)) = judge(ai, entry, source_title, changed).await else {
            continue;
        };
        // Asymmetric on purpose: corroboration only lifts a fresh assertion;
        // contradiction sticks until a human (or a later, wiser pass) clears
        // it; supersession is for facts and decisions.
        let next = match (entry.kind.as_str(), entry.status.as_str(), verdict.as_str()) {
            ("assertion", "asserted", "corroborates") => Some("corroborated"),
            ("assertion", _, "contradicts") => Some("contradicted"),
            ("fact", "current", "contradicts" | "supersedes") => Some("superseded"),
            ("decision", "decided", "supersedes") => Some("superseded"),
            _ => None,
        };
        let Some(next) = next else { continue };
        let mut updated = entry.clone();
        updated.status = next.to_string();
        let stamp = chrono::Local::now().format("%Y-%m-%d").to_string();
        let line = format!("{stamp}: {next} by \u{201c}{source_title}\u{201d} ({reason})");
        updated.why = if updated.why.is_empty() {
            line
        } else {
            format!("{}\n{line}", updated.why)
        };
        updated.updated_at = now();
        db.update_ledger_entry(&updated).await?;
        moved += 1;
        crate::note!(
            "weave: \u{201c}{}\u{2026}\u{201d} \u{2192} {next} (per {source_title})",
            entry.text.chars().take(48).collect::<String>()
        );
    }
    if moved > 0 {
        crate::note!("weave: {moved} ledger row(s) moved for notebook {notebook_id}");
    }
    Ok(())
}

/// One strict verdict. Anything malformed, hedged, or outside the vocabulary
/// is a skip — the pass must opt IN to acting.
async fn judge(
    ai: &Ai,
    entry: &LedgerEntry,
    source_title: &str,
    changed: &str,
) -> Option<(String, String)> {
    let messages = vec![
        ChatTurn::system(
            "You weigh newly arrived text against one recorded claim. Reply with exactly two \
             lines:\nVERDICT: one of corroborates | contradicts | supersedes | extends | \
             unrelated\nREASON: one short sentence naming the specific evidence.\nBe \
             conservative: contradicts requires the new text to be INCOMPATIBLE with the \
             claim, not merely different in emphasis; supersedes requires the new text to \
             REPLACE the claim with a newer state of affairs. When unsure: unrelated.",
        ),
        ChatTurn::user(format!(
            "RECORDED {} ({}):\n{}\n\nNEW TEXT from \u{201c}{}\u{201d}:\n{}",
            entry.kind.to_uppercase(),
            entry.status,
            entry.text,
            source_title,
            changed,
        )),
    ];
    let out = ai.chat_role(Role::Small, &messages).await.ok()?;
    crate::freshness::record_outcome(&out);
    let reply = out.text;
    let mut verdict = None;
    let mut reason = String::new();
    for line in reply.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("VERDICT:") {
            let word = rest.trim().to_lowercase();
            if [
                "corroborates",
                "contradicts",
                "supersedes",
                "extends",
                "unrelated",
            ]
            .contains(&word.as_str())
            {
                verdict = Some(word);
            }
        } else if let Some(rest) = line.strip_prefix("REASON:") {
            reason = rest.trim().chars().take(240).collect();
        }
    }
    match verdict {
        Some(v) if v != "unrelated" && v != "extends" && !reason.is_empty() => Some((v, reason)),
        _ => None,
    }
}
