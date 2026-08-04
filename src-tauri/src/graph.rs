//! The notebook link graph (docs/RFC-document-surface.md phase 5).
//!
//! `source_backlinks` answers "what links HERE?" by scanning every document
//! in the notebook once per open. That is fine for a footer on one source and
//! quadratic for a graph, which needs the answer for every node at once. This
//! module does the whole notebook in a single pass instead: build one index
//! of what each document could be referred to BY, then scan each document's
//! text once against it.
//!
//! "Once" is load-bearing. The obvious version — for each document, for each
//! needle, `content.contains(needle)` — is O(documents x needles) full-text
//! scans, and a real notebook (330 documents, 660 needles, tens of KB each)
//! spends seconds there and pins a core while the pane sits empty. An
//! Aho-Corasick automaton finds every needle in one pass per document
//! instead, which is the difference between a visible hang and a blink.
//!
//! Edges are found the three ways documents actually refer to each other:
//! an absolute URL, a bare filename (how a relative link in a sibling
//! document points at a file source), and an Obsidian `[[wikilink]]` naming a
//! title. All three already exist in the corpus — nothing here asks the user
//! to link anything a new way.

use std::collections::{HashMap, HashSet};

use aho_corasick::AhoCorasick;
use serde::Serialize;

/// One document in the graph. `kind` is "source" or "note"; `sourceType`
/// carries the source's own type so the view can shape nodes the way the
/// gallery shapes cards.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub source_type: String,
    /// Outbound + inbound edge count, so the view can size nodes by
    /// connectedness without recomputing it.
    pub degree: usize,
}

/// A directed reference from one document to another.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotebookGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// A document as the graph builder needs it — decoupled from `Source`/`Note`
/// so this is testable without a database.
pub struct GraphDoc {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub source_type: String,
    /// A source's URL or file path; empty for notes.
    pub url: String,
    pub content: String,
}

/// A filename is only a useful needle if it is distinctive. "a.md" appears
/// inside other words; a name this long effectively does not collide.
const MIN_FILENAME_LEN: usize = 6;
/// Same reasoning for titles matched via wikilinks.
const MIN_TITLE_LEN: usize = 3;

/// Build the link graph for a set of documents.
pub fn build(docs: &[GraphDoc]) -> NotebookGraph {
    // What can each document be referred to by? Longest needles first, so a
    // full URL wins over the filename inside it.
    let mut by_needle: Vec<(String, &str)> = Vec::new();
    // Wikilink targets resolve on title, case-insensitively.
    let mut by_title: HashMap<String, &str> = HashMap::new();

    for doc in docs {
        if !doc.url.is_empty() {
            by_needle.push((doc.url.clone(), doc.id.as_str()));
            let is_path = !doc.url.starts_with("http") && !doc.url.contains("://");
            if is_path {
                if let Some(name) = doc.url.rsplit('/').next() {
                    if name.len() >= MIN_FILENAME_LEN {
                        by_needle.push((name.to_string(), doc.id.as_str()));
                    }
                }
            }
        }
        let title = doc.title.trim().to_lowercase();
        if title.chars().count() >= MIN_TITLE_LEN {
            // First writer wins: two documents sharing a title is ambiguous,
            // and silently pointing at the second is worse than at the first.
            by_title.entry(title).or_insert(doc.id.as_str());
        }
    }
    by_needle.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));

    // One automaton over every needle. Overlapping matches on purpose: a
    // document's URL and its bare filename are both needles and one contains
    // the other, and a leftmost-longest scan would report only the URL —
    // fine here since both point at the same document, but not something to
    // rely on when two different documents' needles overlap in the text.
    let automaton = if by_needle.is_empty() {
        None
    } else {
        AhoCorasick::new(by_needle.iter().map(|(needle, _)| needle.as_str())).ok()
    };

    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut degree: HashMap<&str, usize> = HashMap::new();

    for doc in docs {
        // Folder-ish parents are containers, not authors of references; their
        // "content" is a file map that would link them to everything.
        if matches!(
            doc.source_type.as_str(),
            "folder" | "git" | "notion" | "obsidian"
        ) {
            continue;
        }
        let mut targets: HashSet<&str> = HashSet::new();
        if let Some(ac) = automaton.as_ref() {
            for hit in ac.find_overlapping_iter(&doc.content) {
                let target_id = by_needle[hit.pattern().as_usize()].1;
                if target_id != doc.id.as_str() {
                    targets.insert(target_id);
                }
            }
        }
        for name in wikilink_targets(&doc.content) {
            if let Some(target_id) = by_title.get(&name) {
                if *target_id != doc.id.as_str() {
                    targets.insert(target_id);
                }
            }
        }
        for target in targets {
            let key = (doc.id.clone(), target.to_string());
            if seen.insert(key) {
                edges.push(GraphEdge {
                    from: doc.id.clone(),
                    to: target.to_string(),
                });
                *degree.entry(doc.id.as_str()).or_default() += 1;
                *degree.entry(target).or_default() += 1;
            }
        }
    }

    let nodes = docs
        .iter()
        .map(|d| GraphNode {
            id: d.id.clone(),
            kind: d.kind.clone(),
            title: d.title.clone(),
            source_type: d.source_type.clone(),
            degree: degree.get(d.id.as_str()).copied().unwrap_or(0),
        })
        .collect();

    NotebookGraph { nodes, edges }
}

