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
//! degeneracy check, and an identifier-grounding check that rejects a gist
//! only on wholesale confabulation (a majority of its identifiers absent from
//! the source), not on the odd paraphrase or plural — three rounds of real
//! corpus proved per-token rejection threw out good gists. A gist that fails
//! the gate is dropped and the (source, hash) pair is remembered for this app
//! run so the sweep doesn't spin on an unwilling model.
//!
//! Phase 2 (RFC-infinite-context §2) rides the same sweep: once gists
//! converge, `ensure_enrichment` re-embeds one low-density page-capture
//! source (url/html) at a time, prepending an LLM-written situating sentence
//! to each chunk's embed input while leaving `Chunk.text`, ids, ordinals, and
//! the FTS index untouched — only the stored vector changes. Which sources
//! are enriched at which content hash is remembered in a small JSON marker in
//! the app-data dir; a lost or stale marker only ever costs recompute.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::Result;

use crate::ai::Ai;
use crate::db::{Db, GistRow, GIST_CHUNK_PREFIX};
use crate::inference::{ChatTurn, Role};
use crate::models::Source;

/// Gate bounds for a stored gist (RFC-infinite-context §1). Wide on purpose:
/// the min only rejects trivial one-liners, and the max only rejects runaway
/// output — a dense source legitimately summarizes to a couple thousand chars,
/// and one gist row per source makes index bloat a non-issue.
const GIST_MIN_CHARS: usize = 120;
const GIST_MAX_CHARS: usize = 3000;
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

/// Identifier-ish tokens: the exact strings a search would target, which the
/// model must not invent — as opposed to prose it is free to paraphrase. A
/// token qualifies as an identifier when it is snake_case (`thread_8f42`), a
/// letter-led token carrying a digit (`ERR-500`, `Kimi-K2.6`, `v1.0`), or
/// CamelCase with no hyphen (`CheckpointLoader`, `OpenAI`).
///
/// Deliberately NOT flagged (common in summaries, rarely verbatim): hyphenated
/// lowercase adjectives (`rust-based`); acronym-adjectives (`LLM-based`,
/// `AI-driven`), where the hyphen rules out the CamelCase branch; and
/// number-led prose (`3-point`, `2-week`, a bare hex `8b95e6`), since a real
/// code leads with a letter. Unicode dashes are treated as word separators so
/// "UI—along" is two words, and markdown emphasis is stripped from token
/// boundaries so "**Studio**" / "_v:1_" verify as the bare word.
fn identifier_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| {
        ch.is_whitespace()
            || ",;()[]{}\"'`".contains(ch)
            || matches!(ch, '\u{2014}' | '\u{2013}' | '\u{2012}')
    })
    .map(|t| t.trim_matches(|ch: char| ".:!?*_~#".contains(ch)))
    .filter(|t| t.chars().count() >= 4)
    .filter(|t| {
        let has_underscore = t.contains('_');
        let lettered_code = t.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && t.chars().any(|c| c.is_ascii_digit());
        let camel = !t.contains('-')
            && t.chars().skip(1).any(|c| c.is_ascii_uppercase())
            && t.chars().any(|c| c.is_ascii_lowercase());
        has_underscore || lettered_code || camel
    })
    .map(str::to_string)
    .collect()
}

