//! Prompt construction for grounded chat and one-shot artifact generation.

use crate::ai::{ChatTurn, UserProfile};
use crate::models::Citation;

/// Format the user's profile into a system-prompt block; empty when unset.
pub fn persona_block(profile: &UserProfile) -> String {
    let mut parts = Vec::new();
    if !profile.name.trim().is_empty() {
        parts.push(format!("Their name is {}.", profile.name.trim()));
    }
    if !profile.profession.trim().is_empty() {
        parts.push(format!("They work as: {}.", profile.profession.trim()));
    }
    if !profile.instructions.trim().is_empty() {
        parts.push(format!(
            "Standing instructions from them:\n{}",
            profile.instructions.trim()
        ));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(
            "About the user:\n{}\nKeep this in mind when relevant; the source material remains authoritative.",
            parts.join("\n")
        )
    }
}

const CHAT_SYSTEM: &str = "You are a research assistant that answers questions strictly from the provided source excerpts. \
Rules:\n\
- Use ONLY the information in the numbered excerpts below. Do not rely on outside knowledge.\n\
- Cite every claim with bracketed numbers matching the excerpt, e.g. [1] or [2][3].\n\
- If the excerpts do not contain the answer, say so plainly. Do not fabricate.\n\
- Be concise and well-structured. Prefer short paragraphs and bullet lists.\n\
- Exception: when the user asks you to FIND or ADD sources, you may propose full, concrete URLs — \
adapt the URLs of existing sources (e.g. change a search query in one) or use well-known sites. \
List each proposed URL on its own line and tell the user to reply \"add those links\" to import them.";

