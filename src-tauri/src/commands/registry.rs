//! The Registry (docs/RFC-registry.md): a closed, user-confirmed cast of
//! things — assets, people, policies, providers, projects, dependencies —
//! and the documents that follow them.
//!
//! Note the name collision: "registry" elsewhere in this codebase means
//! `rag::ARTIFACT_KINDS`, the generator registry. This module is the cast.
//!
//! The one load-bearing rule is graded literal matching. A card's
//! *identifiers* are distinctive strings — a VIN, a policy number, a serial
//! — so a document containing one is that card's document, and it attaches
//! without asking. A card's *name* is ordinary language, so a document
//! containing it is a candidate and nothing more: it proposes. Nothing here
//! infers, embeds, or calls a model; a wrong attachment files a document
//! under the wrong thing and then answers questions from it, and no
//! similarity score is worth that.

use super::*;
use crate::models::{CardAttachment, CardFact, RegistryCard};

pub(crate) const REGISTRY_KINDS: &[&str] = &[
    "asset",
    "person",
    "policy",
    "provider",
    "project",
    "dependency",
];

/// The statuses an attachment may hold. `rejected` is terminal and kept —
/// it is the refusal memory that stops the sweep re-proposing a pair the
/// user already turned down.
pub(crate) const ATTACHMENT_STATUSES: &[&str] = &["confirmed", "proposed", "rejected"];

/// An identifier must be this distinctive before it may attach a document
/// on its own: long enough not to collide, and carrying a digit so an
/// ordinary word can never be one. "ab12" attaches nothing, ever.
const MIN_IDENTIFIER_LEN: usize = 6;
/// Below this a name is too short to be a safe proposal signal.
const MIN_NAME_LEN: usize = 4;
/// A card stops accruing proposals past this many pending — a queue nobody
/// can face is the same as no queue.
const MAX_PENDING_PER_CARD: usize = 12;
/// The prefix of a document weighed on arrival. Identifiers and names live
/// near the top of a document (headers, title blocks); scanning a whole
/// book to find a VIN in the appendix is not worth the read.
const SCAN_CAP: usize = 200_000;

pub(crate) fn validate_registry_kind(kind: &str) -> Result<(), String> {
    if REGISTRY_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(format!(
            "Unknown card kind \u{201c}{kind}\u{201d} — one of: {}",
            REGISTRY_KINDS.join(", ")
        ))
    }
}

pub(crate) fn validate_attachment_status(status: &str) -> Result<(), String> {
    if ATTACHMENT_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(format!(
            "Unknown attachment status \u{201c}{status}\u{201d} — one of: {}",
            ATTACHMENT_STATUSES.join(", ")
        ))
    }
}

/// The identifiers of `card` that are distinctive enough to auto-attach.
/// Stored identifiers are already `normalize_tags`-normalized (lowercase,
/// whitespace-separated), so this is a filter, not a parse.
fn attaching_identifiers(identifiers: &str) -> Vec<&str> {
    identifiers
        .split_whitespace()
        .filter(|t| {
            t.chars().count() >= MIN_IDENTIFIER_LEN && t.chars().any(|c| c.is_ascii_digit())
        })
        .collect()
}

/// Why this document belongs to this card, or `None`.
///
/// Returns the receipt that will be stored verbatim: the matched identifier
/// (auto-attach), or "name" (propose only). The haystack must already be
/// lowercased — the caller lowercases once per document, not once per card.
fn match_card(card: &RegistryCard, haystack: &str) -> Option<(String, &'static str)> {
    for id in attaching_identifiers(&card.identifiers) {
        if haystack.contains(id) {
            return Some((id.to_string(), "confirmed"));
        }
    }
    let name = card.name.trim().to_lowercase();
    if name.chars().count() >= MIN_NAME_LEN && haystack.contains(&name) {
        return Some(("name".into(), "proposed"));
    }
    None
}

/// File `source_id` under every card that claims it.
///
/// Silent and best-effort by contract: this rides source arrival, and a
/// registry miss must never fail an import. Cards that already know this
/// source — in any status, including `rejected` — are skipped, so the sweep
/// is idempotent and a turned-down proposal stays down.
pub(crate) async fn match_source_to_cards(
    db: &crate::db::Db,
    notebook_id: &str,
    source_id: &str,
    text: &str,
) -> usize {
    let Ok(cards) = db.list_registry().await else {
        return 0;
    };
    if cards.is_empty() {
        return 0;
    }
    let head: String = text.chars().take(SCAN_CAP).collect();
    let haystack = head.to_lowercase();
    let ts = now();
    let mut filed = 0;
    for mut card in cards {
        if card.attachments.iter().any(|a| a.source_id == source_id) {
            continue;
        }
        let Some((matched, status)) = match_card(&card, &haystack) else {
            continue;
        };
        if status == "proposed"
            && card
                .attachments
                .iter()
                .filter(|a| a.status == "proposed")
                .count()
                >= MAX_PENDING_PER_CARD
        {
            continue;
        }
        card.attachments.push(CardAttachment {
            source_id: source_id.to_string(),
            notebook_id: notebook_id.to_string(),
            status: status.to_string(),
            matched,
            at: ts,
        });
        card.updated_at = ts;
        if db.update_registry_card(&card).await.is_ok() {
            filed += 1;
        }
    }
    if filed > 0 {
        // The staff filed something while you were looking at it.
        super::notify_changed("registry", None);
    }
    filed
}