/// The `[[targets]]` a document names, lowercased, with `|aliases` and
/// `#headings` stripped — the same shape `debracket_wikilinks` reads.
fn wikilink_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        // `[[Page|alias]]` targets Page; `[[Page#Section]]` targets Page.
        let target = inner
            .split('|')
            .next()
            .unwrap_or(inner)
            .split('#')
            .next()
            .unwrap_or(inner)
            .trim()
            .to_lowercase();
        if !target.is_empty() {
            out.push(target);
        }
        rest = &after[end + 2..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, title: &str, url: &str, content: &str) -> GraphDoc {
        GraphDoc {
            id: id.into(),
            kind: "source".into(),
            title: title.into(),
            source_type: "markdown".into(),
            url: url.into(),
            content: content.into(),
        }
    }

    #[test]
    fn links_by_url_filename_and_wikilink() {
        let docs = vec![
            doc(
                "a",
                "Alpha",
                "/notes/alpha-paper.md",
                "see [[Beta]] and https://ex.com/g",
            ),
            doc(
                "b",
                "Beta",
                "/notes/beta-paper.md",
                "back to alpha-paper.md",
            ),
            doc("g", "Gamma", "https://ex.com/g", "unrelated"),
        ];
        let g = build(&docs);
        let has = |f: &str, t: &str| {
            g.edges.contains(&GraphEdge {
                from: f.into(),
                to: t.into(),
            })
        };
        assert!(has("a", "b"), "wikilink [[Beta]] -> Beta");
        assert!(has("a", "g"), "absolute URL -> Gamma");
        assert!(has("b", "a"), "bare filename -> Alpha");
        assert_eq!(g.edges.len(), 3, "no phantom edges: {:?}", g.edges);
    }

    /// Degree counts both directions, so the view can size by connectedness.
    #[test]
    fn degree_counts_both_ends() {
        let docs = vec![
            doc("a", "Alpha", "", "[[Beta]]"),
            doc("b", "Beta", "", "nothing here"),
        ];
        let g = build(&docs);
        let deg = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().degree;
        assert_eq!(deg("a"), 1);
        assert_eq!(deg("b"), 1);
    }

    /// A document quoting its own filename or title must not link to itself —
    /// self-loops are noise in a graph and every document names itself.
    #[test]
    fn no_self_edges() {
        let docs = vec![doc(
            "a",
            "Alpha",
            "/notes/alpha-paper.md",
            "this file is alpha-paper.md, see [[Alpha]]",
        )];
        assert!(build(&docs).edges.is_empty());
    }

    /// Folder parents hold a map of every child path; treating that as
    /// authorship would wire the folder to the whole notebook.
    #[test]
    fn folder_parents_do_not_author_edges() {
        let mut folder = doc("f", "Repo", "/repo", "/repo/alpha-paper.md\n/repo/beta.md");
        folder.source_type = "folder".into();
        let docs = vec![folder, doc("a", "Alpha", "/repo/alpha-paper.md", "")];
        assert!(build(&docs).edges.is_empty());
    }

    /// Short filenames and titles are not distinctive enough to match on.
    #[test]
    fn short_needles_are_ignored() {
        let docs = vec![
            doc("a", "Al", "/n/a.md", "mentions a.md and [[Al]]"),
            doc("b", "Beta", "/n/beta-paper.md", ""),
        ];
        assert!(build(&docs).edges.is_empty());
    }

    #[test]
    fn wikilink_targets_strip_alias_and_heading() {
        assert_eq!(
            wikilink_targets("[[Page|alias]] [[Other#Section]] [[ Spaced ]]"),
            vec!["page", "other", "spaced"]
        );
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    /// A notebook the size of Alchemy Development: hundreds of documents,
    /// each tens of KB, all cross-referencing. This is the shape that made
    /// the graph pane hang, so it is the shape worth timing.
    #[test]
    #[ignore = "timing, not correctness — run with --ignored"]
    fn large_notebook_builds_quickly() {
        const N: usize = 330;
        let filler = "lorem ipsum dolor sit amet consectetur ".repeat(500);
        let docs: Vec<GraphDoc> = (0..N)
            .map(|i| GraphDoc {
                id: format!("id{i}"),
                kind: "source".into(),
                title: format!("Document Number {i}"),
                source_type: "markdown".into(),
                url: format!("/notes/document-number-{i}.md"),
                content: format!(
                    "{filler}\nsee document-number-{}.md and [[Document Number {}]]\n{filler}",
                    (i + 1) % N,
                    (i + 7) % N
                ),
            })
            .collect();
        let bytes: usize = docs.iter().map(|d| d.content.len()).sum();
        let start = std::time::Instant::now();
        let g = build(&docs);
        let ms = start.elapsed().as_millis();
        println!(
            "{N} docs, {}KB total, {} edges in {ms}ms",
            bytes / 1024,
            g.edges.len()
        );
        assert!(
            g.edges.len() >= N,
            "expected the seeded links: {}",
            g.edges.len()
        );
        assert!(ms < 2000, "graph build took {ms}ms — the pane will hang");
    }
}
