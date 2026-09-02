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
use crate::db::{Db, GistRow, SectionGist, GIST_CHUNK_PREFIX};
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
        // WITH content: `list_sources` strips it, which silently disabled
        // the staleness fast-path below — every source, converged or not,
        // then paid a full `get_source` scan per batch, and the scheduler
        // ticks this sweep once a minute. One content scan per notebook
        // here makes the hash real and a converged corpus cost nothing.
        for s in db.sources_with_content(&nb.id).await? {
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
            Ok(out) => {
                crate::freshness::record_outcome(&out);
                out.text
            }
            Err(err) => {
                // Engine trouble ends the batch — the next sweep retries.
                crate::note!("gist: generation failed for \"{}\": {err:#}", source.title);
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
                crate::note!("gist: gate rejected \"{}\": {reason}", source.title);
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

// ---- Section summaries (docs/RFC-outline-index.md Phase 2) ---------------

/// A source needs at least this many top-level sections to earn section
/// summaries — below it, the source gist already says what each part is.
const SECTION_MIN_SECTIONS: usize = 3;
/// And at most this many are summarized: a 300-page manual with 14
/// chapters is 14 calls; an outline with hundreds of h1s is a table of
/// contents, and the source gist has to do.
const SECTION_MAX_SECTIONS: usize = 40;
/// How much of a section the summarizer reads.
const SECTION_HEAD_CHARS: usize = 3000;
/// Summaries written per sweep pass, matching the gist budget's restraint.
const SECTION_SWEEP_BUDGET: usize = 6;
const SECTION_MIN_CHARS: usize = 40;
const SECTION_MAX_CHARS: usize = 700;
/// Where the per-source stamp lives: source id → hash of its section
/// spans and headings, so a re-chunk (or a new h1) reopens the source and
/// an unchanged one costs nothing.
const SECTION_STATE_FILE: &str = "section-gists.json";

/// One top-level section of a stored source, from its chunk rows: a `# `
/// heading chunk opens a section that runs to the next one.
pub struct Section {
    pub heading: String,
    pub start: i32,
    pub end: i32,
    pub text: String,
}

/// The top-level sections of a source's stored chunks (ordinal order).
/// Heading chunks keep their heading line verbatim, so the outline is a
/// scan, no stored tree needed.
pub fn sections_of(rows: &[(String, i32, String)]) -> Vec<Section> {
    let mut rows: Vec<&(String, i32, String)> = rows.iter().collect();
    rows.sort_by_key(|r| r.1);
    let mut out: Vec<Section> = Vec::new();
    for (_, ord, text) in rows {
        let first = text.lines().next().unwrap_or("");
        if let Some(h) = first.strip_prefix("# ") {
            out.push(Section {
                heading: h.trim().to_string(),
                start: *ord,
                end: *ord,
                text: text.clone(),
            });
        } else if let Some(cur) = out.last_mut() {
            cur.end = *ord;
            if cur.text.len() < SECTION_HEAD_CHARS {
                cur.text.push_str("\n\n");
                cur.text.push_str(text);
            }
        }
    }
    out
}

fn section_stamp(sections: &[Section]) -> i32 {
    let key: String = sections
        .iter()
        .map(|s| format!("{}:{}-{}", s.heading, s.start, s.end))
        .collect::<Vec<_>>()
        .join("\n");
    content_hash(&key)
}

fn build_section_messages(title: &str, heading: &str, text: &str) -> Vec<ChatTurn> {
    let head: String = text.chars().take(SECTION_HEAD_CHARS).collect();
    vec![
        ChatTurn::system(
            "You write the index entry for one section of a document, for a reader who \
             will search for it by whatever words they know. Reply with exactly two \
             lines. Line 1: one or two plain sentences — what thing the section is about, \
             then what it covers about it (the specific figures, procedures, and parts it \
             gives). Line 2: \"Also called:\" followed by up to five other names a reader \
             might type for that thing — everyday words, abbreviations, the common name \
             for a technical one, the technical name for a common one, and for a city \
             its country. Use only what the section says or what the name plainly means. \
             Nothing else.",
        ),
        ChatTurn::user(format!(
            "Document: {title}\nSection: {heading}\n\n---\n{head}"
        )),
    ]
}

/// Accept a section summary: bounded, and not the model narrating itself.
fn section_gate(candidate: &str) -> Option<String> {
    let trimmed = candidate.trim().trim_matches('"').trim();
    let n = trimmed.chars().count();
    if !(SECTION_MIN_CHARS..=SECTION_MAX_CHARS).contains(&n) {
        return None;
    }
    let lower = trimmed.to_lowercase();
    if lower.starts_with("i ") || lower.starts_with("as an ai") || lower.contains("cannot") {
        return None;
    }
    Some(trimmed.to_string())
}

fn section_state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(SECTION_STATE_FILE)
}

fn load_section_state(dir: &Path) -> HashMap<String, i32> {
    std::fs::read_to_string(section_state_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_section_state(dir: &Path, state: &HashMap<String, i32>) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            section_state_path(dir),
            serde_json::to_vec(state).unwrap_or_default(),
        )
    };
    if let Err(err) = write() {
        crate::note!("sections: marker write failed: {err}");
    }
}

