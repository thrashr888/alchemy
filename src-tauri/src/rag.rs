//! Prompt construction for grounded chat and one-shot artifact generation.

use crate::ai::ChatTurn;
use crate::models::Citation;

const CHAT_SYSTEM: &str = "You are a research assistant that answers questions strictly from the provided source excerpts. \
Rules:\n\
- Use ONLY the information in the numbered excerpts below. Do not rely on outside knowledge.\n\
- Cite every claim with bracketed numbers matching the excerpt, e.g. [1] or [2][3].\n\
- If the excerpts do not contain the answer, say so plainly. Do not fabricate.\n\
- Be concise and well-structured. Prefer short paragraphs and bullet lists.";

/// Build the chat message list from the retrieved citations and the question.
/// The citation list passed in becomes excerpts [1..n] in order.
pub fn build_chat_messages(
    history: &[ChatTurn],
    question: &str,
    citations: &[Citation],
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

    let mut messages = vec![ChatTurn::system(CHAT_SYSTEM)];
    // Keep a short rolling window of prior turns for conversational context.
    let start = history.len().saturating_sub(6);
    messages.extend(history[start..].iter().cloned());
    messages.push(ChatTurn::user(format!(
        "Source excerpts:\n\n{context}\n---\n\nQuestion: {question}"
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
pub fn build_artifact_messages(instruction: &str, corpus: &str) -> Vec<ChatTurn> {
    // Guard against blowing the context window on very large notebooks.
    // Truncate on a char boundary (byte slicing can panic on Unicode).
    const MAX_CHARS: usize = 24_000;
    let corpus: String = corpus.chars().take(MAX_CHARS).collect();
    let corpus = corpus.as_str();
    vec![
        ChatTurn::system(
            "You generate well-structured Markdown documents from provided source material. \
             Stay faithful to the sources and never invent facts.",
        ),
        ChatTurn::user(format!("{instruction}\n\n--- SOURCES ---\n\n{corpus}")),
    ]
}
