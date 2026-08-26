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

/// One nightly pass at a time. Separate from `IN_FLIGHT` on purpose - see
/// `spawn_nightly`.
static NIGHTLY_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    // One nightly pass at a time, and never blocked by arrival judgments:
    // the pass is serial internally, so it is not the pile-up the arrival
    // cap guards against.
    if NIGHTLY_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        match nightly_pass(&db, &ai, since, &budget).await {
            Ok(true) => {}
            // Work that did not happen must not be marked done.
            Ok(false) => crate::scheduler::rewind_weave_stamp(since),
            Err(err) => {
                crate::note!("weave nightly: {err:#}");
                crate::scheduler::rewind_weave_stamp(since);
            }
        }
        NIGHTLY_RUNNING.store(false, Ordering::SeqCst);
    });
}

async fn nightly_pass(db: &Db, ai: &Ai, since: i64, budget: &str) -> anyhow::Result<bool> {
    let events = db.source_events_since(since).await.unwrap_or_default();
    if events.is_empty() {
        crate::note!("weave nightly: nothing changed since the last pass");
        return Ok(true);
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
        match weave_pass(db, ai, &event.notebook_id, &event.source_title, &changed).await {
            Ok(true) => judged += 1,
            // The model never answered. Nothing was judged, so nothing may be
            // recorded as judged - and the window must stay open so the next
            // pass retries these changes instead of skipping them forever.
            Ok(false) => {
                crate::note!(
                    "weave nightly: the model was unavailable after {judged} sources; \
                     leaving the rest for the next pass"
                );
                return Ok(false);
            }
            Err(err) => crate::note!("weave nightly: {} failed: {err:#}", event.source_title),
        }
    }
    crate::note!("weave nightly: judged {judged} changed sources");
    Ok(true)
}

/// Judge one changed text against the notebook's ledger. `Ok(false)` means
/// the engine never answered - the work did not happen and must not be
/// recorded as if it had.
async fn weave_pass(
    db: &Db,
    ai: &Ai,
    notebook_id: &str,
    source_title: &str,
    changed: &str,
) -> anyhow::Result<bool> {
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
        return Ok(true);
    }

    // One embed call covers the entries and the arrival.
    let mut inputs: Vec<String> = entries.iter().map(|e| e.text.clone()).collect();
    inputs.push(changed.to_string());
    let vectors = ai.embed(&inputs).await?;
    let (entry_vecs, changed_vec) = vectors.split_at(entries.len());
    let Some(changed_vec) = changed_vec.first() else {
        return Ok(true);
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
        let (verdict, reason) = match judge(ai, entry, source_title, changed).await {
            Judgment::Verdict(v, r) => (v, r),
            Judgment::NoOpinion => continue,
            // Stop rather than grinding through the remaining pairs: if the
            // engine is down for one it is down for all, and the caller needs
            // to know this source was never really judged.
            Judgment::EngineDown => return Ok(false),
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
    Ok(true)
}

/// One strict verdict. Anything malformed, hedged, or outside the vocabulary
/// is a skip — the pass must opt IN to acting.
/// What came back from one judgment. `EngineDown` is deliberately separate
/// from `NoOpinion`: a night that could not ask must never look like a night
/// that asked and found nothing.
pub(crate) enum Judgment {
    Verdict(String, String),
    NoOpinion,
    EngineDown,
}

async fn judge(ai: &Ai, entry: &LedgerEntry, source_title: &str, changed: &str) -> Judgment {
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
    let out = match ai.chat_role(Role::Small, &messages).await {
        Ok(out) => out,
        Err(err) => {
            crate::note!("weave: judge unavailable: {err:#}");
            return Judgment::EngineDown;
        }
    };
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
        Some(v) if v != "unrelated" && v != "extends" && !reason.is_empty() => {
            Judgment::Verdict(v, reason)
        }
        // The model answered and had nothing status-driving to say. That is
        // a real result, and the quiet night it produces is honest.
        _ => Judgment::NoOpinion,
    }
}
