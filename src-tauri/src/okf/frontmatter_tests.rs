//! One frontmatter block per file, merged, latest record wins
//! (docs/RFC-okf-live.md §5.3). Before 0.59 a source whose file was its own
//! concept file was re-read whole on every resync, and every write stacked
//! a fresh block on the last — one document in the field reached 312
//! blocks, each block's description quoting the block before it.
use super::sync_tests::Lab;
use super::*;

const BODY: &str = "# Architecture\n\nAlchemy is a local-first research notebook.\n\n## Data flow\n\nimport → extract → chunk → embed.";

/// One source concept the way `source_concept` shapes it, over `content`.
fn architecture(content: &str) -> OkfConcept {
    OkfConcept {
        id: "f2a95179".into(),
        title: "Architecture".into(),
        content: content.to_string(),
        type_label: "Source".into(),
        resource: "file:///Users/paul/notebooklm-local/docs/ARCHITECTURE.md".into(),
        tags: vec!["markdown".into()],
        generated_at: 1_788_000_000_000,
        edited_at: 1_788_000_000_000,
        alchemy: vec![
            ("id".into(), "f2a95179".into()),
            ("source_type".into(), "markdown".into()),
            ("tags".into(), "shell rust".into()),
            ("device".into(), "MacBook Pro".into()),
        ],
        ..OkfConcept::blank()
    }
}

