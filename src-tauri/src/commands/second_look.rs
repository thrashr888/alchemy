//! The Second Look (docs/RFC-second-look.md): claim-by-claim verification
//! of a draft. The draft is split into checkable claims, each claim gets a
//! fresh hybrid retrieval over the notebook — excluding the draft's own
//! chunks, so it can never support itself — and the Small role (a different
//! engine than the one that writes prose here) judges each claim against
//! what came back: supported / weak / unsupported / contradicted. Verdicts
//! that fail the strict parse are reported as unjudged, never dropped.

use tauri::{AppHandle, Emitter, Manager, State};

use crate::ai::ChatTurn;
use crate::inference::Role;
use crate::models::{Citation, Note};

use super::{add_note_indexed, e, new_id, now, AppState};

const MAX_CLAIMS: usize = 20;
const MIN_CLAIM_CHARS: usize = 40;
const K: usize = 6;
const EXCERPT_CAP: usize = 700;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimVerdict {
    pub claim: String,
    /// supported | weak | unsupported | contradicted | unjudged
    pub verdict: String,
    pub reason: String,
    /// Source title of the strongest fresh excerpt (empty when none).
    pub evidence_title: String,
    /// The strongest fresh excerpt itself, capped.
    pub evidence_snippet: String,
}

/// Fire-and-forget from the UI: the report note lands beside the draft,
/// with an event + notification when done.
#[tauri::command]
pub async fn run_second_look(
    app: AppHandle,
    state: State<'_, AppState>,
    note_id: String,
) -> Result<(), String> {
    let Some(note) = e(state.db.get_note(&note_id).await)? else {
        return Err("no note with that id".into());
    };
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        match second_look_pass(
            &state,
            &note.notebook_id,
            Some(&note.id),
            &note.title,
            &note.content,
        )
        .await
        {
            Ok((report, verdicts)) => {
                #[derive(serde::Serialize, Clone)]
                #[serde(rename_all = "camelCase")]
                struct Changed<'a> {
                    scope: &'a str,
                    notebook_id: Option<&'a str>,
                }
                let _ = app.emit(
                    "mcp://changed",
                    Changed {
                        scope: "notes",
                        notebook_id: Some(&report.notebook_id),
                    },
                );
                if crate::scheduler::notifications_wanted(&app).await {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = app
                        .notification()
                        .builder()
                        .title("Second Look finished")
                        .body(format!("{} — {}", report.title, count_line(&verdicts)))
                        .show();
                }
            }
            Err(err) => crate::note!("second look: {err:#}"),
        }
    });
    Ok(())
}

/// The whole pass: split → retrieve fresh → judge → report note.
/// `exclude_note_id` keeps the draft's own chunks out of its evidence.
pub(crate) async fn second_look_pass(
    state: &AppState,
    notebook_id: &str,
    exclude_note_id: Option<&str>,
    title: &str,
    text: &str,
) -> anyhow::Result<(Note, Vec<ClaimVerdict>)> {
    let ai = state.ai.read().await.clone();

    // 1. Split into checkable claims (strict numbered format, parse-or-skip).
    let split = ai
        .chat_role(
            Role::Small,
            &[
                ChatTurn::system(
                    "Split the draft into independent, checkable factual claims. Output ONLY \
                     numbered lines like \"1. <claim>\". Each claim must stand alone (name its \
                     subject, no dangling pronouns) and assert something a document could confirm \
                     or refute. Skip greetings, formatting, opinions, and questions. At most 20.",
                ),
                ChatTurn::user(text.chars().take(24_000).collect::<String>()),
            ],
        )
        .await?
        .text;
    let claims: Vec<String> = split
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(|c: char| c.is_ascii_digit())?;
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            let rest = rest.strip_prefix('.')?.trim();
            (rest.chars().count() >= MIN_CLAIM_CHARS).then(|| rest.to_string())
        })
        .take(MAX_CLAIMS)
        .collect();
    if claims.is_empty() {
        anyhow::bail!("no checkable claims found in the draft");
    }

    // 2. One embed call covers every claim.
    let vectors = ai.embed(&claims).await?;

    // 3. Fresh retrieval + judgment per claim.
    let mut verdicts = Vec::with_capacity(claims.len());
    for (claim, vec) in claims.iter().zip(vectors) {
        let hits: Vec<Citation> = state
            .db
            .search_chunks(notebook_id, vec, claim, K, None)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c| exclude_note_id.is_none_or(|id| c.note_id != id))
            .collect();
        verdicts.push(judge(&ai, claim, &hits).await);
    }

    // 4. The report note, beside the draft.
    let ts = now();
    let report = Note {
        id: new_id(),
        notebook_id: notebook_id.to_string(),
        title: format!("Second Look: {}", title.trim()),
        content: report_markdown(title, &verdicts),
        kind: "note".into(),
        prompt: String::new(),
        origin: "second-look".into(),
        status: String::new(),
        created_at: ts,
        updated_at: ts,
    };
    add_note_indexed(state, &report).await?;
    Ok((report, verdicts))
}