/// Fire-and-forget matching on source arrival. Takes owned handles (the
/// `gist::spawn_sweep` shape) so callers without a Tauri handle — reingest
/// — can spawn it. Nothing is owed: a failure is silent and the next change
/// retries.
pub(crate) fn spawn_registry_match(
    db: std::sync::Arc<crate::db::Db>,
    notebook_id: String,
    source_id: String,
    text: String,
) {
    tauri::async_runtime::spawn(async move {
        match_source_to_cards(&db, &notebook_id, &source_id, &text).await;
    });
}

// ---- The suggester -------------------------------------------------------
//
// An empty registry is an opt-in gate, and this app doesn't ship those: the
// intelligent behavior is on and the toggle is cost control. So the cast
// populates itself — but it *proposes*, it never mints. A suggested card is
// `origin: "auto"` and does nothing until you confirm it; a dismissed one is
// kept as `origin: "dismissed"` so the same guess never comes back. The
// authority stays exactly where the RFC put it: with you.
//
// This is the `ensure_tags` bargain (gist.rs) at card scale — auto-fill what
// is empty, never touch what a human has curated.

/// Notebooks already offered suggestions this app run. App-run lifetime is
/// deliberate (the gallery's backfill sweep uses the same scope): the pass
/// converges immediately so the gist sweep's loop can terminate, and a new
/// launch reconsiders a corpus that has since grown.
static SUGGESTED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

/// Cards proposed per notebook. A first look at a notebook should read as a
/// short list you can rule on, not an inbox — but low enough that the model
/// starts triaging for you is worse than a couple of extra rows to dismiss.
const MAX_SUGGESTIONS: usize = 8;
/// Gists concatenated into the one prompt — a wide view of the notebook for
/// a single Small call, instead of one call per source.
const MAX_GIST_ROWS: usize = 40;
const GIST_EXCERPT_CHARS: usize = 300;
/// A notebook this small hasn't told us what it's about yet.
const MIN_GISTS_TO_SUGGEST: usize = 3;

/// Propose cards for notebooks that haven't been looked at this run.
///
/// One `Small` call per notebook over its existing gists — no per-source
/// fan-out. Every proposal is gated the way `gate_tags` gates a tag: the
/// name must appear verbatim in the material, the kind must be in the
/// vocabulary, and anything already in the cast (in any origin, including
/// dismissed) is skipped. Returns how many cards were proposed.
pub async fn suggest_cards(db: &crate::db::Db, ai: &crate::ai::Ai) -> anyhow::Result<usize> {
    let gists = db.list_gists().await?;
    if gists.is_empty() {
        return Ok(0);
    }
    let mut proposed = 0usize;

    for nb in db.list_notebooks().await? {
        {
            let mut guard = SUGGESTED.lock().unwrap();
            let seen = guard.get_or_insert_with(Default::default);
            if !seen.insert(nb.id.clone()) {
                continue;
            }
        }
        // Fetched per notebook, not once for the sweep: a card proposed for
        // the previous notebook must block the same name here, or every
        // notebook that shares a dependency mints its own copy of it.
        let existing = db.list_registry().await.unwrap_or_default();
        proposed += suggest_for_notebook(db, ai, &nb.id, &gists, &existing, None)
            .await
            .unwrap_or_default()
            .len();
    }
    if proposed > 0 {
        super::notify_changed("registry", None);
    }
    // Triage whatever has queued up — this run's proposals and any left
    // over from earlier ones. One batched Small call; when the queue is
    // short enough to rule on by hand it costs nothing at all.
    if let Err(err) = triage_suggested_cards(db, ai).await {
        eprintln!("registry triage failed: {err:#}");
    }
    Ok(proposed)
}

// ---- Triage ---------------------------------------------------------------
//
// A busy corpus can queue more suggestions than anyone wants to rule on
// one by one. The triage pass reads the whole queue in ONE Small call —
// each candidate with how often it recurs and a snippet of the material
// that mentions it — and marks the ones worth keeping as `triage:
// "recommended"`. It marks; it never rules. "Keep recommended" is still a
// human (or agent) click, so the closed-cast rule stands untouched.

/// Pending, untriaged suggestions it takes before triage spends a model
/// call. Below this a person rules on the strip faster than a model can.
const MIN_QUEUE_TO_TRIAGE: usize = 4;
/// Suggestions weighed per pass — one batched call, never one per card.
const MAX_TRIAGE_BATCH: usize = 40;
/// Characters of context shown on each side of a candidate's first mention.
const TRIAGE_SNIPPET_RADIUS: usize = 70;

/// Mark the suggested cards worth recommending. Returns how many cards got
/// a verdict (recommended or routine); 0 when the queue is too short.
pub(crate) async fn triage_suggested_cards(
    db: &crate::db::Db,
    ai: &crate::ai::Ai,
) -> anyhow::Result<usize> {
    use crate::inference::Role;
    let cards = db.list_registry().await?;
    let queue: Vec<RegistryCard> = cards
        .into_iter()
        .filter(|c| c.origin == "auto" && c.triage.is_empty())
        .take(MAX_TRIAGE_BATCH)
        .collect();
    if queue.len() < MIN_QUEUE_TO_TRIAGE {
        return Ok(0);
    }
    // Frequency across the corpus's gists is the recurrence signal; the
    // snippet is the source context. Lowercase once, not once per card.
    let gists = db.list_gists().await.unwrap_or_default();
    let lowered: Vec<String> = gists.iter().map(|g| g.text.to_lowercase()).collect();
    let mut lines = String::new();
    for (i, c) in queue.iter().enumerate() {
        let name_lc = c.name.to_lowercase();
        let mentions = lowered.iter().filter(|t| t.contains(&name_lc)).count();
        let snippet = lowered
            .iter()
            .find_map(|t| snippet_around(t, &name_lc, TRIAGE_SNIPPET_RADIUS))
            .unwrap_or_default();
        lines.push_str(&format!(
            "{}. {}|{} — in {} document{}{}\n",
            i + 1,
            c.kind,
            c.name,
            mentions,
            if mentions == 1 { "" } else { "s" },
            if snippet.is_empty() {
                String::new()
            } else {
                format!("; \u{201c}\u{2026}{snippet}\u{2026}\u{201d}")
            },
        ));
    }
    let reply = ai
        .chat_role(Role::Small, &build_triage_messages(&lines))
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?
        .text;
    let picked = parse_triage_reply(&reply, queue.len());
    let ts = now();
    let mut marked = 0usize;
    for (i, mut card) in queue.into_iter().enumerate() {
        card.triage = if picked.contains(&(i + 1)) {
            "recommended"
        } else {
            "routine"
        }
        .into();
        card.updated_at = ts;
        if db.update_registry_card(&card).await.is_ok() {
            marked += 1;
        }
    }
    if marked > 0 {
        super::notify_changed("registry", None);
    }
    Ok(marked)
}

