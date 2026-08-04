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
    let existing = db.list_registry().await.unwrap_or_default();
    let mut proposed = 0usize;

    for nb in db.list_notebooks().await? {
        {
            let mut guard = SUGGESTED.lock().unwrap();
            let seen = guard.get_or_insert_with(Default::default);
            if !seen.insert(nb.id.clone()) {
                continue;
            }
        }
        proposed += suggest_for_notebook(db, ai, &nb.id, &gists, &existing, None)
            .await
            .unwrap_or_default()
            .len();
    }
    if proposed > 0 {
        super::notify_changed("registry", None);
    }
    Ok(proposed)
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
        for (kind, name) in gated {
            // Anything already in the cast — yours, pending, or turned down
            // — is settled. Never re-propose it.
            if existing.iter().any(|c| same_thing(&c.name, &name)) {
                continue;
            }
            let ts = now();
            let card = RegistryCard {
                id: new_id(),
                kind,
                name: name.clone(),
                origin: "auto".into(),
                identifiers: String::new(),
                note: String::new(),
                facts: Vec::new(),
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

/// Whether a proposed name restates a card that already exists.
///
/// Exact match isn't enough: across runs the same model names one object
/// two ways — "Rheem Performance Platinum" and "Rheem Performance Platinum
/// water heater" — and two cards for one water heater is precisely the mess
/// the Registry exists to prevent. Prefix containment catches the
/// restatement while leaving genuinely different things alone ("Apple" does
/// not swallow "Pineapple Studios"), and it only ever suppresses a
/// suggestion, never a card someone made.
fn same_thing(existing: &str, proposed: &str) -> bool {
    let a = existing.trim().to_lowercase();
    let b = proposed.trim().to_lowercase();
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    // Word-boundary prefix only, so "Sea Otter" never absorbs "Sea Otter II"
    // by accident of length alone — it has to read as the same name with
    // more said after it.
    short.chars().count() >= MIN_NAME_LEN
        && long.starts_with(short.as_str())
        && long[short.len()..].starts_with(' ')
}

/// Keep only proposals that are grounded and well-formed.
///
/// The load-bearing check is the last one: the name must appear verbatim in
/// the material. An invented card is worse than no card — it is a thing in
/// your registry that was never in your life.
fn gate_suggestions(reply: &str, haystack: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
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
        let joined = parts[at + 1..].join(" ");
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
        if out.iter().any(|(_, n)| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push((kind, name.to_string()));
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

/// Re-run matching over the whole corpus, in the background.
fn spawn_rematch_all(db: std::sync::Arc<crate::db::Db>) {
    tauri::async_runtime::spawn(async move {
        let Ok(notebooks) = db.list_notebooks().await else {
            return;
        };
        for nb in notebooks {
            let Ok(sources) = db.list_sources(&nb.id).await else {
                continue;
            };
            for s in sources {
                if s.source_type == "folder" {
                    continue;
                }
                let Ok(text) = db.source_content(&s.id).await else {
                    continue;
                };
                match_source_to_cards(&db, &nb.id, &s.id, &text).await;
            }
        }
    });
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
    let sources = e(state.db.list_sources(&notebook_id).await)?;
    let mut filed = 0;
    for s in sources {
        if s.source_type == "folder" {
            continue;
        }
        let Ok(text) = state.db.source_content(&s.id).await else {
            continue;
        };
        filed += match_source_to_cards(&state.db, &notebook_id, &s.id, &text).await;
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
    fn identifier_wins_over_name() {
        let c = card("Ducati Monster", "wj9401ab2233");
        let (_, status) = match_card(&c, "ducati monster, vin wj9401ab2233").unwrap();
        assert_eq!(status, "confirmed");
    }
}
