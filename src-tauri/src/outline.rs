//! Per-file top-level symbol outlines for the repo map (RFC-git-sources §5):
//! ast-grep's bundled tree-sitter grammars parse each code file and the
//! top-level definition names land on the file's line in the map tree — so
//! `search_chunks_trace` is retrievable by BM25 and vector even when the
//! file itself is grep-tier and never embedded. Top-level only; nested
//! granularity waits for retrieval traces to ask for it.

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::SupportLang;
use std::path::Path;

/// Most symbols carried per file line; the rest collapse into "+N".
const MAX_SYMBOLS: usize = 8;

/// A node kind counts as a definition when it contains one of these. Kind
/// names are grammar-specific ("function_item", "class_definition",
/// "method_declaration"…) but converge on this vocabulary, which is what
/// makes one generic walk work across all bundled grammars.
const KIND_HINTS: &[&str] = &[
    "function",
    "method",
    "class",
    "struct",
    "enum",
    "trait",
    "impl",
    "interface",
    "type_",
    "module",
    "mod_item",
];

/// Outline a file's top-level symbols. Empty when the language has no
/// bundled grammar (grep and chunking still cover it) or nothing matched.
pub fn outline(path: &str, source: &str) -> Vec<String> {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if name == "dockerfile" {
        return dockerfile_outline(source);
    }
    let Some(lang) = lang_for(path) else {
        return Vec::new();
    };
    let grep = lang.ast_grep(source);
    let root = grep.root();
    let mut out = Vec::new();
    collect(&root, lang, 1, &mut out);
    out
}

/// Wrapper kinds worth descending one level through: `export function f()`
/// parses as export_statement→function_declaration, Python decorators wrap
/// their definition, and HCL's blocks live under a `body` node.
const WRAPPER_KINDS: &[&str] = &["export_statement", "decorated_definition", "body"];

fn collect<D: ast_grep_core::Doc>(
    node: &ast_grep_core::Node<D>,
    lang: SupportLang,
    depth: usize,
    out: &mut Vec<String>,
) {
    for child in node.children() {
        if !child.is_named() {
            continue;
        }
        let kind = child.kind();
        // HCL's blocks (resource/module/provider …) — Terraform repos read
        // beautifully with their labels inlined.
        let is_def = KIND_HINTS.iter().any(|h| kind.contains(h))
            || (matches!(lang, SupportLang::Hcl) && kind.as_ref() == "block");
        if is_def {
            if let Some(sym) = symbol_name(&child) {
                if !out.contains(&sym) {
                    out.push(sym);
                }
            }
            continue;
        }
        if depth > 0 && WRAPPER_KINDS.contains(&kind.as_ref()) {
            collect(&child, lang, depth - 1, out);
        }
    }
}

/// Render an outline as the map-tree suffix: ` — a, b, c (+4)`.
pub fn suffix(symbols: &[String]) -> String {
    if symbols.is_empty() {
        return String::new();
    }
    let shown: Vec<&str> = symbols
        .iter()
        .take(MAX_SYMBOLS)
        .map(|s| s.as_str())
        .collect();
    let mut out = format!(" — {}", shown.join(", "));
    if symbols.len() > MAX_SYMBOLS {
        out.push_str(&format!(" (+{})", symbols.len() - MAX_SYMBOLS));
    }
    out
}

fn symbol_name<D: ast_grep_core::Doc>(node: &ast_grep_core::Node<D>) -> Option<String> {
    // Grammars mostly expose the identifier as the `name` field; Rust impl
    // blocks use `type`. Fall back to the signature line, trimmed.
    for field in ["name", "type"] {
        if let Some(n) = node.field(field) {
            let t = n.text().to_string();
            if !t.trim().is_empty() && t.len() <= 80 {
                return Some(t.trim().to_string());
            }
        }
    }
    let first_line = node.text().lines().next().unwrap_or("").trim().to_string();
    let cleaned = first_line
        .trim_end_matches('{')
        .trim_end_matches("=>")
        .trim()
        .to_string();
    (!cleaned.is_empty() && cleaned.len() <= 80).then_some(cleaned)
}

pub(crate) fn lang_for(path: &str) -> Option<SupportLang> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    Some(match ext.as_str() {
        "rs" => SupportLang::Rust,
        "ts" => SupportLang::TypeScript,
        "tsx" => SupportLang::Tsx,
        "js" | "jsx" | "mjs" | "cjs" => SupportLang::JavaScript,
        "py" => SupportLang::Python,
        "go" => SupportLang::Go,
        "rb" => SupportLang::Ruby,
        "java" => SupportLang::Java,
        "swift" => SupportLang::Swift,
        "c" | "h" => SupportLang::C,
        "cc" | "cpp" | "hpp" | "hh" => SupportLang::Cpp,
        "php" => SupportLang::Php,
        "html" | "htm" => SupportLang::Html,
        "sh" | "bash" | "zsh" => SupportLang::Bash,
        "hcl" | "tf" | "tfvars" => SupportLang::Hcl,
        _ => return None,
    })
}

/// One structural-search hit for the MCP `ast_search` tool.
pub struct AstHit {
    pub file_index: usize,
    pub line: usize,
    pub text: String,
}