/// A snippet of `text` around the first occurrence of `needle`, or `None`.
/// `text` and `needle` are already lowercased; slicing stays on the byte
/// boundary `find` returns, then counts chars so multibyte text can't panic.
fn snippet_around(text: &str, needle: &str, radius: usize) -> Option<String> {
    let at = text.find(needle)?;
    let prefix_chars = text[..at].chars().count();
    let begin = prefix_chars.saturating_sub(radius);
    let take = radius * 2 + needle.chars().count();
    let s: String = text.chars().skip(begin).take(take).collect();
    Some(s.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn build_triage_messages(candidates: &str) -> Vec<crate::ai::ChatTurn> {
    vec![
        crate::ai::ChatTurn {
            role: "system".into(),
            content: "You triage a queue of suggested registry cards — things that turned \
                 up across a person's documents, each a candidate to track over time. \
                 Mark the ones worth recommending.\n\n\
                 Recommend a candidate when the person will keep accumulating paperwork \
                 about it: something they own, insure, pay, or work on — especially when \
                 it recurs across documents or is central to the ones that mention it. \
                 Skip anything mentioned only in passing: a company named as context, a \
                 person merely quoted, a place passed through.\n\n\
                 Reply with only the numbers of the recommended candidates, \
                 comma-separated (for example: 1, 3, 4). If none stand out, reply: none."
                .into(),
        },
        crate::ai::ChatTurn {
            role: "user".into(),
            content: candidates.to_string(),
        },
    ]
}

/// The candidate numbers a triage reply recommends, bounded to 1..=n.
/// Forgiving like `gate_suggestions`: bullets, restated lines, and prose
/// around the numbers all still parse; "none" (however decorated) is empty.
fn parse_triage_reply(reply: &str, n: usize) -> std::collections::HashSet<usize> {
    let mut out = std::collections::HashSet::new();
    if reply.trim().to_lowercase().starts_with("none") {
        return out;
    }
    let mut cur = 0usize;
    let mut in_num = false;
    for ch in reply.chars() {
        if let Some(d) = ch.to_digit(10) {
            cur = cur.saturating_mul(10) + d as usize;
            in_num = true;
        } else {
            if in_num && (1..=n).contains(&cur) {
                out.insert(cur);
            }
            cur = 0;
            in_num = false;
        }
    }
    if in_num && (1..=n).contains(&cur) {
        out.insert(cur);
    }
    out
}

/// Propose cards for one notebook. Split out of the sweep so it can also be
/// asked for directly — waiting for a background pass is a poor way to find
/// out whether the suggester works, for a user as much as for a developer.
async fn suggest_for_notebook(
    db: &crate::db::Db,
    ai: &crate::ai::Ai,
    notebook_id: &str,
    gists: &[crate::db::GistRow],
    existing: &[RegistryCard],
    mut echo: Option<&mut String>,
) -> anyhow::Result<Vec<String>> {
    use crate::inference::Role;
    let mut made: Vec<String> = Vec::new();
    {
        let sources = db.list_sources(notebook_id).await.unwrap_or_default();
        let ids: std::collections::HashSet<&str> = sources.iter().map(|s| s.id.as_str()).collect();
        let mut material = String::new();
        let mut rows = 0usize;
        for g in gists.iter().filter(|g| ids.contains(g.source_id.as_str())) {
            if rows >= MAX_GIST_ROWS {
                break;
            }
            let title = sources
                .iter()
                .find(|s| s.id == g.source_id)
                .map(|s| s.title.as_str())
                .unwrap_or("");
            let excerpt: String = g.text.chars().take(GIST_EXCERPT_CHARS).collect();
            material.push_str(&format!("- {title}: {excerpt}\n"));
            rows += 1;
        }
        // Below this there isn't enough of a notebook to say what it's
        // about. Fall back to titles and heads when gists haven't landed —
        // an explicit ask shouldn't be blocked on the distillation sweep.
        if rows < MIN_GISTS_TO_SUGGEST {
            material.clear();
            for s in sources.iter().take(MAX_GIST_ROWS) {
                let Ok(full) = db.source_content(&s.id).await else {
                    continue;
                };
                let head: String = full.chars().take(GIST_EXCERPT_CHARS).collect();
                material.push_str(&format!("- {}: {head}\n", s.title));
            }
            if material.trim().is_empty() {
                return Ok(made);
            }
        }
        let reply = ai
            .chat_role(Role::Small, &build_suggest_messages(&material))
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))?
            .text;
        if let Some(out) = &mut echo {
            out.push_str(&reply);
        }
        let haystack = material.to_lowercase();
        let gated = gate_suggestions(&reply, &haystack);
        if gated.is_empty() {
            eprintln!(
                "registry: nothing survived the gate; model said: {}",
                reply
                    .replace('\n', " / ")
                    .chars()
                    .take(400)
                    .collect::<String>()
            );
        }
        for (kind, name, facts) in gated {
            // Anything already in the cast — yours, pending, or turned down
            // — is settled. Never re-propose it. `made` guards within this
            // reply: a model can restate one thing twice in one answer.
            if existing.iter().any(|c| same_thing(&c.name, &name))
                || made.iter().any(|m| same_thing(m, &name))
            {
                continue;
            }
            let ts = now();
            let card = RegistryCard {
                id: new_id(),
                kind,
                name: name.clone(),
                origin: "auto".into(),
                triage: String::new(),
                identifiers: String::new(),
                note: String::new(),
                facts,
                attachments: Vec::new(),
                created_at: ts,
                updated_at: ts,
            };
            if db.add_registry_card(&card).await.is_ok() {
                made.push(name);
            }
        }
    }
    Ok(made)
}