/// Write section summaries for long structured sources that lack them
/// (docs/RFC-outline-index.md Phase 2). One Small call per top-level
/// section, `SECTION_SWEEP_BUDGET` sources per pass; a source is stamped
/// once its rows land and reopened only when its sections change.
/// Returns how many sources were summarized this pass.
pub async fn ensure_section_gists(db: &Db, ai: &Ai) -> Result<usize> {
    let dir = ai.data_dir().to_path_buf();
    let mut state = load_section_state(&dir);
    let mut dirty = false;
    let mut written = 0usize;
    'outer: for nb in db.list_notebooks().await? {
        for s in db.list_sources(&nb.id).await? {
            if written >= SECTION_SWEEP_BUDGET {
                break 'outer;
            }
            if s.source_type == "code" || s.chunk_count < SECTION_MIN_SECTIONS as i64 {
                continue;
            }
            let rows = db.source_chunk_rows(&s.id).await?;
            let sections = sections_of(&rows);
            if sections.len() < SECTION_MIN_SECTIONS || sections.len() > SECTION_MAX_SECTIONS {
                continue;
            }
            let stamp = section_stamp(&sections);
            if state.get(&s.id) == Some(&stamp) {
                continue;
            }
            let mut gists: Vec<SectionGist> = Vec::with_capacity(sections.len());
            for sec in &sections {
                let messages = build_section_messages(&s.title, &sec.heading, &sec.text);
                let reply = match ai.chat_role(Role::Small, &messages).await {
                    Ok(out) => {
                        crate::freshness::record_outcome(&out);
                        out.text
                    }
                    Err(err) => {
                        crate::note!("sections: Small role failed for \"{}\": {err:#}", s.title);
                        #[cfg(test)]
                        eprintln!("  sections: Small role failed: {err:#}");
                        break 'outer;
                    }
                };
                #[cfg(test)]
                if std::env::var("ALCHEMY_TRACE_SECTIONS").is_ok() {
                    eprintln!(
                        "  [{}] gate {}: {}",
                        sec.heading,
                        if section_gate(&reply).is_some() {
                            "ok"
                        } else {
                            "REFUSED"
                        },
                        reply
                            .trim()
                            .replace('\n', " / ")
                            .chars()
                            .take(220)
                            .collect::<String>()
                    );
                }
                if let Some(summary) = section_gate(&reply) {
                    gists.push(SectionGist {
                        start: sec.start,
                        end: sec.end,
                        chain: format!("{} › {}", s.title, sec.heading),
                        summary,
                    });
                }
            }
            // Stamp even when the gate refused everything: the sections were
            // read, and only a change to them earns another pass.
            state.insert(s.id.clone(), stamp);
            dirty = true;
            if gists.is_empty() {
                continue;
            }
            let inputs: Vec<String> = gists
                .iter()
                .map(|g| format!("[{} — section]\n{}", g.chain, g.summary))
                .collect();
            let embeddings = ai.embed(&inputs).await?;
            db.replace_section_rows(&nb.id, &s.id, &gists, &embeddings)
                .await?;
            written += 1;
            crate::note!("sections: {} summaries for \"{}\"", gists.len(), s.title);
        }
    }
    if dirty {
        save_section_state(&dir, &state);
    }
    Ok(written)
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
        crate::note!("enrich: marker write failed: {err}");
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
        image_url: String::new(),
        author: String::new(),
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
            Ok(out) => {
                crate::freshness::record_outcome(&out);
                out.text
            }
            Err(err) => {
                crate::note!(
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
    let contexts: Vec<String> = chunks.iter().map(|c| c.context.clone()).collect();
    db.reembed_source_chunks(
        &source.notebook_id,
        &source.id,
        &rows,
        &contexts,
        &embeddings,
    )
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

    // One projected batch read of the candidates' content — hashing used to
    // full-scan per candidate, including every already-enriched one, on
    // every pass of every sweep.
    let contents = db.source_contents(&candidates).await?;
    for source_id in candidates {
        let hash = match contents.get(&source_id) {
            Some(c) => content_hash(c),
            None => continue,
        };
        if state.get(&source_id) == Some(&hash) || enrich_refused(&source_id, hash) {
            continue;
        }
        let source = match db.get_source(&source_id).await? {
            Some(s) => s,
            None => continue,
        };
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
    let ai = ai.background();
    if SWEEPING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // Consolidation first (RFC-living-notebook phase 5): wiki indexes
        // track the shelf on every sweep, before any model work runs —
        // deterministic, and a no-change pass costs reads only.
        match crate::growth::refresh_wiki_indexes(&db).await {
            Ok(n) if n > 0 => crate::note!("sweep: refreshed {n} wiki index(es)"),
            Ok(_) => {}
            Err(err) => crate::note!("sweep: wiki index refresh failed: {err:#}"),
        }
        // Web-enabled notebooks get their standing queries warmed too —
        // day-cached, so at most one metered search per notebook per day.
        match crate::growth::sweep_web_searches(&db).await {
            Ok(n) if n > 0 => crate::note!("sweep: warmed web proposals for {n} notebook(s)"),
            Ok(_) => {}
            Err(err) => crate::note!("sweep: web warm failed: {err:#}"),
        }
        for _ in 0..MAX_SWEEP_BATCHES {
            match ensure_gists(&db, &ai).await {
                // Gists converged; spend the batch on chunk enrichment (RFC §2
                // "gists first, chunks only when idle"). Enrichment ends the
                // sweep only when it, too, has nothing left to do.
                // Gists converged. Tag whatever is still untagged, then spend
                // what's left on chunk enrichment. Tags come first: they are
                // one cheap call per source and they show up in the UI, where
                // enrichment only ever shows up in retrieval quality.
                Ok((0, 0)) => match ensure_section_gists(&db, &ai).await {
                    // Section summaries ride right behind gists: same
                    // budget shape, and the outline they index is what
                    // long documents are retrieved by.
                    Ok(n) if n > 0 => {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                    Err(err) => {
                        crate::diagnostics::error(
                            "sweep",
                            format!("section sweep failed: {err:#}"),
                        );
                        break;
                    }
                    Ok(_) => match ensure_tags(&db, &ai).await {
                        Ok(n) if n > 0 => {
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        }
                        // Tags converged. Offer registry cards once per notebook
                        // per run (commands::registry::suggest_cards) — it reads
                        // the gists this sweep just settled, so it comes after
                        // them and before enrichment, same reasoning as tags:
                        // one cheap call, and it shows up in the UI.
                        Ok(_) => match crate::commands::suggest_cards(&db, &ai).await {
                            Ok(n) if n > 0 => {
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            }
                            // Chunk enrichment is the one stage that never
                            // converges quickly — one small-model call per
                            // chunk across the corpus — so it runs in quiet
                            // hours only. The day's sweeps stop here, converged
                            // on everything a person can see.
                            Ok(_) if !crate::freshness::is_quiet_hours(crate::commands::now()) => {
                                break
                            }
                            Ok(_) => match ensure_enrichment(&db, &ai).await {
                                Ok(0) => break,
                                Ok(_) => {
                                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                }
                                Err(err) => {
                                    crate::diagnostics::error(
                                        "sweep",
                                        format!("enrichment sweep failed: {err:#}"),
                                    );
                                    break;
                                }
                            },
                            Err(err) => {
                                crate::diagnostics::error(
                                    "sweep",
                                    format!("card suggestion failed: {err:#}"),
                                );
                                break;
                            }
                        },
                        Err(err) => {
                            crate::diagnostics::error(
                                "sweep",
                                format!("tag sweep failed: {err:#}"),
                            );
                            break;
                        }
                    },
                },
                Ok(_) => {
                    // Yield between batches so imports and chat stay snappy.
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                Err(err) => {
                    crate::diagnostics::error("sweep", format!("gist sweep failed: {err:#}"));
                    break;
                }
            }
        }
        SWEEPING.store(false, Ordering::SeqCst);
    });
}

/// Tags proposed per source. Few enough to stay a glance, not a cloud.
const MAX_AUTO_TAGS: usize = 4;
/// How much of the source the tagger reads.
const TAG_PROMPT_HEAD_CHARS: usize = 2000;
/// Sources tagged per sweep batch, matching the gist budget's restraint.
const TAG_BUDGET: usize = 8;

/// Suggest tags for sources that have none (docs/RFC-source-tags.md deferred
/// this to v2; this is it).
///
/// Only ever *adds* tags to an untagged source. A source the user has tagged
/// is theirs — we never append to it, never reorder it, never re-tag it when
/// the content changes. That is the whole confirmation story: the cost of a
/// wrong tag is one click to fix, and it can only ever be wrong on a source
/// nobody has curated yet.
///
/// Returns how many sources were tagged.
pub async fn ensure_tags(db: &Db, ai: &Ai) -> Result<usize> {
    let mut tagged = 0usize;
    for nb in db.list_notebooks().await? {
        // The untagged batch's content in one projected read — a converged
        // notebook costs nothing, and an untagged one no longer pays a full
        // table scan per source before its model call.
        let untagged: Vec<crate::models::Source> = db
            .list_sources(&nb.id)
            .await?
            .into_iter()
            .filter(|s| s.tags.trim().is_empty())
            .take(TAG_BUDGET.saturating_sub(tagged))
            .collect();
        if untagged.is_empty() {
            continue;
        }
        let ids: Vec<String> = untagged.iter().map(|s| s.id.clone()).collect();
        let contents = db.source_contents(&ids).await?;
        for source in untagged {
            if tagged >= TAG_BUDGET {
                return Ok(tagged);
            }
            let Some(content) = contents.get(&source.id) else {
                continue;
            };
            if content.trim().is_empty() {
                continue;
            }
            let messages = build_tag_messages(&source.title, content);
            let reply = match ai.chat_role(Role::Small, &messages).await {
                Ok(out) => out.text,
                Err(err) => {
                    crate::note!("tags: generation failed for \"{}\": {err:#}", source.title);
                    return Ok(tagged);
                }
            };
            let haystack = format!("{}\n{content}", source.title);
            let tags = gate_tags(&reply, &haystack);
            if tags.is_empty() {
                continue;
            }
            db.set_source_tags(&source.id, &tags).await?;
            tagged += 1;
        }
    }
    Ok(tagged)
}

fn build_tag_messages(title: &str, text: &str) -> Vec<ChatTurn> {
    let head: String = text.chars().take(TAG_PROMPT_HEAD_CHARS).collect();
    vec![
        ChatTurn::system(
            "You tag documents for a personal research library. Reply with ONLY the \
             tags: lowercase, single words, separated by spaces. No #, no commas, \
             no explanation.",
        ),
        ChatTurn::user(format!(
            "Give two to four single-word tags for this document titled \"{title}\". \
             Use words that appear in the document. Prefer the subject matter over \
             the format.\n\nDocument:\n---\n{head}"
        )),
    ]
}

/// Keep only tags the document can vouch for — the same bargain `gate` makes
/// for gists. A Small model asked for keywords will happily invent a plausible
/// one, and an invented tag is worse than no tag: it is a filter that lies.
fn gate_tags(reply: &str, haystack: &str) -> String {
    let hay = haystack.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for raw in reply.split(|c: char| c.is_whitespace() || c == ',') {
        let tag = raw
            .trim()
            .trim_start_matches('#')
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
            .to_lowercase();
        // Single words only; "machine-learning" is fine, a sentence is not.
        if tag.len() < 3 || tag.chars().count() > 24 {
            continue;
        }
        if !tag.chars().all(|c| c.is_alphanumeric() || c == '-') {
            continue;
        }
        if !hay.contains(&tag) {
            continue;
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
        if out.len() >= MAX_AUTO_TAGS {
            break;
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    /// Sections are the `# ` headings in ordinal order; each runs to the
    /// next one, and the section text is its chunks joined (capped).
    #[test]
    fn sections_follow_top_level_headings() {
        let rows = vec![
            (
                "c0".to_string(),
                0,
                "Preamble before any heading.".to_string(),
            ),
            (
                "c1".to_string(),
                1,
                "# Chapter 1: Hydraulics\n\nabout".to_string(),
            ),
            (
                "c2".to_string(),
                2,
                "## Torque Values\n\ntorque".to_string(),
            ),
            (
                "c3".to_string(),
                3,
                "# Chapter 2: Landing Gear\n\nabout".to_string(),
            ),
            ("c4".to_string(), 4, "## Parts\n\nFM-1210".to_string()),
        ];
        let secs = super::sections_of(&rows);
        let shape: Vec<(&str, i32, i32)> = secs
            .iter()
            .map(|s| (s.heading.as_str(), s.start, s.end))
            .collect();
        assert_eq!(
            shape,
            [
                ("Chapter 1: Hydraulics", 1, 2),
                ("Chapter 2: Landing Gear", 3, 4)
            ]
        );
        assert!(
            secs[1].text.contains("FM-1210"),
            "section text joins its chunks"
        );
        // Reordered rows land the same outline.
        let mut shuffled = rows.clone();
        shuffled.reverse();
        assert_eq!(super::sections_of(&shuffled).len(), 2);
        // Stamp follows headings and spans only.
        assert_eq!(
            super::section_stamp(&secs),
            super::section_stamp(&super::sections_of(&shuffled))
        );
    }

    /// The gate keeps a bounded plain summary and drops the model talking
    /// about itself.
    #[test]
    fn section_gate_bounds_and_refuses_narration() {
        let ok = "Landing gear: the undercarriage that carries the aircraft on the ground. Also called: undercarriage, wheels.";
        assert_eq!(super::section_gate(ok).as_deref(), Some(ok));
        assert!(super::section_gate("Too short.").is_none());
        assert!(super::section_gate("I cannot summarize this section for you.").is_none());
        let long = "x".repeat(super::SECTION_MAX_CHARS + 1);
        assert!(super::section_gate(&long).is_none());
    }

    use super::*;

    /// The gate is the whole safety story: a tag the document cannot vouch
    /// for is a filter that lies, so it never lands.
    #[test]
    fn gate_tags_keeps_only_grounded_single_words() {
        let hay = "Retrieval-augmented generation pairs a parametric model with \
                   a dense vector index over a corpus.";
        assert_eq!(
            super::gate_tags("retrieval vector corpus", hay),
            "retrieval vector corpus"
        );
        // Invented terms are dropped, grounded ones survive alongside.
        assert_eq!(super::gate_tags("retrieval kubernetes", hay), "retrieval");
        // Decoration and separators are tolerated on the way in.
        assert_eq!(super::gate_tags("#vector, #corpus", hay), "vector corpus");
        // Junk shapes: too short, too long, not a word.
        assert_eq!(super::gate_tags("a ---- the", hay), "");
        // Capped, and deduped.
        assert_eq!(
            super::gate_tags("retrieval retrieval vector corpus dense model", hay),
            "retrieval vector corpus dense"
        );
    }

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