/// Build the chat message list from the retrieved citations and the question.
/// The citation list passed in becomes excerpts [1..n] in order. `sources` is
/// the full (title, url) list for the notebook — url empty for local files —
/// so the model can answer corpus-level questions ("what documents do we
/// have?") even when top-k retrieval only surfaced chunks from a few of them,
/// and can propose new addable URLs derived from existing ones.
pub fn build_chat_messages(
    history: &[ChatTurn],
    question: &str,
    citations: &[Citation],
    sources: &[(String, String)],
    extra_system: &str,
    persona: &str,
) -> Vec<ChatTurn> {
    let mut context = String::new();
    if citations.is_empty() {
        context.push_str("(No source excerpts matched this question.)");
    } else {
        for (i, c) in citations.iter().enumerate() {
            context.push_str(&format!(
                "[{}] (from \"{}\")\n{}\n\n",
                i + 1,
                c.source_title,
                c.snippet.trim()
            ));
        }
    }

    let mut system = if extra_system.trim().is_empty() {
        CHAT_SYSTEM.to_string()
    } else {
        format!(
            "{CHAT_SYSTEM}\n\nAdditional style guidance: {}",
            extra_system.trim()
        )
    };
    if !persona.is_empty() {
        system.push_str(&format!("\n\n{persona}"));
    }
    let mut messages = vec![ChatTurn::system(system)];
    // Keep a short rolling window of prior turns for conversational context.
    let start = history.len().saturating_sub(6);
    messages.extend(history[start..].iter().cloned());

    // A manifest of every source in the notebook. The excerpts below are only
    // the top matches for THIS question; this list is the whole corpus, so
    // "which documents are here?" is answerable without relying on retrieval.
    let manifest = if sources.is_empty() {
        "(none)".to_string()
    } else {
        sources
            .iter()
            .map(|(title, url)| {
                if url.is_empty() {
                    format!("- {title}")
                } else {
                    format!("- {title} — {url}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    messages.push(ChatTurn::user(format!(
        "Sources in this notebook ({} total):\n{manifest}\n\n\
         Source excerpts (top matches for this question only):\n\n{context}\n---\n\n\
         Question: {question}",
        sources.len()
    )));
    messages
}

/// (title, instruction) for each generated artifact kind.
pub fn artifact_spec(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "summary" => Some((
            "Summary",
            "Write a clear, structured summary of the sources below. Lead with a one-sentence overview, \
             then the key points as bullets. Keep it faithful to the sources.",
        )),
        "faq" => Some((
            "FAQ",
            "Generate a list of 8-12 frequently asked questions a reader would have about the sources below, \
             each with a concise answer grounded in the material. Format as **Q:** / **A:** pairs.",
        )),
        "study_guide" => Some((
            "Study Guide",
            "Create a study guide for the sources below. Include: key concepts (with one-line definitions), \
             5-8 review questions, and a short glossary. Use Markdown headings.",
        )),
        "briefing" => Some((
            "Briefing Doc",
            "Write an executive briefing document on the sources below: purpose, key findings, important details, \
             and open questions. Use Markdown headings and keep it skimmable.",
        )),
        "timeline" => Some((
            "Timeline",
            "Extract a chronological timeline of events, milestones, or developments mentioned in the sources below. \
             Format each entry as `- **<when>** — <what>`. If little temporal information exists, say so.",
        )),
        "problems" => Some((
            "Problems",
            "Critically analyze the sources below and identify the problems in them: factual errors, unsupported \
             claims, internal contradictions, gaps or missing information, risks, weak arguments, and open \
             questions. For each, use a `### <short problem title>` heading followed by: what the problem is, \
             where it appears (quote or reference the source), and why it matters. Be specific and grounded in \
             the sources — do not invent issues. If the sources are sound, say so and note only minor caveats.",
        )),
        "prd" => Some((
            "PRD",
            "Write a Product Requirements Document in the HashiCorp style, grounded in the sources below. \
             Use these Markdown sections: ## Overview, ## Problem Statement, ## Goals, ## Non-Goals, \
             ## User Stories, ## Requirements (functional and non-functional), ## Success Metrics, \
             ## Open Questions. Be concrete and concise; infer reasonable detail from the sources but do not fabricate facts.",
        )),
        "prfaq" => Some((
            "PR/FAQ",
            "Write a PR/FAQ (press release + FAQ) in the Amazon/HashiCorp working-backwards style, grounded in the sources below. \
             Start with a one-line **Headline** and **Subheadline**, then a press-release body written as if the product has shipped \
             (dateline, summary paragraph, customer problem, the solution, a customer quote, how to get started). \
             Then ## External FAQ (customer-facing) and ## Internal FAQ (team-facing: risks, dependencies, open questions). Do not invent quotes attributed to real people.",
        )),
        "rfc" => Some((
            "RFC",
            "Write an RFC (request for comments) in the HashiCorp engineering style, grounded in the sources below. \
             Use these Markdown sections: ## Summary, ## Background, ## Proposal, ## Rationale & Alternatives Considered, \
             ## Downsides & Risks, ## Open Questions, ## Decision. Write for an engineering audience; be specific and honest about trade-offs.",
        )),
        "skill" => Some((
            "Skill",
            "Produce a Claude Code SKILL.md that teaches how to perform the task described by the sources below. \
             Begin with YAML frontmatter delimited by `---` containing `name:` (kebab-case) and `description:` \
             (one sentence on when to use it). After the frontmatter, write Markdown with: a one-paragraph overview, \
             a ## Steps section with numbered, actionable instructions, and a ## Notes section for gotchas and tips. \
             Keep it practical and faithful to the sources.",
        )),
        _ => None,
    }
}

/// System prompt for the read-distillation sub-call: pull only the passages
/// relevant to the question out of a document, verbatim — so a full read
/// contributes evidence, not bulk, to the final answer context.
const DISTILL_SYSTEM: &str = "You extract evidence from a document for a research assistant. \
You are given a question and one document. Return ONLY the parts of the document that help \
answer the question:\n\
- Copy relevant passages VERBATIM — no paraphrasing, no commentary, no code fences.\n\
- Separate passages with a blank line, in document order.\n\
- Begin with a single line `NOTE: <one sentence on what the document is>` — the only line \
that may be your own words.\n\
- If nothing is relevant, return just the NOTE line describing what the document covers.\n\
Keep the total under about 500 words; prefer the few most load-bearing passages.";

/// Build the sub-read prompt that distills one document against the question.
pub fn build_distill_messages(question: &str, title: &str, content: &str) -> Vec<ChatTurn> {
    vec![
        ChatTurn::system(DISTILL_SYSTEM),
        ChatTurn::user(format!(
            "Question: {question}\n\nDocument \"{title}\":\n\n{content}"
        )),
    ]
}

const RERANK_SYSTEM: &str =
    "You rank search results for relevance. Given a question and a numbered \
list of passages, respond with EXACTLY ONE JSON object and nothing else: \
{\"keep\":[<indices of the passages that help answer the question, most relevant first>]}. \
Exclude passages that are off-topic even if they share keywords with the question.";

/// Build the rerank prompt: pick the `keep` most relevant of the numbered snippets.
pub fn build_rerank_messages(
    question: &str,
    snippets: &[(String, String)],
    keep: usize,
) -> Vec<ChatTurn> {
    let list = snippets
        .iter()
        .enumerate()
        .map(|(i, (title, text))| format!("[{i}] ({title}) {text}"))
        .collect::<Vec<_>>()
        .join("\n");
    vec![
        ChatTurn::system(RERANK_SYSTEM),
        ChatTurn::user(format!(
            "Question: {question}\n\nPassages:\n{list}\n\n\
             Keep at most {keep}. One JSON object:"
        )),
    ]
}

const AGENT_SYSTEM: &str = "You are a retrieval planner for a research assistant. Your job is NOT to answer the \
question yet — it is to decide the next retrieval step that will gather the evidence needed to answer it well.\n\n\
Respond with EXACTLY ONE JSON object and nothing else. Valid actions:\n\
- {\"action\":\"search\",\"query\":\"<focused search phrase>\"}  — vector-search the sources for a sub-topic.\n\
- {\"action\":\"read\",\"sourceId\":\"<id>\"}  — read a full source when you need broad context from it.\n\
- {\"action\":\"answer\"}  — stop; enough evidence has been gathered.\n\n\
Guidance: break multi-part questions into several searches across turns. Prefer distinct queries that cover \
different facets. Choose \"answer\" once the gathered excerpts can support a complete, grounded response.";

/// Build the planner prompt for one step of the agentic retrieval loop.
pub fn build_agent_decision(
    question: &str,
    source_list: &str,
    transcript: &str,
    gathered_count: usize,
) -> Vec<ChatTurn> {
    let gathered = if transcript.trim().is_empty() {
        "(nothing yet)".to_string()
    } else {
        transcript.to_string()
    };
    vec![
        ChatTurn::system(AGENT_SYSTEM),
        ChatTurn::user(format!(
            "Question: {question}\n\nAvailable sources:\n{source_list}\n\n\
             Evidence gathered so far ({gathered_count} excerpts):\n{gathered}\n\n\
             Next action (one JSON object):"
        )),
    ]
}

/// Build the message list for a one-shot artifact over concatenated source text.
pub fn build_artifact_messages(instruction: &str, corpus: &str, persona: &str) -> Vec<ChatTurn> {
    // Safety net only — callers budget the corpus per-source upstream.
    // Truncate on a char boundary (byte slicing can panic on Unicode).
    const MAX_CHARS: usize = 200_000;
    let corpus: String = corpus.chars().take(MAX_CHARS).collect();
    let corpus = corpus.as_str();
    let mut system = "You generate well-structured Markdown documents from provided source \
                      material. Stay faithful to the sources and never invent facts."
        .to_string();
    if !persona.is_empty() {
        system.push_str(&format!("\n\n{persona}"));
    }
    vec![
        ChatTurn::system(system),
        ChatTurn::user(format!("{instruction}\n\n--- SOURCES ---\n\n{corpus}")),
    ]
}