/// Ask for suggestions on one notebook, now — the Registry's "Suggest cards"
/// action. Ignores the once-per-run marker: an explicit ask is not the
/// background pass, and a user who clicks twice means it.
#[tauri::command]
pub async fn suggest_cards_now(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<SuggestOutcome, String> {
    let gists = e(state.db.list_gists().await)?;
    let existing = e(state.db.list_registry().await)?;
    let ai = state.ai.read().await.clone();
    let mut reply = String::new();
    let made = suggest_for_notebook(
        &state.db,
        &ai,
        &notebook_id,
        &gists,
        &existing,
        Some(&mut reply),
    )
    .await
    .map_err(|e| e.to_string())?;
    if !made.is_empty() {
        super::notify_changed("registry", None);
    }
    // Triage in the background — the click shouldn't wait on a second
    // model call, and the strip re-sorts itself on the registry bump.
    let db = state.db.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = triage_suggested_cards(&db, &ai).await {
            eprintln!("registry triage failed: {err:#}");
        }
    });
    Ok(SuggestOutcome {
        created: made,
        reply,
    })
}

/// What an explicit ask produced. `reply` carries the model's raw answer so
/// "it suggested nothing" can be told apart from "it said something I
/// couldn't parse" — by a user reading an error, and by whoever is
/// debugging the prompt.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestOutcome {
    pub created: Vec<String>,
    pub reply: String,
}

fn build_suggest_messages(material: &str) -> Vec<crate::ai::ChatTurn> {
    vec![
        crate::ai::ChatTurn {
            role: "system".into(),
            content: format!(
                "You name the recurring THINGS a person's documents are about, so they can be \
                 tracked over time. Reply with at most {MAX_SUGGESTIONS} lines and nothing else. \
                 Each line is a kind, a pipe, then the name — exactly two \
                 fields, like:\n\
                 asset|Ducati Monster\n\
                 provider|Corley Automotive\n\n\
                 The kind must be one of: {}\n\
                 - asset: a physical thing owned (a vehicle, appliance, instrument, property)\n\
                 - person: a named individual\n\
                 - policy: an insurance policy, warranty, or service contract\n\
                 - provider: a company, practitioner, or service\n\
                 - project: a piece of work with a beginning and an end\n\
                 - dependency: a library, framework, or service relied on\n\n\
                 Name it exactly as the documents name it, copied verbatim. Prefer specific \
                 proper names over categories: \"Ducati Monster\" not \"motorcycle\".\n\n\
                 The test is NOT how often it is mentioned. It is whether this is something \
                 the person will keep accumulating paperwork about. Include it even when it \
                 appears only once:\n\
                 - every insurance policy, warranty, or service contract, named as the \
                 documents name it\n\
                 - every company or practitioner they pay, hire, moor at, or buy from\n\
                 - every substantial thing they own\n\n\
                 Name it the way a person would say it out loud, not as a bare reference \
                 number: \"Ashfield Mutual studio rider\", not \"AM-88214\". A number may \
                 follow the name, never replace it.\n\n\
                 After the name you may add up to {MAX_FACTS_PER_CARD} key facts, each as \
                 one more pipe field written \"Label: value\" — a policy number, a serial, \
                 a renewal date, a model:\n\
                 asset|Ducati Monster|VIN: ZDM1RAZ4XWB012345|Year: 2019\n\
                 Only facts whose value is copied word-for-word from the material; \
                 anything paraphrased or guessed is discarded. No facts is fine.\n\n\
                 Leave out anything that is not theirs to track: a company named only as \
                 context or comparison, a person merely quoted, a place merely passed \
                 through. If there is genuinely nothing, reply with nothing at all.",
                REGISTRY_KINDS.join(", ")
            ),
        },
        crate::ai::ChatTurn {
            role: "user".into(),
            content: material.to_string(),
        },
    ]
}

