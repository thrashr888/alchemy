//! User template folder — custom Studio generators.
//!
//! Each `~/Documents/Alchemy/templates/*.md` file is one generator: markdown
//! with a small frontmatter block (`name:` / `description:`) and the
//! generation instruction as the body. The body runs through the existing
//! custom-prompt generation path over the notebook's sources. A default pack
//! is written when the folder is first created; user edits and deletions are
//! never overwritten.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    /// Filename stem, e.g. "swot-analysis".
    pub id: String,
    pub name: String,
    pub description: String,
    /// The generation instruction (file body), run over the notebook's sources.
    pub prompt: String,
}

/// `~/Documents/Alchemy/templates`. None only when $HOME is unset.
fn templates_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents/Alchemy/templates"))
}

/// The starter pack, written once when the folder is created: (id, file contents).
const DEFAULT_TEMPLATES: &[(&str, &str)] = &[
    (
        "swot-analysis",
        "---\n\
         name: SWOT Analysis\n\
         description: Strengths, weaknesses, opportunities, and threats\n\
         ---\n\
         Develop a SWOT analysis of the subject of the provided sources. Present four \
         sections — Strengths, Weaknesses, Opportunities, and Threats — as Markdown \
         headings, each with 3-6 bullet points grounded in the material. Close with a \
         short paragraph assessing the overall strategic position.\n",
    ),
    (
        "press-release",
        "---\n\
         name: Press Release\n\
         description: Industry-standard announcement of the subject\n\
         ---\n\
         Write a press release announcing the subject of the provided sources, following \
         industry-standard structure: a compelling headline, a dateline, an opening \
         paragraph that delivers the key news, body paragraphs with supporting details \
         and context, a plausible attributed spokesperson quote, a boilerplate \"About\" \
         paragraph, and a media contact line. Ground every claim in the sources.\n",
    ),
    (
        "meeting-agenda",
        "---\n\
         name: Meeting Agenda\n\
         description: Topics, speakers, and time allocations\n\
         ---\n\
         Draft a meeting agenda for a working session on the subject of the provided \
         sources. State the meeting goal, then list the topics to cover with a suggested \
         owner and a time allocation for each, ordered to build toward decisions. End \
         with the desired outcomes and any pre-reading drawn from the sources.\n",
    ),
    (
        "user-stories",
        "---\n\
         name: User Stories\n\
         description: As a [user], I want [action] so that [benefit]\n\
         ---\n\
         Write a set of user stories for the product or system described in the provided \
         sources. Format each as \"As a [type of user], I want [an action] so that [a \
         benefit]\". Group the stories by user type or theme, cover the main capabilities \
         in the material, and add acceptance-criteria bullets under the most important \
         stories.\n",
    ),
    (
        "sop",
        "---\n\
         name: SOP\n\
         description: Step-by-step procedure with tools and safety\n\
         ---\n\
         Write a Standard Operating Procedure for the process described in the provided \
         sources. Include: purpose and scope, roles and responsibilities, required tools \
         and materials, numbered step-by-step instructions, safety or risk warnings where \
         relevant, and how to verify the procedure succeeded.\n",
    ),
    (
        "memo",
        "---\n\
         name: Memo\n\
         description: Standard To/From/Date/Subject memo\n\
         ---\n\
         Write a standard memo about the subject of the provided sources, with To, From, \
         Date, and Subject lines (use sensible placeholders where the sources don't say). \
         Open with the purpose in the first sentence, present the key points as short \
         paragraphs or bullets, and close with any actions requested.\n",
    ),
    (
        "blog-post",
        "---\n\
         name: Blog Post\n\
         description: Intro, main points, and conclusion\n\
         ---\n\
         Write a detailed blog post about the subject of the provided sources: an \
         engaging introduction that hooks the reader, well-organized main points under \
         clear subheadings, concrete examples drawn from the material, and a conclusion \
         that summarizes the takeaways and ends with a thought or call to action.\n",
    ),
    (
        "tech-spec",
        "---\n\
         name: Tech Spec\n\
         description: Architecture, data models, and algorithms\n\
         ---\n\
         Write a technical specification for the system described in the provided \
         sources. Cover: overview and goals, architecture (the components and how they \
         interact), data models, key algorithms or flows, and external interfaces. Use \
         Markdown headings, keep every detail grounded in the sources, and call out open \
         questions or gaps explicitly.\n",
    ),
    (
        "category-taxonomy",
        "---\n\
         name: Category Taxonomy\n\
         description: Hierarchical categories for the content\n\
         ---\n\
         Organize the content of the provided sources into a hierarchical category \
         taxonomy. Use nested Markdown lists — top-level categories, subcategories, and \
         specific items from the material — with a one-line description of each \
         top-level category. Aim for categories that are mutually exclusive and together \
         cover the whole corpus.\n",
    ),
    (
        "key-entities",
        "---\n\
         name: Key Entities\n\
         description: People, companies, and products mentioned\n\
         ---\n\
         Identify and list all notable entities mentioned in the provided sources — \
         people, companies, organizations, products, technologies, and places. Group \
         them by type under Markdown headings, and give each entity a one-line \
         description of who or what it is and its role in the material.\n",
    ),
    (
        "key-themes",
        "---\n\
         name: Key Themes\n\
         description: Main themes with a rationale for each\n\
         ---\n\
         Identify the key themes running through the provided sources. For each theme, \
         use a short heading naming the theme itself, then a concise rationale (2-3 \
         sentences) explaining it and which sources or passages support it. Order the \
         themes from most to least prominent.\n",
    ),
];