/// Run an ast-grep structural pattern over the files. The pattern compiles
/// per language (a Rust pattern won't parse as Python — those files are
/// skipped rather than erroring the whole search).
pub fn ast_search_files(pattern: &str, files: &[String], max_hits: usize) -> Vec<AstHit> {
    use ast_grep_core::matcher::Pattern;
    use std::collections::HashMap;
    let mut compiled: HashMap<SupportLang, Option<Pattern>> = HashMap::new();
    let mut out = Vec::new();
    for (i, path) in files.iter().enumerate() {
        if out.len() >= max_hits {
            break;
        }
        let Some(lang) = lang_for(path) else { continue };
        let pat = compiled
            .entry(lang)
            .or_insert_with(|| Pattern::try_new(pattern, lang).ok());
        let Some(pat) = pat.as_ref() else { continue };
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let grep = lang.ast_grep(&src);
        let root = grep.root();
        for m in root.find_all(pat) {
            let node = m.get_node();
            let text: String = node.text().lines().take(12).collect::<Vec<_>>().join("\n");
            out.push(AstHit {
                file_index: i,
                line: node.start_pos().line() + 1,
                text,
            });
            if out.len() >= max_hits {
                break;
            }
        }
    }
    out
}

/// Dockerfiles have no bundled grammar; build stages are the symbols worth
/// naming and a line scan gets them exactly.
fn dockerfile_outline(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l
                .strip_prefix("FROM ")
                .or_else(|| l.strip_prefix("from "))?;
            let mut parts = rest.split_whitespace();
            let image = parts.next()?;
            match (parts.next(), parts.next()) {
                (Some(as_kw), Some(stage)) if as_kw.eq_ignore_ascii_case("as") => {
                    Some(format!("stage {stage}"))
                }
                _ => Some(format!("FROM {image}")),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_outline_names_top_level_items() {
        let src = "pub struct Db { x: i64 }\n\npub fn search_chunks_trace(q: &str) {}\n\nimpl Db {\n    fn inner(&self) {}\n}\n";
        let syms = outline("db.rs", src);
        assert!(syms.contains(&"Db".to_string()), "{syms:?}");
        assert!(
            syms.contains(&"search_chunks_trace".to_string()),
            "{syms:?}"
        );
        // Top-level only: impl methods stay out.
        assert!(!syms.iter().any(|s| s.contains("inner")), "{syms:?}");
    }

    #[test]
    fn python_and_typescript_outline() {
        let syms = outline(
            "m.py",
            "class Loader:\n    pass\n\ndef fetch_all():\n    pass\n",
        );
        assert!(syms.contains(&"Loader".to_string()), "{syms:?}");
        assert!(syms.contains(&"fetch_all".to_string()), "{syms:?}");
        let syms = outline(
            "c.ts",
            "export interface Props { a: string }\nexport function render(p: Props) {}\n",
        );
        assert!(syms.contains(&"Props".to_string()), "{syms:?}");
        assert!(syms.contains(&"render".to_string()), "{syms:?}");
    }

    #[test]
    fn hcl_blocks_and_dockerfile_stages() {
        let syms = outline(
            "main.tf",
            "resource \"aws_s3_bucket\" \"assets\" {\n  bucket = \"x\"\n}\n\nmodule \"vpc\" {\n  source = \"./vpc\"\n}\n",
        );
        assert!(!syms.is_empty(), "{syms:?}");
        assert!(syms[0].contains("aws_s3_bucket"), "{syms:?}");
        let syms = outline(
            "Dockerfile",
            "FROM rust:1.79 AS builder\nRUN cargo build\nFROM debian:slim\n",
        );
        assert_eq!(
            syms,
            vec!["stage builder".to_string(), "FROM debian:slim".to_string()]
        );
    }

    #[test]
    fn ast_search_matches_structural_patterns() {
        let dir = std::env::temp_dir().join(format!("alch-ast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.rs");
        std::fs::write(
            &f,
            "fn alpha() { beta().unwrap(); }\nfn beta() -> Option<u8> { None }\n",
        )
        .unwrap();
        let files = vec![f.to_string_lossy().to_string()];
        let hits = ast_search_files("$X.unwrap()", &files, 10);
        assert_eq!(hits.len(), 1, "unwrap-call pattern");
        assert!(hits[0].text.contains("beta().unwrap()"), "{}", hits[0].text);
        assert_eq!(hits[0].line, 1);
        let hits = ast_search_files("fn $NAME($$$) { $$$ }", &files, 10);
        assert!(!hits.is_empty(), "fn pattern should match");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_language_is_quietly_empty() {
        assert!(outline("data.csv", "a,b,c").is_empty());
        assert!(outline("weird.xyz", "???").is_empty());
    }

    #[test]
    fn suffix_caps_and_counts() {
        let syms: Vec<String> = (0..11).map(|i| format!("f{i}")).collect();
        let s = suffix(&syms);
        assert!(s.starts_with(" — f0, f1"), "{s}");
        assert!(s.ends_with("(+3)"), "{s}");
        assert_eq!(suffix(&[]), "");
    }
}