/// One word, spelled the way it would be spelled out loud: lowercased,
/// punctuation dropped, and the everyday address abbreviations expanded —
/// "Rd" is "Road", "St" is "Street". This is what lets "15217 Canyon Seven
/// Rd" and "15217 Canyon Seven Road" read as one thing instead of two.
/// Deliberately not fuzzy: digits must match exactly ("Timberline 2.0" and
/// "Timberline 3.0" stay two projects), and an unknown word is only itself.
fn canon_word(w: &str) -> String {
    let w: String = w
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    match w.as_str() {
        "rd" => "road",
        "st" => "street",
        "ave" | "av" => "avenue",
        "blvd" => "boulevard",
        "dr" => "drive",
        "ln" => "lane",
        "hwy" => "highway",
        "ct" => "court",
        "pkwy" => "parkway",
        "cir" => "circle",
        "pl" => "place",
        "sq" => "square",
        "ste" => "suite",
        "apt" => "apartment",
        "mt" => "mount",
        "ft" => "fort",
        "n" => "north",
        "s" => "south",
        "e" => "east",
        "w" => "west",
        _ => return w,
    }
    .to_string()
}

/// Whether a proposed name restates a card that already exists.
///
/// Exact match isn't enough: across runs the same model names one object
/// two ways — "Rheem Performance Platinum" and "Rheem Performance Platinum
/// water heater", "Paul Thrasher" and "Paul Scott Thrasher", "15217 Canyon
/// Seven Rd" and "…Road" — and two cards for one thing is precisely the
/// mess the Registry exists to prevent. Words are compared in `canon_word`
/// form; the shorter name's words, in order and whole, inside the longer
/// one is a restatement. Anything less leaves genuinely different things
/// alone ("Apple" does not swallow "Pineapple Studios"), and this only ever
/// suppresses a suggestion, never a card someone made.
fn same_thing(existing: &str, proposed: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(canon_word)
            .filter(|w| !w.is_empty())
            .collect()
    };
    let a = words(existing);
    let b = words(proposed);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if short.iter().map(|w| w.chars().count()).sum::<usize>() < MIN_NAME_LEN {
        return false;
    }
    // Whole words, in order — "Sea Otter" reads inside "Sea Otter II" and a
    // middle name never splits a person in two, but "Chen Maya" is not
    // "Maya Chen Portfolio".
    let mut long_words = long.iter();
    short.iter().all(|w| long_words.any(|lw| lw == w))
}

/// Facts allowed onto one suggested card. Auto-fill, not an essay: the
/// facts grid should arrive started, not finished.
const MAX_FACTS_PER_CARD: usize = 4;
const MAX_FACT_LABEL_LEN: usize = 24;
const MAX_FACT_VALUE_LEN: usize = 80;

/// Parse one "Label: value" field into a fact, or refuse it.
///
/// Gated exactly like a name: the value must appear verbatim in the
/// material. An invented fact on a card is a lie with a label — worse than
/// the empty grid, because it reads as checked.
fn gate_fact(part: &str, haystack: &str) -> Option<CardFact> {
    let (label, value) = part.split_once(':')?;
    let label = label.trim().trim_matches('"');
    let value = value.trim().trim_matches('"').trim();
    if label.is_empty() || label.chars().count() > MAX_FACT_LABEL_LEN {
        return None;
    }
    if value.is_empty() || value.chars().count() > MAX_FACT_VALUE_LEN {
        return None;
    }
    if !haystack.contains(&value.to_lowercase()) {
        return None;
    }
    Some(CardFact {
        label: label.to_string(),
        value: value.to_string(),
    })
}

/// Keep only proposals that are grounded and well-formed.
///
/// The load-bearing check is the verbatim one: the name must appear in the
/// material, and so must every fact value. An invented card is worse than
/// no card — it is a thing in your registry that was never in your life.
fn gate_suggestions(reply: &str, haystack: &str) -> Vec<(String, String, Vec<CardFact>)> {
    let mut out: Vec<(String, String, Vec<CardFact>)> = Vec::new();
    for line in reply.lines() {
        if out.len() >= MAX_SUGGESTIONS {
            break;
        }
        // Forgiving by design. A model handed "kind|name" as a template can
        // emit the literal word "kind" as a third field, wrap the line in
        // bullets or bold, or number it — none of which means it got the
        // task wrong. So: strip the decoration, find the field that IS a
        // kind, and treat what follows as the name.
        let line = line
            .trim()
            .trim_start_matches(['-', '*', '#', ' '])
            .trim_matches('*')
            .trim_matches('`')
            .trim();
        let line = match line.split_once(". ") {
            Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => line,
        };
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if parts.len() < 2 {
            continue;
        }
        let Some(at) = parts
            .iter()
            .position(|p| REGISTRY_KINDS.contains(&p.to_lowercase().as_str()))
        else {
            continue;
        };
        if at + 1 >= parts.len() {
            continue;
        }
        let kind = parts[at].to_lowercase();
        // The field after the kind is the name; fields after that are fact
        // attempts ("Label: value" — kept only when gated) or, colon-less,
        // strays a model split out of the name itself.
        let mut name_parts: Vec<&str> = vec![parts[at + 1]];
        let mut facts: Vec<CardFact> = Vec::new();
        for p in &parts[at + 2..] {
            if p.contains(':') {
                if facts.len() < MAX_FACTS_PER_CARD {
                    if let Some(f) = gate_fact(p, haystack) {
                        facts.push(f);
                    }
                }
            } else if facts.is_empty() {
                name_parts.push(p);
            }
        }
        let joined = name_parts.join(" ");
        let name = joined.trim().trim_matches('"').trim();
        let len = name.chars().count();
        if !(MIN_NAME_LEN..=60).contains(&len) {
            continue;
        }
        // A reference number is not a name. "AM-88214" tells you nothing at
        // a glance and sorts nowhere useful, so require at least one real
        // word — a run of three or more letters.
        if !name
            .split(|c: char| !c.is_alphabetic())
            .any(|w| w.chars().count() >= 3)
        {
            continue;
        }
        if !haystack.contains(&name.to_lowercase()) {
            continue;
        }
        // Near-duplicates within one reply merge into the first ("15217
        // Canyon Seven Rd" and "…Road" are one address, not two cards).
        if out.iter().any(|(_, n, _)| same_thing(n, name)) {
            continue;
        }
        out.push((kind, name.to_string(), facts));
    }
    out
}

