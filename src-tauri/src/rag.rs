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

const META_SYSTEM: &str = "You are a research assistant answering questions across the user's ENTIRE library of notebooks (docs/RFC-meta-chat.md). \
Rules:\n\
- Use ONLY the numbered excerpts below. Do not rely on outside knowledge.\n\
- Every excerpt is tagged with the notebook it lives in. When the question is about WHERE something is (\"which notebook…\", \"where did I save…\"), name the notebook plainly and early, in **bold**.\n\
- Cite claims with bracketed numbers matching the excerpts, e.g. [1] or [2][3].\n\
- If the excerpts do not contain the answer, say so plainly. Do not fabricate.\n\
- Be concise — this answer renders in a small palette, not a document. Short paragraphs, no headers.";

/// One passage feeding a meta-chat answer (see commands::MetaCitation).
/// `number` is the SOURCE's reference number — several excerpts from the
/// same source share it, so the model's [n] citations line up with the
/// deduplicated reference list the UI shows.
pub struct MetaPassage {
    pub number: usize,
    /// "source" (document excerpt) | "note" (a prior conclusion).
    pub kind: String,
    pub notebook_title: String,
    pub title: String,
    pub snippet: String,
}

/// Build the corpus-wide chat message list: like `build_chat_messages`, but
/// each excerpt names its notebook and there is no per-notebook manifest.
pub fn build_meta_messages(
    history: &[ChatTurn],
    question: &str,
    passages: &[MetaPassage],
    persona: &str,
) -> Vec<ChatTurn> {
    let mut context = String::new();
    if passages.is_empty() {
        context.push_str("(No passages in any notebook matched this question.)");
    } else {
        for p in passages {
            context.push_str(&format!(
                "[{}] (notebook: \"{}\" · {}: \"{}\")\n{}\n\n",
                p.number,
                p.notebook_title,
                p.kind,
                p.title,
                p.snippet.trim()
            ));
        }
    }

    let mut system = META_SYSTEM.to_string();
    if !persona.is_empty() {
        system.push_str(&format!("\n\n{persona}"));
    }
    let mut messages = vec![ChatTurn::system(system)];
    let start = history.len().saturating_sub(6);
    messages.extend(history[start..].iter().cloned());
    messages.push(ChatTurn::user(format!(
        "Excerpts from across all notebooks:\n\n{context}\nQuestion: {question}"
    )));
    messages
}

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
            // Notes are prior conclusions, not source documents — say so, so
            // the model never presents its own earlier synthesis as evidence.
            let tier = if c.note_id.is_empty() {
                "from"
            } else {
                "from note"
            };
            context.push_str(&format!(
                "[{}] ({} \"{}\")\n{}\n\n",
                i + 1,
                tier,
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

/// System prompt for the auto-evidence post-pass: after a chat answer that
/// synthesized across sources, one model call decides whether the exchange
/// produced a durable conclusion and drafts the evidence record if so
/// (docs/RFC-note-curator.md phase 3). The model must opt IN — the default
/// is SKIP, and malformed output is treated as SKIP.
const AUTO_EVIDENCE_SYSTEM: &str =
    "You review ONE exchange from a research-notebook chat and decide \
whether it produced a conclusion worth preserving as an evidence record — a durable note a future \
reader (human or agent) can audit and build on.\n\
\n\
Reply with exactly SKIP when the exchange is not worth a record: simple lookups, casual or \
exploratory turns, answers that restate a single source, questions about the notebook itself, \
or anything without a real cross-source conclusion. Most exchanges are SKIP.\n\
\n\
Otherwise reply in exactly this form:\n\
TITLE: <the claim itself in one line — a specific statement, not a topic label>\n\
\n\
**Claim:** one sentence.\n\
**Evidence:** the 1-3 load-bearing passages, quoted from the excerpts below, each naming its \
source title. Use ONLY the provided excerpts.\n\
**Confidence:** high / medium / low, with a phrase on why (corroborated across sources, single \
source, dated…).\n\
**Counter-evidence:** anything in the excerpts that cuts against the claim, or \"none found\".\n\
**Open questions:** what would firm this up, if anything.\n\
\n\
When a PRIOR RECORD is provided, the claim was recorded before: reply with the MERGED record in \
the same form — keep its still-valid evidence, fold in the new, and update the confidence.";

/// Build the auto-evidence post-pass messages. `citations` should be source
/// passages only (note passages are prior conclusions — evidence derived
/// from them would be circular). `prior` is an existing record to merge.
pub fn build_auto_evidence_messages(
    question: &str,
    answer: &str,
    citations: &[Citation],
    prior: Option<(&str, &str)>,
) -> Vec<ChatTurn> {
    let mut excerpts = String::new();
    for (i, c) in citations.iter().enumerate() {
        excerpts.push_str(&format!(
            "[{}] (from \"{}\")\n{}\n\n",
            i + 1,
            c.source_title,
            c.snippet.trim()
        ));
    }
    let mut user = format!(
        "Question: {question}\n\nAnswer given:\n{answer}\n\nSource excerpts behind the answer:\n\n{excerpts}"
    );
    if let Some((title, content)) = prior {
        user.push_str(&format!(
            "\nPRIOR RECORD \"{}\":\n{}\n",
            title,
            content.trim()
        ));
    }
    vec![ChatTurn::system(AUTO_EVIDENCE_SYSTEM), ChatTurn::user(user)]
}

/// System prompt for curator consolidation (docs/RFC-note-curator.md phase
/// 5): judge whether two auto evidence records state the same claim and, if
/// so, write the single merged record. KEEP is the instructed default;
/// `parse_auto_evidence` treats KEEP (and anything malformed) as None.
const CONSOLIDATE_SYSTEM: &str = "You review TWO evidence records from the same research notebook \
and decide whether they record the SAME underlying claim.\n\
\n\
Reply with exactly KEEP when they are distinct claims. Related topics are still distinct claims. \
When in doubt, KEEP.\n\
\n\
Only when they state the same claim, reply with the single merged record:\n\
TITLE: <the claim in one line>\n\
\n\
**Claim:** one sentence.\n\
**Evidence:** every load-bearing passage from BOTH records, deduplicated, each naming its source.\n\
**Confidence:** high / medium / low for the combined evidence, with why.\n\
**Counter-evidence:** from either record, or \"none found\".\n\
**Open questions:** whatever remains open after combining.\n\
\n\
Use ONLY material from the two records — invent nothing.";

/// Build the consolidation judgment for one candidate pair.
pub fn build_consolidate_messages(
    a_title: &str,
    a_content: &str,
    b_title: &str,
    b_content: &str,
) -> Vec<ChatTurn> {
    vec![
        ChatTurn::system(CONSOLIDATE_SYSTEM),
        ChatTurn::user(format!(
            "Record A \"{a_title}\":\n{}\n\nRecord B \"{b_title}\":\n{}",
            a_content.trim(),
            b_content.trim()
        )),
    ]
}

/// Parse the post-pass reply: None = skip (explicit SKIP/KEEP or anything
/// without a TITLE line — conservatism is the point), Some((title, body))
/// otherwise. Tolerant of how models actually write: markdown decoration
/// around the markers ("**TITLE:**", "Title:") and reasoning preamble
/// before the record.
pub fn parse_auto_evidence(raw: &str) -> Option<(String, String)> {
    let text = raw.trim();
    // The first non-empty line decides the decline path, decoration and
    // casing aside ("SKIP", "**SKIP**", "Decision: KEEP — distinct claims").
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let first_up = first.to_uppercase();
    if !first_up.contains("TITLE:") && (first_up.contains("SKIP") || first_up.contains("KEEP")) {
        return None;
    }
    let deco = |c: char| c == '*' || c == '_' || c == '#' || c == '>';
    let mut lines = text.lines();
    let title = loop {
        let line = lines.next()?; // no TITLE anywhere = skip
        let clean = line.trim().trim_start_matches(deco).trim_start();
        if clean
            .get(..6)
            .is_some_and(|p| p.eq_ignore_ascii_case("title:"))
        {
            let t = clean[6..].trim().trim_matches(deco).trim();
            if t.is_empty() {
                return None;
            }
            break t.to_string();
        }
    };
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    if body.is_empty() {
        return None;
    }
    Some((title, body))
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
        "insights" => Some((
            "Insights",
            "Surface the non-obvious insights hiding across the sources below — the things a reader \
             skimming each document separately would miss. Look specifically for: connections between \
             different sources (one source's claim explains or extends another's), agreements and \
             contradictions between sources, surprising or counterintuitive facts, recurring themes, \
             and notable gaps nothing covers. Write 4-8 insights. For each, use a `### <one-line \
             insight>` heading stating the takeaway itself (not a topic label), then 2-4 sentences of \
             support naming the specific sources involved and why it matters. Order by how interesting \
             the insight is, most interesting first. Only report what the sources support — if the \
             sources are too thin or too similar for real cross-source insights, say so briefly \
             instead of inventing them.",
        )),
        "flashcards" => Some((
            "Flashcards",
            "Create a deck of flashcards covering the most important facts, concepts, and definitions \
             in the sources below. Make 12-20 cards (fewer if the material is thin — never pad). \
             Spread the cards across ALL the sources and order them from foundational to advanced. \
             Each front is one focused question or term; each back is a concise answer (1-3 sentences) \
             that stands alone without the question. Avoid yes/no questions and avoid answers that \
             just restate the front. Format each card exactly as:\n\
             **Front:** <question or term>\n\
             **Back:** <answer>\n\
             separated by `---` lines, with no headings or preamble.",
        )),
        "quiz" => Some((
            "Quiz",
            "Write a multiple-choice quiz that tests real understanding of the sources below. Create \
             8-12 questions spanning ALL the sources, mixing recall with comprehension (why/how, \
             implications, comparisons) — not just surface facts. Each question has options A-D with \
             exactly one correct answer and plausible distractors drawn from the material (no joke \
             options, no \"all of the above\"). Number the questions under a `## Questions` heading, \
             each option on its own line. Then add a `## Answer Key` section listing each number's \
             correct letter with a one-sentence explanation of why it is right.",
        )),
        "data_table" => Some((
            "Data Table",
            "Distill the sources below into structured reference tables. First decide what the \
             natural rows are (events, entities, options, studies, versions, claims — whatever the \
             material dictates) and pick 3-6 columns that capture what matters about each. Produce \
             one Markdown (GFM) table per natural grouping — usually one, at most three — with a \
             short `##` heading per table. Keep cells terse (numbers, names, fragments — not \
             sentences), use consistent units, and put `—` in cells the sources don't answer. After \
             the tables, add a short **Notes** list for caveats or conflicting figures between \
             sources. Every value must come from the sources — do not estimate.",
        )),
        "audio_overview" => Some((
            "Audio Overview",
            "Write the script for a two-host podcast episode discussing the sources below — \
             aim for roughly 3,000 words (a twenty-minute listen); going long beats going \
             short. HOST is a warm, curious interviewer who frames the big questions, reacts, \
             and keeps the thread moving; GUEST is the expert who answers with specifics from \
             the material, reaching for analogies and concrete examples. Structure it like a \
             real episode: a cold-open hook (the most surprising fact or sharpest question in \
             the sources), a one-breath preview of where the conversation is going, then 4-6 \
             segments that each dig deep into one thread with natural transitions between \
             them, a brief mid-episode recap of what's emerged so far, and a closing exchange \
             that lands the takeaways. Make it SOUND spoken, not written: contractions \
             everywhere, short reaction beats (\"Right.\", \"Huh — okay.\", \"Wait, really?\"), \
             rhetorical questions, one host occasionally interrupting or finishing the \
             other's thought, and genuine pushback wherever the sources disagree. Use \
             punctuation as delivery: em dashes for interruptions and pivots, ellipses for a \
             beat of hesitation, question and exclamation marks for lift. Vary the rhythm — \
             quick three-word volleys against longer explanations. Write ONLY dialogue lines, \
             each on its own line, in exactly this form:\n\
             HOST: <what they say>\n\
             GUEST: <what they say>\n\
             No headings, no stage directions, no sound-effect cues, no markdown, no names \
             other than HOST and GUEST. Keep any single turn under 80 words.",
        )),
        "slide_deck" => Some((
            "Slide Deck",
            "Turn the sources below into a presentation deck in Marp-style Markdown, working \
             like a world-class presentation designer. The deck must tell ONE clear story and \
             stand on its own without a presenter.\n\
             Plan silently before writing: decide the single thing a reader should walk away \
             believing, the 2-4 acts that build to it, and the strongest specific evidence in \
             the sources for each act — numbers, names, dates, comparisons, quotes. Only then \
             write slides.\n\
             Start the output with a front-matter block — a `---` line, `theme: <name>`, \
             `font: <name>`, and another `---` line. Pick the theme whose mood fits the topic: \
             sepia (warm paper — essays, history, writing), latte (soft light — friendly, \
             everyday), github-light (clean light — docs, how-tos), slate (cool neutral — \
             technical, product), midnight (dark indigo — engineering, systems), nord (icy blue \
             — science, data, research), gruvbox (earthy retro — nature, craft, outdoors), \
             dracula (playful dark — community, fun), synthwave (neon dark — bold takes, \
             launches), matrix (green terminal — security, hacking). Pick the font the same \
             way: serif (essays, history, humanities), sans (product, business, general), mono \
             (engineering, terminals, protocols), rounded (friendly, consumer, kids). Slides \
             are separated by `---` lines.\n\
             Headlines are ASSERTIONS, not topics: every `##` on a content slide states that \
             slide's takeaway as a claim a reader could agree or disagree with (\"LanceDB \
             removes the database server entirely\", never \"Storage\"). One idea per slide. \
             The body is the evidence for its headline — concrete specifics pulled from the \
             sources; if a bullet would survive on any other deck about any other topic, it is \
             filler: cut it.\n\
             Choose each slide's FORMAT to match what it is saying — a deck of identical \
             heading-plus-list slides is a failure. The shapes: (1) title slide, first: `# \
             <deck title>` plus one italic subtitle that promises what the deck answers; (2) \
             content slide, the workhorse: assertion `## <headline>` with 3-6 parallel bullets \
             — full clauses of 8-16 words each, 40-80 words per slide; sub-bullets for \
             supporting detail; (3) section divider, a lone `## <act title>`, opening each act; \
             (4) statement slide, exactly one — no heading, a single punchy sentence under 20 \
             words carrying the deck's most surprising fact or number, placed where it lands \
             hardest; (5) quote slide — a `> blockquote` plus an attribution line — when a \
             source has a genuinely quotable line; (6) table slide — `## <headline>` plus a \
             small GFM table (3-5 columns) — for ANY comparison of options, versions, or \
             trade-offs; tables beat bullets for comparisons every time.\n\
             Stay faithful: every claim comes from the sources, nothing invented; where sources \
             disagree, show the disagreement instead of smoothing it over. Cover the whole \
             corpus in 10-16 slides building from context to specifics, and close with a `## \
             Takeaways` slide of 3-5 bullets that pays off the title slide's promise and says \
             what to do next. Output ONLY the deck markdown: no code fences around it, no \
             speaker notes, no prose outside the slides.",
        )),
        "mind_map" => Some((
            "Mind Map",
            "Distill the sources below into a mind-map outline. The FIRST line is the central \
             topic: 2-5 words, no bullet, no heading marks. Every following line is a `- ` bullet, \
             indented by exactly two additional spaces per level. Use 3-6 main branches that \
             together cover the whole corpus, each with 2-5 sub-items, going at most 3 levels \
             below the root. Keep every label short — 5 words or fewer, telegraphic style. \
             Output ONLY the outline: no prose before or after, no headings, no code fences, \
             no bold or other formatting.",
        )),
        "problems" => Some((
            "Problems",
            "Critically analyze the sources below and identify the problems in them: factual errors, unsupported \
             claims, internal contradictions, gaps or missing information, risks, weak arguments, and open \
             questions. For each, use a `### <short problem title>` heading followed by: what the problem is, \
             where it appears (quote or reference the source), and why it matters. Be specific and grounded in \
             the sources — do not invent issues. If the sources are sound, say so and note only minor caveats.",
        )),
        "evidence" => Some((
            "Evidence Log",
            "Build an evidence log from the sources below: the major claims, findings, or decisions \
             the corpus supports, each as a durable record another reader (or agent) can audit later. \
             Write 4-10 records. For each, use a `### <the claim itself>` heading — a one-line \
             statement, not a topic label — followed by:\n\
             - **Evidence:** the 1-3 most load-bearing passages, quoted verbatim or tightly \
               paraphrased, each naming its source title.\n\
             - **Confidence:** high / medium / low, with a phrase on why (corroborated across \
               sources, single source, secondhand, dated…).\n\
             - **Counter-evidence:** anything in the sources that cuts against the claim, or \
               \"none found\".\n\
             - **Open questions:** what would firm this up, if anything.\n\
             Order by importance, strongest first. Only record what the sources actually support — \
             if the corpus is too thin for real evidence records, say so instead of padding.",
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

/// Continuation prompt for an under-length Audio Overview script: more
/// dialogue picking up mid-episode — never a restart.
pub fn build_audio_continuation(
    instruction: &str,
    corpus: &str,
    persona: &str,
    script_so_far: &str,
) -> Vec<ChatTurn> {
    const MAX_CHARS: usize = 150_000;
    let corpus: String = corpus.chars().take(MAX_CHARS).collect();
    let mut system = "You continue writing a two-host podcast script from provided source \
                      material. Stay faithful to the sources and never invent facts."
        .to_string();
    if !persona.is_empty() {
        system.push_str(&format!("\n\n{persona}"));
    }
    vec![
        ChatTurn::system(system),
        ChatTurn::user(format!(
            "Episode brief:\n{instruction}\n\n--- SOURCES ---\n\n{corpus}\n\n\
             --- EPISODE SO FAR ---\n\n{script_so_far}\n\n--- TASK ---\n\
             The episode above stopped short of its length target. Continue it from exactly \
             where it leaves off: new dialogue lines only, in the same one-line-per-turn \
             HOST:/GUEST: format. Do not restart, re-introduce the show, or repeat ground \
             already covered — pick up mid-conversation and dig into the threads and details \
             from the sources that haven't been discussed yet. When (and only when) the full \
             episode reaches the target length, land the takeaways in a proper closing \
             exchange. Output ONLY the new lines.",
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