/// One strict verdict; anything malformed is unjudged, never dropped.
async fn judge(ai: &crate::ai::Ai, claim: &str, hits: &[Citation]) -> ClaimVerdict {
    let mut best = ClaimVerdict {
        claim: claim.to_string(),
        verdict: "unjudged".into(),
        reason: String::new(),
        evidence_title: String::new(),
        evidence_snippet: String::new(),
    };
    if hits.is_empty() {
        best.verdict = "unsupported".into();
        best.reason = "A fresh search returned nothing relevant.".into();
        return best;
    }
    let excerpts = hits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "[{}] ({}) {}",
                i + 1,
                c.source_title,
                c.snippet.chars().take(EXCERPT_CAP).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let reply = ai
        .chat_role(
            Role::Small,
            &[
                ChatTurn::system(
                    "You verify one claim against freshly retrieved excerpts. Reply with exactly \
                     three lines:\nVERDICT: one of supported | weak | unsupported | \
                     contradicted\nEVIDENCE: the number of the single strongest excerpt, or 0\n\
                     REASON: one short sentence naming the deciding evidence.\nsupported needs an \
                     excerpt that states the claim's substance; weak means related but not \
                     confirming; contradicted requires an excerpt INCOMPATIBLE with the claim.",
                ),
                ChatTurn::user(format!("CLAIM:\n{claim}\n\nEXCERPTS:\n{excerpts}")),
            ],
        )
        .await;
    let Ok(reply) = reply else { return best };
    let mut evidence_idx = 0usize;
    for line in reply.text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("VERDICT:") {
            let word = rest.trim().to_lowercase();
            if ["supported", "weak", "unsupported", "contradicted"].contains(&word.as_str()) {
                best.verdict = word;
            }
        } else if let Some(rest) = line.strip_prefix("EVIDENCE:") {
            evidence_idx = rest.trim().parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("REASON:") {
            best.reason = rest.trim().chars().take(300).collect();
        }
    }
    if best.verdict == "unjudged" {
        best.reason = String::new(); // strict: no verdict, no borrowed reason
    } else if let Some(hit) = evidence_idx.checked_sub(1).and_then(|i| hits.get(i)) {
        best.evidence_title = hit.source_title.clone();
        best.evidence_snippet = hit.snippet.chars().take(EXCERPT_CAP).collect();
    }
    best
}

pub(crate) fn count_line(verdicts: &[ClaimVerdict]) -> String {
    let count = |v: &str| verdicts.iter().filter(|c| c.verdict == v).count();
    let mut parts = vec![
        format!("{} supported", count("supported")),
        format!("{} weak", count("weak")),
        format!("{} unsupported", count("unsupported")),
        format!("{} contradicted", count("contradicted")),
    ];
    let unjudged = count("unjudged");
    if unjudged > 0 {
        parts.push(format!("{unjudged} unjudged"));
    }
    parts.join(" · ")
}

fn report_markdown(title: &str, verdicts: &[ClaimVerdict]) -> String {
    let mut out = format!(
        "Second Look at \u{201c}{}\u{201d} — {} claims: {}\n",
        title.trim(),
        verdicts.len(),
        count_line(verdicts)
    );
    for (i, v) in verdicts.iter().enumerate() {
        let label = match v.verdict.as_str() {
            "supported" => "Supported",
            "weak" => "Weakly supported",
            "unsupported" => "Unsupported",
            "contradicted" => "Contradicted",
            _ => "Unjudged",
        };
        out.push_str(&format!("\n## {}. {label}\n\n{}\n", i + 1, v.claim));
        if !v.reason.is_empty() {
            out.push_str(&format!("\n{}\n", v.reason));
        }
        if !v.evidence_snippet.is_empty() {
            out.push_str(&format!(
                "\n> {}\n> — {}\n",
                v.evidence_snippet.replace('\n', " "),
                v.evidence_title
            ));
        }
    }
    out
}