#[tauri::command]
pub async fn list_registry(state: State<'_, AppState>) -> Result<Vec<RegistryCard>, String> {
    e(state.db.list_registry().await)
}

/// Rule on a suggested card: "" confirms it (it becomes yours, and its
/// notebook is re-matched so it picks up its documents), "dismissed" turns
/// it down and is remembered forever.
#[tauri::command]
pub async fn set_card_origin(
    state: State<'_, AppState>,
    id: String,
    origin: String,
) -> Result<RegistryCard, String> {
    if !["", "auto", "dismissed"].contains(&origin.as_str()) {
        return Err(format!("Unknown card origin \u{201c}{origin}\u{201d}"));
    }
    let Some(mut card) = e(state.db.get_registry_card(&id).await)? else {
        return Err("Card not found".into());
    };
    let confirming = card.origin == "auto" && origin.is_empty();
    card.origin = origin;
    // The triage verdict is queue metadata; a ruled-on card carries none.
    card.triage = String::new();
    card.updated_at = now();
    e(state.db.update_registry_card(&card).await)?;
    // Keeping a suggestion is the moment it should acquire its documents —
    // it was proposed from gists and has no attachments yet. Corpus-wide and
    // backgrounded: a card spans notebooks, and reading every source is too
    // slow to make the click wait.
    if confirming {
        spawn_rematch_all(state.db.clone());
    }
    super::notify_changed("registry", None);
    Ok(card)
}

/// Rule on suggested cards in bulk — the strip's "Keep recommended" /
/// "Keep all" / "Dismiss all". Shared with the MCP tool. With
/// `only_recommended`, only the cards the triage pass marked are ruled; the
/// rest stay in the queue. One rematch sweep at the end instead of one per
/// card, so keeping a dozen suggestions doesn't read the corpus a dozen
/// times over.
pub(crate) async fn rule_all_suggested_cards(
    db: &std::sync::Arc<crate::db::Db>,
    origin: &str,
    only_recommended: bool,
) -> Result<usize, String> {
    let cards = db.list_registry().await.map_err(|e| e.to_string())?;
    let ts = now();
    let mut ruled = 0usize;
    for mut card in cards {
        if card.origin != "auto" {
            continue;
        }
        if only_recommended && card.triage != "recommended" {
            continue;
        }
        card.origin = origin.to_string();
        card.triage = String::new();
        card.updated_at = ts;
        db.update_registry_card(&card)
            .await
            .map_err(|e| e.to_string())?;
        ruled += 1;
    }
    if ruled > 0 && origin.is_empty() {
        spawn_rematch_all(db.clone());
    }
    Ok(ruled)
}

/// The Tauri face of `rule_all_suggested_cards`: "" keeps suggestions,
/// "dismissed" turns them down (remembered, like a one-by-one dismiss).
/// `only_recommended` limits either verdict to the triage pass's picks.
#[tauri::command]
pub async fn rule_all_suggested(
    state: State<'_, AppState>,
    origin: String,
    only_recommended: Option<bool>,
) -> Result<usize, String> {
    if !["", "dismissed"].contains(&origin.as_str()) {
        return Err(format!("Unknown card origin \u{201c}{origin}\u{201d}"));
    }
    let ruled =
        rule_all_suggested_cards(&state.db, &origin, only_recommended.unwrap_or(false)).await?;
    if ruled > 0 {
        super::notify_changed("registry", None);
    }
    Ok(ruled)
}

/// One corpus rematch at a time. A sweep reads every source's content — a
/// Lance scan apiece, and DataFusion fans each scan across cores — so
/// confirming five suggestions one-by-one used to stack five concurrent
/// corpus sweeps and pin every performance core for minutes. Requests that
/// arrive mid-sweep coalesce into one more pass.
static REMATCH_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static REMATCH_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Re-run matching over the whole corpus, in the background.
fn spawn_rematch_all(db: std::sync::Arc<crate::db::Db>) {
    use std::sync::atomic::Ordering::SeqCst;
    REMATCH_PENDING.store(true, SeqCst);
    if REMATCH_RUNNING.swap(true, SeqCst) {
        // The running sweep will see the pending flag and go around again.
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            while REMATCH_PENDING.swap(false, SeqCst) {
                rematch_all(&db).await;
            }
            REMATCH_RUNNING.store(false, SeqCst);
            // A request that landed between the pending check and the
            // running drop would otherwise be lost — reclaim and go again.
            if REMATCH_PENDING.load(SeqCst) && !REMATCH_RUNNING.swap(true, SeqCst) {
                continue;
            }
            break;
        }
    });
}