fn notebook() -> OkfNotebook {
    OkfNotebook {
        id: "nb".into(),
        title: "Alchemy Development".into(),
        color: String::new(),
        icon: String::new(),
        generated_at: 1,
    }
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("alchemy-fm-{tag}-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn alchemy_blocks(text: &str) -> usize {
    text.matches("\nalchemy:\n").count()
}

/// One pass of the loop as it ran: the file's text taken back in as the
/// row's content — through the file reader, which trimmed every line — then
/// written again with a block composed over it and a description cut from
/// the head of that content.
fn stack_once(content: &str) -> String {
    let flattened: String = content.lines().map(|l| format!("{}\n", l.trim())).collect();
    let concept = architecture(&flattened);
    let description = okf_description(&flattened);
    format!(
        "{}{flattened}",
        okf_frontmatter(&concept, &description, &HashMap::new())
    )
}

// (1) A document's own keys and Alchemy's share one block, and a round
// trip through the file keeps both — stable from the second pass on.
#[test]
fn a_round_trip_keeps_the_authors_keys_and_ours_in_one_block() {
    let root = scratch("round-trip");
    let authored = format!(
        "---\nauthor: Paul\naliases: [arch, architecture]\ntags: [design, gear]\n---\n\n{BODY}"
    );

    let first = root.join("first");
    write_bundle(&notebook(), &[architecture(&authored)], &[], &first, None).unwrap();
    let text1 = std::fs::read_to_string(first.join("sources/architecture.md")).unwrap();
    assert_eq!(alchemy_blocks(&text1), 1, "{text1}");
    assert_eq!(text1.matches("\n---\n").count(), 1, "one block:\n{text1}");
    let doc1 = parse_okf_doc(&text1);
    assert_eq!(doc1.str("title").as_deref(), Some("Architecture"));
    assert_eq!(doc1.str("author").as_deref(), Some("Paul"));
    assert_eq!(
        doc1.get("aliases")
            .and_then(|v| v.as_sequence())
            .map(|s| s.len()),
        Some(2)
    );
    // `tags` is shared: Alchemy's type label leads, the author's follow.
    assert_eq!(doc1.tags(), vec!["markdown", "design", "gear"]);
    assert_eq!(doc1.body, format!("{BODY}\n"));
    // The description is cut from the body, never from frontmatter text.
    assert_eq!(
        doc1.str("description").as_deref(),
        Some(okf_description(BODY).as_str())
    );

    // Read back: the author's keys come home as the content's leading block,
    // Alchemy's do not.
    let content2 = doc1.document_text();
    assert!(content2.starts_with("---\nauthor: Paul\n"), "{content2}");
    assert!(
        content2.contains("aliases:\n- arch\n- architecture\n"),
        "{content2}"
    );
    assert!(content2.contains("tags:\n- design\n- gear\n"), "{content2}");
    assert!(!content2.contains("alchemy"), "{content2}");
    assert!(
        content2.ends_with(&format!("---\n\n{BODY}\n")),
        "{content2}"
    );

    // Export again from the read-back text: the same file, byte for byte;
    // and again from that.
    for pass in 2..=3 {
        let dir = root.join(format!("pass-{pass}"));
        write_bundle(&notebook(), &[architecture(&content2)], &[], &dir, None).unwrap();
        let text = std::fs::read_to_string(dir.join("sources/architecture.md")).unwrap();
        assert_eq!(text, text1, "pass {pass}");
        assert_eq!(parse_okf_doc(&text).document_text(), content2);
    }
    let _ = std::fs::remove_dir_all(&root);
}

// (2) The same document key on both sides: the newer record's value wins.
#[test]
fn latest_record_wins_when_a_document_key_differs() {
    let file_read_at = 1_788_000_500_000;
    // A bundle whose file was last read at `file_read_at` carrying the
    // file's keys, then a write from a row edited at `edited_at` whose
    // content block carries the row's.
    let seed = |tag: &str, edited_at: i64| {
        let root = scratch(tag);
        let bundle = root.join("nb");
        let manifest_at = root.join("manifest.json");
        write_bundle(
            &notebook(),
            &[architecture(BODY)],
            &[],
            &bundle,
            Some(&manifest_at),
        )
        .unwrap();
        let mut manifest = load_manifest(&manifest_at);
        let entry = manifest.concepts.get_mut("f2a95179").unwrap();
        entry.file_mtime = file_read_at;
        entry.extra.insert("author".into(), "From the file".into());
        entry.extra.insert("reviewed".into(), "yes".into());
        save_manifest(&manifest_at, &manifest);

        let content = format!("---\nauthor: From the row\nstatus_note: draft\n---\n\n{BODY}");
        let mut concept = architecture(&content);
        concept.edited_at = edited_at;
        write_bundle(&notebook(), &[concept], &[], &bundle, Some(&manifest_at)).unwrap();
        let text = std::fs::read_to_string(bundle.join("sources/architecture.md")).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(alchemy_blocks(&text), 1, "{text}");
        parse_okf_doc(&text)
    };

    // The row was edited after the file was last read: the row's value.
    let doc = seed("row-newer", file_read_at + 1);
    assert_eq!(doc.str("author").as_deref(), Some("From the row"));
    // Keys only one side has are kept from both.
    assert_eq!(doc.str("reviewed").as_deref(), Some("yes"));
    assert_eq!(doc.str("status_note").as_deref(), Some("draft"));

    // The file is newer, or the clocks tie: the file's value (§5.4).
    let doc = seed("file-newer", file_read_at);
    assert_eq!(doc.str("author").as_deref(), Some("From the file"));
    assert_eq!(doc.str("reviewed").as_deref(), Some("yes"));
    assert_eq!(doc.str("status_note").as_deref(), Some("draft"));
}

// (3) The heal: a stack mirroring the field one — nested escaped
// descriptions, inner blocks flattened by the file reader — comes down to
// one block over the body, byte for byte, keeping the newest block's own key.
#[tokio::test]
async fn the_heal_merges_a_stack_down_to_one_block_and_keeps_the_body() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let a = lab.replica("a", &bundle).await;

    // Three passes of the loop over a body that carried an author's key,
    // with the key changed between the oldest and the newest pass.
    let oldest = stack_once(&format!("---\nauthor: Old\nsince: 2024\n---\n\n{BODY}"));
    let middle = stack_once(&oldest);
    let stacked = stack_once(&middle).replacen("timestamp:", "author: Paul\ntimestamp:", 1);
    assert_eq!(alchemy_blocks(&stacked), 3);
    // The field shape: the outer description quotes the block under it,
    // escaped, deeper per pass; the inner blocks lost their indentation.
    assert!(
        stacked.contains("description: \"--- type: Source title: \\\"Architecture\\\" description: \\\"--- type: Source"),
        "{stacked}"
    );
    assert!(stacked.contains("\ngenerated:\nby: "), "{stacked}");

    let single = stack_once(BODY);
    let row = |id: &str, content: &str| Source {
        id: id.into(),
        notebook_id: "shared-notebook".into(),
        title: "Architecture".into(),
        source_type: "markdown".into(),
        url: String::new(),
        origin_device: String::new(),
        remote: false,
        content: content.to_string(),
        char_count: content.chars().count() as i64,
        chunk_count: 0,
        created_at: 1,
        status: "ready".into(),
        error: String::new(),
        parent_id: String::new(),
        mtime: 0,
        author: String::new(),
        image_url: String::new(),
        tags: String::new(),
        note: String::new(),
        fetched_at: 1,
        fetch_failures: 0,
    };
    let obsidian = format!("---\ntitle: Espresso\ntags: [gear]\n---\n\n{BODY}");
    for (id, content) in [
        ("looped", stacked.as_str()),
        ("dragged-in", single.as_str()),
        ("obsidian", obsidian.as_str()),
        ("plain", BODY),
    ] {
        a.db.insert_source(&row(id, content), &[], &[])
            .await
            .unwrap();
    }
    let done = heal_stacked_frontmatter_checked(&a).await.unwrap();
    let healed = format!("---\nauthor: Paul\nsince: 2024\n---\n\n{BODY}\n");
    assert_eq!(
        done,
        FrontmatterHeal {
            sources: 1,
            chars_removed: stacked.chars().count() - healed.chars().count(),
            keys_preserved: 2,
            failed: 0,
        }
    );
    let looped = a.db.get_source("looped").await.unwrap().unwrap();
    // The body after the last block, byte for byte, under the one block:
    // the newest `author`, the older `since`, nothing of ours.
    assert_eq!(looped.content, healed);
    assert_eq!(looped.char_count, healed.chars().count() as i64);
    assert_eq!(looped.title, "Architecture");
    // One block of ours is a file dragged out of a bundle, not the loop;
    // the writer merges it on the way out. An author's block is theirs.
    for (id, content) in [
        ("dragged-in", single.as_str()),
        ("obsidian", obsidian.as_str()),
        ("plain", BODY),
    ] {
        assert_eq!(
            a.db.get_source(id).await.unwrap().unwrap().content,
            content,
            "{id}"
        );
    }
    // Nothing to do the second time.
    assert_eq!(
        heal_stacked_frontmatter_checked(&a).await.unwrap(),
        FrontmatterHeal::default()
    );
    // And the writer puts one block over the healed row, not a second.
    let dir = lab.0.join("after");
    write_bundle(&notebook(), &[architecture(&healed)], &[], &dir, None).unwrap();
    let text = std::fs::read_to_string(dir.join("sources/architecture.md")).unwrap();
    assert_eq!(alchemy_blocks(&text), 1);
    let doc = parse_okf_doc(&text);
    assert_eq!(doc.str("author").as_deref(), Some("Paul"));
    assert_eq!(doc.body, format!("{BODY}\n"));
}