/// First use: create the folder and write the default pack. An existing folder
/// is left entirely alone — user edits and deletions stick.
fn ensure_default_templates(dir: &Path) -> anyhow::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    for (id, contents) in DEFAULT_TEMPLATES {
        let file = dir.join(format!("{id}.md"));
        if !file.exists() {
            std::fs::write(file, contents)?;
        }
    }
    Ok(())
}

/// Tolerant frontmatter parse: an optional `---`-fenced block with `name:` /
/// `description:` lines, then the body as the prompt. No frontmatter → the
/// filename stem is the name and the whole file is the prompt. Files with an
/// empty prompt are skipped (None).
fn parse_template(stem: &str, text: &str) -> Option<Template> {
    let mut name = String::new();
    let mut description = String::new();
    let mut body = text.trim();
    if let Some((front, rest)) = body.strip_prefix("---").and_then(|r| r.split_once("\n---")) {
        for line in front.lines() {
            if let Some(v) = line.strip_prefix("name:") {
                name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("description:") {
                description = v.trim().to_string();
            }
        }
        body = rest.trim();
    }
    if body.is_empty() {
        return None;
    }
    if name.is_empty() {
        name = stem.to_string();
    }
    Some(Template {
        id: stem.to_string(),
        name,
        description,
        prompt: body.to_string(),
    })
}

/// Read every parseable template in `dir`, sorted by name.
fn read_templates(dir: &Path) -> anyhow::Result<Vec<Template>> {
    let mut templates = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Unreadable or empty files are skipped, not fatal — one bad template
        // shouldn't hide the rest.
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(t) = parse_template(stem, &text) {
            templates.push(t);
        }
    }
    templates.sort_by_key(|t| t.name.to_lowercase());
    Ok(templates)
}

#[tauri::command]
pub fn list_templates() -> Result<Vec<Template>, String> {
    let dir = templates_dir().ok_or_else(|| "Could not resolve the home directory".to_string())?;
    ensure_default_templates(&dir).map_err(|e| format!("{e:#}"))?;
    read_templates(&dir).map_err(|e| format!("{e:#}"))
}

/// Open the templates folder in Finder so the user can add or edit templates.
#[tauri::command]
pub fn open_templates_folder() -> Result<(), String> {
    let dir = templates_dir().ok_or_else(|| "Could not resolve the home directory".to_string())?;
    ensure_default_templates(&dir).map_err(|e| format!("{e:#}"))?;
    std::process::Command::new("open")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alchemy-tmpl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn frontmatter_parses_name_description_and_body() {
        let t = parse_template(
            "swot",
            "---\nname: SWOT Analysis\ndescription: Four quadrants\n---\nDevelop a SWOT analysis.",
        )
        .unwrap();
        assert_eq!(t.id, "swot");
        assert_eq!(t.name, "SWOT Analysis");
        assert_eq!(t.description, "Four quadrants");
        assert_eq!(t.prompt, "Develop a SWOT analysis.");
    }

    #[test]
    fn missing_frontmatter_falls_back_to_stem() {
        let t = parse_template("my-generator", "Just do the thing.").unwrap();
        assert_eq!(t.name, "my-generator");
        assert_eq!(t.description, "");
        assert_eq!(t.prompt, "Just do the thing.");
    }

    #[test]
    fn empty_files_are_skipped() {
        assert!(parse_template("empty", "").is_none());
        assert!(parse_template("only-front", "---\nname: X\n---\n\n").is_none());
    }

    /// The whole default pack must survive a round-trip through the parser —
    /// a malformed pack entry would silently vanish from the Studio panel.
    #[test]
    fn default_pack_writes_once_and_parses() {
        let root = tmp_dir();
        let dir = root.join("templates");
        ensure_default_templates(&dir).unwrap();
        let templates = read_templates(&dir).unwrap();
        assert_eq!(templates.len(), DEFAULT_TEMPLATES.len());
        assert!(templates.iter().all(|t| !t.prompt.is_empty()));
        assert!(templates.iter().any(|t| t.name == "SWOT Analysis"));

        // An existing folder is never touched: deletions and edits stick.
        std::fs::remove_file(dir.join("memo.md")).unwrap();
        std::fs::write(dir.join("sop.md"), "My own SOP instruction.").unwrap();
        ensure_default_templates(&dir).unwrap();
        assert!(!dir.join("memo.md").exists());
        let sop = read_templates(&dir)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "sop")
            .unwrap();
        assert_eq!(sop.prompt, "My own SOP instruction.");
        let _ = std::fs::remove_dir_all(root);
    }
}