/// Accept or reject a generated gist. `Err(reason)` means "store nothing" —
/// the caller falls back to prefix-only retrieval (today's behavior) and
/// logs the reason, so a run of rejections is diagnosable instead of opaque.
pub fn gate(candidate: &str, raw: &str) -> Result<String, String> {
    let gist = candidate.trim();
    let n = gist.chars().count();
    if n < GIST_MIN_CHARS {
        return Err(format!("too short ({n} chars)"));
    }
    if n > GIST_MAX_CHARS {
        return Err(format!("too long ({n} chars)"));
    }
    // Degeneracy: a looping model repeats lines; real prose doesn't.
    let lines: Vec<&str> = gist
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let distinct: HashSet<&&str> = lines.iter().collect();
    if lines.len() >= 4 && distinct.len() * 2 < lines.len() {
        return Err("degenerate (repeated lines)".into());
    }
    // Identifier grounding, softened from per-token rejection after three
    // rounds of real-corpus false positives (RFC-infinite-context §1): a good
    // summary legitimately paraphrases, pluralizes ("RecordBatch" →
    // "RecordBatches"), and names entities the extractor dropped from the
    // body, so a single unverified token is not evidence of hallucination.
    // Reject only on WHOLESALE confabulation — several unverified identifiers
    // AND a majority of them unverified — which is what an untethered model
    // actually produces; one or two strays ride along.
    let raw_lower = raw.to_lowercase();
    let idents = identifier_tokens(gist);
    let unverified = idents
        .iter()
        .filter(|t| !raw_lower.contains(&t.to_lowercase()))
        .count();
    if unverified >= 3 && unverified * 2 > idents.len() {
        return Err(format!(
            "{unverified} of {} identifiers unverified (likely confabulated)",
            idents.len()
        ));
    }
    Ok(gist.to_string())
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
        // Verify identifiers against the title too, not just the body: a
        // model naturally names something from the title ("Kimi-K2.6" lives in
        // the source's title, not its prose), and readability extraction can
        // drop it from the content.
        let haystack = format!("{}\n{}", source.title, source.content);
        let gist = match gate(&reply, &haystack) {
            Ok(g) => g,
            Err(reason) => {
                eprintln!("gist: gate rejected \"{}\": {reason}", source.title);
                remember_refusal(&want.source_id, hash);
                continue;
            }
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

// ---- Phase 2: distilled embeddings for low-density page captures ----------

/// Situating-sentence gate bounds (RFC-infinite-context §2). Tighter than the
/// gist bounds: this is one sentence, not a paragraph.
const SITUATE_MIN_CHARS: usize = 40;
const SITUATE_MAX_CHARS: usize = 300;
/// How much of a chunk the situating prompt sees — a Small model gets the head
/// only; the sentence orients the chunk, it doesn't re-summarize it.
const SITUATE_CHUNK_HEAD: usize = 1200;
/// How much of the source gist the situating prompt sees as document context.
const SITUATE_GIST_HEAD: usize = 600;
/// Marker file (app-data dir) recording which sources are enriched at which
/// content hash. Self-healing: a missing/corrupt file just means re-enrich.
const ENRICH_STATE_FILE: &str = "enrichment.json";

/// (source_id → content hash) pairs we could not enrich this app run — a
/// chunker-drift skip, remembered so the sweep doesn't re-select the same
/// unworkable source every batch (the REFUSED idea at source scope).
static ENRICH_REFUSED: Mutex<Option<HashMap<String, i32>>> = Mutex::new(None);

fn enrich_refused(source_id: &str, hash: i32) -> bool {
    let guard = ENRICH_REFUSED.lock().unwrap();
    guard
        .as_ref()
        .and_then(|m| m.get(source_id))
        .is_some_and(|h| *h == hash)
}

fn remember_enrich_refusal(source_id: &str, hash: i32) {
    let mut guard = ENRICH_REFUSED.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(source_id.to_string(), hash);
}

fn enrich_state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(ENRICH_STATE_FILE)
}

/// Load the enrichment marker. Any read/parse failure yields an empty map, so
/// a lost or corrupt file self-heals into a re-enrichment (recompute only).
fn load_enrich_state(dir: &Path) -> HashMap<String, i32> {
    std::fs::read_to_string(enrich_state_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the enrichment marker. Best-effort — the state is always
/// re-derivable from content hashes, so a failed write never blocks the sweep.
fn save_enrich_state(dir: &Path, state: &HashMap<String, i32>) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let bytes = serde_json::to_vec(state).unwrap_or_default();
        std::fs::write(enrich_state_path(dir), bytes)
    };
    if let Err(err) = write() {
        eprintln!("enrich: marker write failed: {err}");
    }
}

/// The situating prompt. One plain sentence out — it orients the chunk within
/// its document; the chunk's verbatim text follows it in the embed input.
fn build_situating_messages(title: &str, gist: Option<&str>, chunk: &str) -> Vec<ChatTurn> {
    let head: String = chunk.chars().take(SITUATE_CHUNK_HEAD).collect();
    let overview = gist
        .map(|g| {
            let g: String = g.chars().take(SITUATE_GIST_HEAD).collect();
            format!("Document overview:\n{g}\n\n")
        })
        .unwrap_or_default();
    vec![
        ChatTurn::system(
            "You situate a passage inside its document for a search index. Reply \
             with ONE plain sentence — no preamble, no quotes, no markdown.",
        ),
        ChatTurn::user(format!(
            "Document titled \"{title}\".\n{overview}In one sentence, say what the \
             passage below covers and how it fits the document. Use only facts from \
             the passage or overview; invent no names, codes, or numbers.\n\n\
             Passage:\n---\n{head}",
        )),
    ]
}

/// Accept or reject a situating sentence. `None` means "keep the chunk's
/// current vector" — the safe degrade to today's prefix-only embedding.
pub fn situating_gate(candidate: &str, raw: &str) -> Option<String> {
    // One line only: a Small model sometimes tacks on a stray second line.
    let sentence = candidate
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let n = sentence.chars().count();
    if !(SITUATE_MIN_CHARS..=SITUATE_MAX_CHARS).contains(&n) {
        return None;
    }
    // Degeneracy: a looping model repeats one token; a real sentence doesn't.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for w in sentence
        .split_whitespace()
        .filter(|w| w.chars().count() >= 4)
    {
        let c = counts.entry(w).or_default();
        *c += 1;
        if *c >= 4 {
            return None;
        }
    }
    // Identifier grounding, softened like the gist gate: reject only on
    // wholesale confabulation, not a lone stray. This sentence is one chunk's
    // embed context, so an odd unverified token is low-harm; the threshold is
    // 2 (not the gist's 3) because a single sentence carries few identifiers.
    let raw_lower = raw.to_lowercase();
    let idents = identifier_tokens(&sentence);
    let unverified = idents
        .iter()
        .filter(|t| !raw_lower.contains(&t.to_lowercase()))
        .count();
    if unverified >= 2 && unverified * 2 > idents.len() {
        return None;
    }
    Some(sentence)
}

/// Outcome of enriching one source's chunks.
enum EnrichOutcome {
    /// Processed at the current hash — mark it so the sweep moves on.
    Enriched,
    /// Chunker drift: the re-derived chunk set doesn't line up with the stored
    /// rows, so rewriting would risk the ids. Skip (and refuse for the run).
    Skip,
    /// The Small role is unavailable — end enrichment for this sweep.
    EngineDown,
}

/// Re-embed one source's chunks with a per-chunk situating sentence prepended
/// to the existing embed input (RFC-infinite-context §2). `Chunk.text`, the
/// chunk ids, ordinals, and FTS content are all preserved — only the stored
/// vectors change, and only for chunks whose situating sentence passed the
/// gate (the rest keep today's prefix-only vector).
async fn enrich_source(
    db: &Db,
    ai: &Ai,
    source: &Source,
    gist: Option<&str>,
) -> Result<EnrichOutcome> {
    // Reproduce the exact stored chunk set through the same path the import
    // used (chunk_source, boilerplate filter and all) so re-chunking lines up
    // 1:1 with the rows — ordinal i of the fresh chunks is row i.
    let extracted = crate::ingest::Extracted {
        title: source.title.clone(),
        source_type: source.source_type.clone(),
        url: source.url.clone(),
        text: source.content.clone(),
    };
    let chunks = crate::ingest::chunk_source(&extracted, None);
    let rows = db.source_chunk_rows(&source.id).await?;
    if rows.is_empty() || chunks.len() != rows.len() {
        return Ok(EnrichOutcome::Skip);
    }

    let mut inputs: Vec<String> = Vec::with_capacity(chunks.len());
    let mut passed = 0usize;
    for (chunk, row) in chunks.iter().zip(&rows) {
        // Both are ordinal-ordered; if the verbatim text disagrees we are not
        // looking at the same chunk — bail rather than corrupt a citation id.
        if chunk.text != row.2 {
            return Ok(EnrichOutcome::Skip);
        }
        let messages = build_situating_messages(&source.title, gist, &chunk.text);
        let reply = match ai.chat_role(Role::Small, &messages).await {
            Ok(out) => out.text,
            Err(err) => {
                eprintln!(
                    "enrich: Small role failed for \"{}\": {err:#}",
                    source.title
                );
                return Ok(EnrichOutcome::EngineDown);
            }
        };
        match situating_gate(&reply, &source.content) {
            Some(sentence) => {
                inputs.push(format!("{sentence}\n{}", chunk.embed_text));
                passed += 1;
            }
            // Gate rejected the sentence: keep this chunk's current vector.
            None => inputs.push(chunk.embed_text.clone()),
        }
    }

    // Nothing usable came back: don't churn the index re-embedding identical
    // inputs — just mark the source processed (a bad model won't do better on
    // the same content until it changes).
    if passed == 0 {
        return Ok(EnrichOutcome::Enriched);
    }

    let embeddings = ai.embed(&inputs).await?;
    db.reembed_source_chunks(&source.notebook_id, &source.id, &rows, &embeddings)
        .await?;
    Ok(EnrichOutcome::Enriched)
}

/// Enrich one un-enriched page-capture source per call (they're expensive:
/// one sequential Small-role call per chunk). Returns 1 if a source was
/// enriched, 0 when there is nothing left to do this sweep. Mirrors
/// `ensure_gists`: desired state is derived, the marker is diffed, only the
/// difference is touched.
pub async fn ensure_enrichment(db: &Db, ai: &Ai) -> Result<usize> {
    let dir = ai.data_dir().to_path_buf();
    let mut state = load_enrich_state(&dir);

    // Desired: every eligible page-capture source (url/html), with the hash
    // its enrichment should carry. Code/pdf/prose/mac keep today's embedding.
    let mut current: HashSet<String> = HashSet::new();
    let mut candidates: Vec<String> = Vec::new(); // eligible source ids
    for nb in db.list_notebooks().await? {
        for s in db.list_sources(&nb.id).await? {
            if !crate::ingest::is_page_capture_type(&s.source_type)
                || s.chunk_count == 0
                || s.char_count < MIN_SOURCE_CHARS
            {
                continue;
            }
            current.insert(s.id.clone());
            candidates.push(s.id);
        }
    }

    // Self-heal: drop marker entries whose source is gone. A lost marker only
    // costs recompute, so pruning is safe and keeps the file bounded.
    let before = state.len();
    state.retain(|sid, _| current.contains(sid));
    if state.len() != before {
        save_enrich_state(&dir, &state);
    }

    // Source gists double as document context for the situating prompt.
    let gists: HashMap<String, String> = db
        .list_gists()
        .await?
        .into_iter()
        .map(|g: GistRow| (g.source_id, g.text))
        .collect();

    for source_id in candidates {
        let source = match db.get_source(&source_id).await? {
            Some(s) => s,
            None => continue,
        };
        let hash = content_hash(&source.content);
        if state.get(&source_id) == Some(&hash) || enrich_refused(&source_id, hash) {
            continue;
        }
        match enrich_source(db, ai, &source, gists.get(&source_id).map(String::as_str)).await? {
            EnrichOutcome::Enriched => {
                state.insert(source_id, hash);
                save_enrich_state(&dir, &state);
                return Ok(1);
            }
            EnrichOutcome::Skip => {
                remember_enrich_refusal(&source_id, hash);
                continue;
            }
            EnrichOutcome::EngineDown => return Ok(0),
        }
    }
    Ok(0)
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
                // Gists converged; spend the batch on chunk enrichment (RFC §2
                // "gists first, chunks only when idle"). Enrichment ends the
                // sweep only when it, too, has nothing left to do.
                Ok((0, 0)) => match ensure_enrichment(&db, &ai).await {
                    Ok(0) => break,
                    Ok(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                    Err(err) => {
                        eprintln!("enrichment sweep failed: {err:#}");
                        break;
                    }
                },
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
        assert!(gate("too short", raw).is_err());
        let long = "word ".repeat(700); // 3500 chars, past GIST_MAX_CHARS
        assert!(gate(&long, raw).is_err());
    }

    #[test]
    fn gate_tolerates_strays_but_rejects_confabulation() {
        let raw = "Retries use ERR-500-RETRY. The loader is CheckpointLoader in \
                   checkpoint_loader.cc for net-45 jobs.";
        // Grounded: every identifier present in the source. Passes.
        let good = "This runbook covers retries via ERR-500-RETRY and the CheckpointLoader \
                    defined in checkpoint_loader.cc, explaining net-45 job handling so it can \
                    answer how retries and loading behave during a stalled restore for a team.";
        assert!(gate(good, raw).is_ok(), "{:?}", gate(good, raw));
        // One unverified identifier (a paraphrase / plural / dropped id) rides
        // along — no longer grounds for rejecting the whole gist.
        let one_stray = good.replace("net-45", "net-90");
        assert!(
            gate(&one_stray, raw).is_ok(),
            "a single stray must be tolerated: {:?}",
            gate(&one_stray, raw)
        );
        // Wholesale confabulation — a majority of identifiers invented — rejects.
        let confab = "This runbook covers ERR-909-FAKE and the PhantomLoader defined in \
                      phantom_loader.cc, explaining zeta-99 job handling so it can answer \
                      how the invented retries and loading behave during some restore now.";
        assert!(
            gate(confab, raw).is_err(),
            "confabulation must reject: {:?}",
            gate(confab, raw)
        );
    }

    #[test]
    fn gate_rejects_looping_output() {
        let line = "It covers the vendor payment process end to end.\n";
        let looped = format!(
            "This document describes vendor payments in detail for the team.\n{}",
            line.repeat(8)
        );
        assert!(gate(&looped, "vendor payments").is_err());
    }

    #[test]
    fn situating_gate_accepts_a_grounded_sentence() {
        let raw = "The CheckpointLoader restores from the last manifest and retries \
                   with ERR-500-RETRY after a ten second wait.";
        let good = "This passage explains how CheckpointLoader restores state and \
                    when it issues ERR-500-RETRY during a stalled restore.";
        assert_eq!(situating_gate(good, raw).as_deref(), Some(good));
    }

    #[test]
    fn situating_gate_rejects_bounds_and_hallucinations() {
        let raw = "Vendor invoices are paid by wire on net-45 terms.";
        // Too short.
        assert!(situating_gate("Payments.", raw).is_none());
        // Too long (well past 300 chars).
        let long = "word ".repeat(120);
        assert!(situating_gate(&long, raw).is_none());
        // A lone invented identifier now rides along (softened gate).
        let one_stray = "This passage covers vendor wire payments and the ERR-999-FAKE path \
                         used when a remittance is disputed by procurement on net-45 terms.";
        assert!(
            situating_gate(one_stray, raw).is_some(),
            "one stray tolerated"
        );
        // Wholesale confabulation — a majority of identifiers invented — rejects.
        let confab = "This passage covers ERR-909-FAKE and the PhantomLoader path via \
                      zeta_bad_ref when a remittance is disputed on some terms.";
        assert!(
            situating_gate(confab, raw).is_none(),
            "confabulated sentence must be rejected"
        );
    }

    #[test]
    fn situating_gate_takes_first_line_and_catches_loops() {
        let raw = "The onboarding guide covers workspace setup for a new teammate.";
        // A trailing second line is dropped; the grounded first line passes.
        let multi = "This passage introduces workspace setup for onboarding a teammate.\n\
                     Note: generated by assistant.";
        assert_eq!(
            situating_gate(multi, raw).as_deref(),
            Some("This passage introduces workspace setup for onboarding a teammate.")
        );
        // A single looping token trips the degeneracy check.
        let looped = "setup setup setup setup setup for onboarding a teammate here now";
        assert!(situating_gate(looped, raw).is_none());
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

    /// Regression: markdown emphasis around a token must not defeat the
    /// verbatim source check (live models emit "**Studio**", "_v:1_").
    #[test]
    fn identifier_tokens_unwrap_markdown_emphasis() {
        let toks =
            identifier_tokens("It documents the **Studio** panel and the **ERR-9917** code.");
        // No wrapper survives to be checked verbatim.
        assert!(!toks.iter().any(|t| t.contains('*')), "got {toks:?}");
        // A real code still gets enforced — in its bare, unwrapped form.
        assert!(toks.contains(&"ERR-9917".to_string()), "got {toks:?}");
        // "Studio" unwraps to a leading-capital word — prose, not an
        // identifier — so it is (correctly) NOT enforced. Pre-fix, the leading
        // `*` made the capital "internal" and the whole gist was rejected.
        assert!(!toks.iter().any(|t| t == "Studio"), "got {toks:?}");
        // The gate clears when the enforced bare word is present in the source
        // even though the gist wrote it wrapped in markdown.
        let raw = "The Studio panel surfaces the ERR-9917 code path for two-host overviews.";
        let gist = "This document describes the **Studio** panel and the **ERR-9917** code \
                    path it surfaces, covering the two-host overview flow so it can answer \
                    questions about the panel, the error path, and how the overall \
                    generation works for a reader exploring the studio surface right now.";
        assert!(gate(gist, raw).is_ok(), "{:?}", gate(gist, raw));
    }

    /// Regression for the false-positive classes seen on the first real
    /// corpus: em-dash word joins, acronym-adjectives, and number-led prose.
    #[test]
    fn identifier_tokens_ignore_prose_lookalikes() {
        // Em-dash is a separator, not part of an identifier ("UI—along").
        let toks = identifier_tokens("The UI—along with the panel—stays inline.");
        assert!(!toks.iter().any(|t| t.contains('\u{2014}')), "got {toks:?}");
        // Acronym-adjectives and number-led prose are not codes.
        let toks =
            identifier_tokens("An LLM-based, AI-driven, 3-point plan over 2-week sprints in 2026.");
        assert!(
            toks.is_empty(),
            "prose look-alikes must not flag, got {toks:?}"
        );
        // A letter-led token with a digit is still a real code and enforced.
        let codes = identifier_tokens("Runs GLM-5.1 and Kimi-K2.6 today.");
        assert!(codes.contains(&"GLM-5.1".to_string()), "got {codes:?}");
        assert!(codes.contains(&"Kimi-K2.6".to_string()), "got {codes:?}");
    }

    /// A code that lives only in the source *title* verifies: the sweep passes
    /// title + body as the haystack, so a gist naming it clears the gate.
    #[test]
    fn gate_verifies_identifiers_present_only_in_title() {
        let title = "GitHub - ollama/ollama: running Kimi-K2.6 and GLM-5.1";
        let body = "This project lets you run open models locally with one command.";
        let haystack = format!("{title}\n{body}");
        let gist = "This page documents the ollama project and how it runs models like \
                    Kimi-K2.6 and GLM-5.1 locally, so it can answer which models are \
                    supported and how to get them running from a single command line here.";
        assert!(gate(gist, &haystack).is_ok(), "{:?}", gate(gist, &haystack));
    }

    /// Regression: hyphenated lowercase adjectives are prose, not identifiers.
    /// Flagging them made the identifier gate reject nearly every real gist
    /// for a repo or article (the words rarely appear verbatim in the source).
    #[test]
    fn identifier_tokens_ignore_hyphenated_adjectives() {
        let toks = identifier_tokens(
            "A rust-based command-line tool that is open-source, cross-platform, \
             and well-documented for end-users.",
        );
        assert!(
            toks.is_empty(),
            "hyphenated adjectives must not be treated as identifiers, got {toks:?}"
        );
        // A gist describing such a project passes the identifier gate even
        // though the adjectives never appear verbatim in the source (long
        // enough here to clear the length gate and isolate the identifier
        // check).
        let raw = "This project is written in Rust. It ships a CLI. The code is public.";
        let gist = "This project provides a rust-based command-line tool that is \
                    open-source and cross-platform. The write-up is well-documented and \
                    beginner-friendly, so it can answer questions about installation, \
                    day-to-day usage, configuration, and troubleshooting for the new \
                    contributors who are getting started with the codebase and its docs.";
        assert!(
            gate(gist, raw).is_ok(),
            "adjective-heavy prose should clear the identifier gate: {:?}",
            gate(gist, raw)
        );
    }
}