// The read-back rule for text that arrives through the file reader: a
// block of ours comes down to the document's keys; anything else is left.
#[test]
fn read_back_merges_our_blocks_and_leaves_everyone_elses() {
    let ours = stack_once(&format!("---\nauthor: Paul\n---\n\n{BODY}"));
    assert_eq!(
        read_back_text(&ours).as_deref(),
        Some(format!("---\nauthor: Paul\n---\n\n{BODY}\n").as_str())
    );
    // Ours with nothing of the document's: the bare body.
    assert_eq!(
        read_back_text(&stack_once(BODY)).as_deref(),
        Some(format!("{BODY}\n").as_str())
    );
    // A flattened block of ours: its lifted children are not an author's.
    let flat = "---\ntype: Source\ntitle: \"A\"\ngenerated:\nby: \"alchemy/0.58.1\"\nat: \"2026-09-03T16:57:10Z\"\nalchemy:\nid: \"x\"\nsource_type: \"markdown\"\ndevice: \"MacBook Pro\"\norigin: \"file:///x\"\nverified: reviewer\ntimestamp: \"2026-09-03T16:57:10Z\"\n---\n\nBody.";
    assert_eq!(
        read_back_text(flat).as_deref(),
        Some("---\nverified: reviewer\n---\n\nBody.")
    );
    // Not ours: untouched, and said so.
    let obsidian = "---\ntitle: Espresso\ntags: [gear]\n---\n\nBody.";
    assert_eq!(read_back_text(obsidian), None);
    assert_eq!(read_back_text(BODY), None);
    assert_eq!(read_back_text("---\nnot closed"), None);
}

// (4) A horizontal rule in the body is not frontmatter.
#[test]
fn a_horizontal_rule_is_not_a_frontmatter_block() {
    // A rule at the top, then prose, then another rule.
    let ruled = "---\n\nIntro paragraph.\n\n---\n\nMore.";
    assert_eq!(peel_frontmatter(ruled), (vec![], ruled));
    assert_eq!(parse_okf_doc(ruled).body, ruled);
    // A block, then a body that opens with a rule and has another later.
    let doc = "---\nauthor: X\n---\n\n---\n\nIntro.\n\n---\n\nMore.";
    assert_eq!(
        peel_frontmatter(doc),
        (vec!["author: X"], "---\n\nIntro.\n\n---\n\nMore.")
    );
    // A fence that is not a line of its own does not close a block.
    let rule = "---\nalchemy:\n----\nbody";
    assert_eq!(peel_frontmatter(rule), (vec![], rule));
    // A stack of ours over an author's block peels both; the body is the same.
    let theirs = "---\ntitle: Espresso\n---\n\nBody.";
    let stacked = stack_once(theirs);
    let (heads, body) = peel_frontmatter(&stacked);
    assert_eq!(heads.len(), 2);
    assert_eq!(body, "Body.\n");
}
