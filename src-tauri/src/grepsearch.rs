//! Query-time exact-match retrieval leg (docs/RFC-git-sources.md §6):
//! when a chat query carries code-shaped tokens, ripgrep's engine runs over
//! the notebook's repo-backed files and the matching line windows join the
//! citation fusion. No index, no staleness, no embedding cost — the
//! mechanism coding agents ride to ~90% of embedding-RAG parity, fused into
//! notebook retrieval instead of an agent loop.

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::SearcherBuilder;

/// Most tokens to search, best windows to return, context lines around a
/// matched line, and per-file match cap (a hot identifier shouldn't turn one
/// file into the whole result set).
const MAX_TOKENS: usize = 6;
const CONTEXT_LINES: usize = 3;
const MAX_LINES_PER_FILE: usize = 8;

/// Pull code-shaped tokens out of a chat query: identifiers with internal
/// structure (snake_case, camelCase, dotted.paths, colon::paths), and
/// backtick- or quote-wrapped literals. Plain prose words never qualify —
/// a query with no code shape skips the grep leg entirely.
pub fn code_tokens(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: &str| {
        let t = t.trim();
        if t.len() >= 3 && !out.iter().any(|x| x == t) && out.len() < MAX_TOKENS {
            out.push(t.to_string());
        }
    };

    // Quoted/backticked spans are explicit "this exact thing" signals.
    for quote in ['`', '"', '\''] {
        let mut parts = query.split(quote);
        parts.next();
        while let (Some(inner), Some(_)) = (parts.next(), parts.next()) {
            if !inner.trim().is_empty() && inner.len() <= 80 && !inner.contains('\n') {
                push(inner);
            }
        }
    }

    for raw in query.split_whitespace() {
        let word = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if word.len() < 4 {
            continue;
        }
        let interior = &word[1..word.len().saturating_sub(1)];
        let snake = interior.contains('_');
        let dotted = raw.contains("::")
            || (word.contains('.')
                && word
                    .split('.')
                    .all(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')));
        let camel = word
            .as_bytes()
            .windows(2)
            .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
        if snake || camel {
            push(word);
        } else if dotted {
            push(
                raw.trim_matches(|c: char| {
                    !c.is_alphanumeric() && c != '_' && c != '.' && c != ':'
                }),
            );
        }
    }
    out
}

/// One ranked exact-match hit: a window of real lines around the matches.
/// Hits arrive ranked by match count — the order is the score.
pub struct GrepHit {
    /// Index into the `files` slice handed to `search_files`.
    pub file_index: usize,
    pub first_line: u64,
    pub window: String,
}

/// Search the token set across the given files (already size-capped at
/// ingest time) and return the best windows, ranked by match count. Purely
/// synchronous — callers wrap in spawn_blocking.
pub fn search_files(tokens: &[String], files: &[String], max_hits: usize) -> Vec<GrepHit> {
    if tokens.is_empty() || files.is_empty() {
        return Vec::new();
    }
    let pattern = tokens
        .iter()
        .map(|t| regex_escape(t))
        .collect::<Vec<_>>()
        .join("|");
    search_pattern(&pattern, files, max_hits).unwrap_or_default()
}

/// Raw-regex variant for the MCP `grep_sources` tool: the agent supplies the
/// pattern; an invalid one comes back as the error message.
pub fn search_pattern(
    pattern: &str,
    files: &[String],
    max_hits: usize,
) -> Result<Vec<GrepHit>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let matcher = RegexMatcherBuilder::new()
        .build(pattern)
        .map_err(|e| format!("invalid pattern: {e}"))?;
    let mut searcher = SearcherBuilder::new().line_number(true).build();

    let mut scored: Vec<(usize, Vec<u64>)> = Vec::new();
    for (i, path) in files.iter().enumerate() {
        let mut lines: Vec<u64> = Vec::new();
        let sink = UTF8(|line_num, _line| {
            lines.push(line_num);
            Ok(lines.len() < MAX_LINES_PER_FILE)
        });
        if searcher.search_path(&matcher, path, sink).is_err() {
            continue;
        }
        if !lines.is_empty() {
            scored.push((i, lines));
        }
    }
    scored.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));

    Ok(scored
        .into_iter()
        .take(max_hits)
        .filter_map(|(i, lines)| {
            let window = window_around(&files[i], &lines)?;
            Some(GrepHit {
                file_index: i,
                first_line: lines[0],
                window,
            })
        })
        .collect())
}

/// Merge the matched line numbers into contiguous ranges (± context) and
/// join the file's real lines — whitespace intact, like every code surface.
fn window_around(path: &str, matched: &[u64]) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &m in matched {
        let m = (m as usize).saturating_sub(1); // 1-based → 0-based
        let start = m.saturating_sub(CONTEXT_LINES);
        let end = (m + CONTEXT_LINES + 1).min(lines.len());
        match ranges.last_mut() {
            Some((_, prev_end)) if start <= *prev_end => *prev_end = end,
            _ => ranges.push((start, end)),
        }
    }
    let mut out = String::new();
    for (idx, (start, end)) in ranges.iter().enumerate() {
        if idx > 0 {
            out.push_str("\n⋮\n");
        }
        out.push_str(&lines[*start..*end].join("\n"));
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Escape a literal for the regex alternation (grep-regex has no
/// fixed-strings builder switch; escaping is equivalent).
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if !c.is_alphanumeric() && c != '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_tokens_pick_identifiers_not_prose() {
        let t = code_tokens("how does search_chunks_trace fuse the fts side?");
        assert_eq!(t, vec!["search_chunks_trace".to_string()]);
        let t = code_tokens("where is RegexMatcherBuilder used with lib.rs?");
        assert!(t.contains(&"RegexMatcherBuilder".to_string()));
        assert!(t.contains(&"lib.rs".to_string()));
        let t = code_tokens("explain the `content_stamp` trick");
        assert_eq!(t[0], "content_stamp");
        assert!(code_tokens("what does this repository actually do overall").is_empty());
        assert!(code_tokens("summarize the design and its tradeoffs").is_empty());
    }

    #[test]
    fn search_files_windows_and_ranks() {
        let dir = std::env::temp_dir().join(format!("alch-grep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        std::fs::write(&a, "fn one() {}\nfn special_fn() {\n    body();\n}\n").unwrap();
        std::fs::write(&b, "// nothing here\nlet x = 1;\n").unwrap();
        let files = vec![
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        ];
        let hits = search_files(&["special_fn".to_string()], &files, 4);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_index, 0);
        assert_eq!(hits[0].first_line, 2);
        assert!(hits[0].window.contains("    body();"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