async fn rematch_all(db: &crate::db::Db) {
    let Ok(notebooks) = db.list_notebooks().await else {
        return;
    };
    for nb in notebooks {
        // One content scan per notebook — this was one full table scan per
        // source, corpus-wide, with a deliberate sleep between each.
        let Ok(sources) = db.sources_with_content(&nb.id).await else {
            continue;
        };
        for s in sources {
            if s.source_type == "folder" {
                continue;
            }
            match_source_to_cards(db, &nb.id, &s.id, &s.content).await;
        }
        // Breathe between notebooks so the sweep shares the machine with
        // the person using it.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tauri::command]
pub async fn get_registry_card(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<RegistryCard>, String> {
    e(state.db.get_registry_card(&id).await)
}

#[tauri::command]
pub async fn add_registry_card(
    state: State<'_, AppState>,
    kind: String,
    name: String,
    identifiers: Option<String>,
    note: Option<String>,
    facts: Option<Vec<CardFact>>,
) -> Result<RegistryCard, String> {
    validate_registry_kind(&kind)?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("A card is a thing with a name — give it one.".into());
    }
    let ts = now();
    let card = RegistryCard {
        id: new_id(),
        kind,
        name,
        origin: String::new(),
        triage: String::new(),
        identifiers: normalize_tags(&identifiers.unwrap_or_default()),
        note: note.unwrap_or_default().trim().to_string(),
        facts: facts.unwrap_or_default(),
        attachments: Vec::new(),
        created_at: ts,
        updated_at: ts,
    };
    e(state.db.add_registry_card(&card).await)?;
    super::notify_changed("registry", None);
    Ok(card)
}

#[tauri::command]
pub async fn update_registry_card(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    identifiers: Option<String>,
    note: Option<String>,
    facts: Option<Vec<CardFact>>,
) -> Result<RegistryCard, String> {
    let Some(mut card) = e(state.db.get_registry_card(&id).await)? else {
        return Err("Card not found".into());
    };
    if let Some(name) = name {
        let name = name.trim().to_string();
        if !name.is_empty() {
            card.name = name;
        }
    }
    if let Some(identifiers) = identifiers {
        card.identifiers = normalize_tags(&identifiers);
    }
    if let Some(note) = note {
        card.note = note.trim().to_string();
    }
    if let Some(facts) = facts {
        card.facts = facts;
    }
    card.updated_at = now();
    e(state.db.update_registry_card(&card).await)?;
    super::notify_changed("registry", None);
    Ok(card)
}

#[tauri::command]
pub async fn delete_registry_card(state: State<'_, AppState>, id: String) -> Result<(), String> {
    e(state.db.delete_registry_card(&id).await)?;
    super::notify_changed("registry", None);
    Ok(())
}

/// Attach a document by hand — the fastest path, and the one that seeds the
/// cast. Re-attaching a source the card already knows re-opens it at the
/// given status, which is how a `rejected` pair is undone.
#[tauri::command]
pub async fn attach_source_to_card(
    state: State<'_, AppState>,
    card_id: String,
    source_id: String,
    status: Option<String>,
) -> Result<RegistryCard, String> {
    let status = status.unwrap_or_else(|| "confirmed".into());
    validate_attachment_status(&status)?;
    let Some(mut card) = e(state.db.get_registry_card(&card_id).await)? else {
        return Err("Card not found".into());
    };
    let Some(source) = e(state.db.get_source(&source_id).await)? else {
        return Err("Source not found".into());
    };
    let ts = now();
    if let Some(a) = card
        .attachments
        .iter_mut()
        .find(|a| a.source_id == source_id)
    {
        a.status = status;
        a.matched = "manual".into();
        a.at = ts;
    } else {
        card.attachments.push(CardAttachment {
            source_id,
            notebook_id: source.notebook_id,
            status,
            matched: "manual".into(),
            at: ts,
        });
    }
    card.updated_at = ts;
    e(state.db.update_registry_card(&card).await)?;
    super::notify_changed("registry", None);
    Ok(card)
}

/// Resolve a proposal: confirm it, reject it (remembered forever), or drop
/// the row entirely.
#[tauri::command]
pub async fn set_attachment_status(
    state: State<'_, AppState>,
    card_id: String,
    source_id: String,
    status: String,
) -> Result<RegistryCard, String> {
    let Some(mut card) = e(state.db.get_registry_card(&card_id).await)? else {
        return Err("Card not found".into());
    };
    if status == "remove" {
        card.attachments.retain(|a| a.source_id != source_id);
    } else {
        validate_attachment_status(&status)?;
        let Some(a) = card
            .attachments
            .iter_mut()
            .find(|a| a.source_id == source_id)
        else {
            return Err("That document isn't filed under this card".into());
        };
        a.status = status;
        a.at = now();
    }
    card.updated_at = now();
    e(state.db.update_registry_card(&card).await)?;
    super::notify_changed("registry", None);
    Ok(card)
}

/// Every card holding `source_id` at a non-rejected status — the reader
/// rail's query, answered without loading the corpus.
#[tauri::command]
pub async fn cards_for_source(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<Vec<RegistryCard>, String> {
    let cards = e(state.db.list_registry().await)?;
    Ok(cards
        .into_iter()
        .filter(|c| {
            c.attachments
                .iter()
                .any(|a| a.source_id == source_id && a.status != "rejected")
        })
        .collect())
}

/// Re-run matching for one card across a notebook's sources — what you want
/// right after adding a VIN to a card that already had documents.
#[tauri::command]
pub async fn rematch_registry(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<usize, String> {
    // One content scan for the notebook instead of one per source.
    let sources = e(state.db.sources_with_content(&notebook_id).await)?;
    let mut filed = 0;
    for s in sources {
        if s.source_type == "folder" {
            continue;
        }
        filed += match_source_to_cards(&state.db, &notebook_id, &s.id, &s.content).await;
    }
    Ok(filed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, identifiers: &str) -> RegistryCard {
        RegistryCard {
            id: "c1".into(),
            kind: "asset".into(),
            name: name.into(),
            origin: String::new(),
            triage: String::new(),
            identifiers: identifiers.into(),
            note: String::new(),
            facts: vec![],
            attachments: vec![],
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn identifier_match_attaches_and_reports_itself() {
        let c = card("Ducati Monster", "zdm1raz4 wj9401ab2233");
        let (matched, status) = match_card(&c, "vin wj9401ab2233 registered 2019").unwrap();
        assert_eq!(status, "confirmed");
        assert_eq!(matched, "wj9401ab2233");
    }

    #[test]
    fn short_or_wordlike_identifiers_never_attach() {
        // Too short, and no digit — either disqualifies.
        let c = card("Zzzz", "ab12 monster");
        assert!(match_card(&c, "the ab12 monster truck").is_none());
    }

    #[test]
    fn name_match_only_proposes() {
        let c = card("Ducati Monster", "");
        let (matched, status) = match_card(&c, "sold my ducati monster last week").unwrap();
        assert_eq!(status, "proposed");
        assert_eq!(matched, "name");
    }

    #[test]
    fn short_names_do_not_propose() {
        let c = card("Cat", "");
        assert!(match_card(&c, "the cat sat on the mat").is_none());
    }

    #[test]
    fn a_reference_number_is_not_a_name() {
        // Observed live: the model answered "policy|AM-88214" for a rider it
        // could have named. A card called that helps nobody.
        let hay = "ashfield mutual studio equipment rider am-88214 covers the kiln";
        assert!(gate_suggestions("policy|AM-88214", hay).is_empty());
        assert_eq!(
            gate_suggestions("policy|Ashfield Mutual studio rider", hay).len(),
            0,
            "ungrounded names are still rejected"
        );
        let hay2 = "ashfield mutual studio rider covers the kiln";
        assert_eq!(
            gate_suggestions("policy|Ashfield Mutual studio rider", hay2).len(),
            1
        );
    }

    #[test]
    fn a_longer_restatement_is_the_same_thing() {
        // Two cards for one water heater is the mess the Registry prevents.
        assert!(same_thing(
            "Rheem Performance Platinum",
            "Rheem Performance Platinum water heater"
        ));
        assert!(same_thing("Sea Otter", "sea otter"));
        // Different things that merely share a start stay different.
        assert!(!same_thing("Apple", "Pineapple Studios"));
        assert!(!same_thing("Corran Marine Works", "Corran Marina"));
    }

    #[test]
    fn a_middle_name_does_not_split_a_person_in_two() {
        // Observed live: "Paul Thrasher" and "Paul Scott Thrasher" as two
        // person cards — prefix containment missed the insertion.
        assert!(same_thing("Paul Thrasher", "Paul Scott Thrasher"));
        assert!(same_thing("Ferrari 458 Spider", "2013 Ferrari 458 Spider"));
        // Same words out of order, or different versions, are not the same.
        assert!(!same_thing("Timberline 2.0", "Timberline 3.0"));
        assert!(!same_thing("Chen Maya", "Maya Chen Portfolio"));
    }

    #[test]
    fn identifier_wins_over_name() {
        let c = card("Ducati Monster", "wj9401ab2233");
        let (_, status) = match_card(&c, "ducati monster, vin wj9401ab2233").unwrap();
        assert_eq!(status, "confirmed");
    }

    #[test]
    fn an_abbreviated_address_is_the_same_address() {
        // The reminder's own example: one address, two spellings.
        assert!(same_thing(
            "15217 Canyon Seven Rd",
            "15217 Canyon Seven Road"
        ));
        assert!(same_thing("Main St. Studio", "Main Street Studio"));
        assert!(same_thing("123 N Elm Ave", "123 North Elm Avenue"));
        // Different house numbers are different places.
        assert!(!same_thing(
            "15217 Canyon Seven Rd",
            "15219 Canyon Seven Rd"
        ));
    }

    #[test]
    fn facts_ride_the_suggestion_only_when_verbatim() {
        let hay = "ducati monster, vin zdm1raz4xwb012345, first titled in 2019";
        let got = gate_suggestions(
            "asset|Ducati Monster|VIN: ZDM1RAZ4XWB012345|Year: 2019|Color: red",
            hay,
        );
        assert_eq!(got.len(), 1);
        let (_, name, facts) = &got[0];
        assert_eq!(name, "Ducati Monster");
        // "red" is nowhere in the material — an invented fact is discarded;
        // the grounded two survive.
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].label, "VIN");
        assert_eq!(facts[0].value, "ZDM1RAZ4XWB012345");
        assert_eq!(facts[1].value, "2019");
    }

    #[test]
    fn one_reply_never_mints_the_same_address_twice() {
        let hay = "deed for 15217 canyon seven rd; the 15217 canyon seven road property";
        let got = gate_suggestions(
            "asset|15217 Canyon Seven Rd\nasset|15217 Canyon Seven Road",
            hay,
        );
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn triage_replies_parse_forgivingly() {
        let all: Vec<usize> = parse_triage_reply("1, 3, 4", 5).into_iter().collect();
        assert_eq!(all.len(), 3);
        assert!(parse_triage_reply("none", 5).is_empty());
        assert!(parse_triage_reply("None stand out.", 5).is_empty());
        // Out-of-range numbers are noise, not verdicts.
        let picked = parse_triage_reply("Recommended: 2 and 7.", 5);
        assert_eq!(picked.len(), 1);
        assert!(picked.contains(&2));
    }

    #[test]
    fn snippet_stays_on_char_boundaries() {
        let text = "caf\u{e9} \u{201c}r\u{e9}sum\u{e9}\u{201d} the ducati monster lives here";
        let s = snippet_around(text, "ducati monster", 8).unwrap();
        assert!(s.contains("ducati monster"));
        assert!(snippet_around(text, "vespa", 8).is_none());
    }
}
