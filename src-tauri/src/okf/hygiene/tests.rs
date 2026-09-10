use super::*;
use crate::okf::sync_tests::Lab;

const HEADING: &str = "## 2026-09-10 \u{2014} thrashr888";

fn entries(log: &str) -> Vec<&str> {
    log.lines().filter(|l| l.starts_with("- ")).collect()
}

fn set_mtime(path: &Path, ms: i64) {
    std::fs::File::open(path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64)),
        )
        .unwrap();
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("alchemy-hygiene-{tag}-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- log.md -----------------------------------------------------------------

#[test]
fn a_repeat_of_the_last_entry_becomes_a_count_not_a_line() {
    let log = log_with_entry(
        "",
        HEADING,
        "20:31:20Z",
        "2 written — 50 sources.",
        "alchemy/1",
    );
    assert_eq!(
        log,
        format!("# Log\n\n{HEADING}\n\n- 20:31:20Z 2 written — 50 sources. (alchemy/1)\n")
    );
    let log = log_with_entry(
        &log,
        HEADING,
        "20:32:20Z",
        "2 written — 50 sources.",
        "alchemy/1",
    );
    let log = log_with_entry(
        &log,
        HEADING,
        "20:33:20Z",
        "2 written — 50 sources.",
        "alchemy/1",
    );
    assert_eq!(
        entries(&log),
        vec!["- 20:31:20Z 2 written — 50 sources. (alchemy/1) \u{d7}3, last 20:33:20Z"]
    );
    // Different words: a new line. Then the old words again: a new line
    // too, not a count on the one two lines up.
    let log = log_with_entry(
        &log,
        HEADING,
        "20:34:20Z",
        "1 removed — 49 sources.",
        "alchemy/1",
    );
    let log = log_with_entry(
        &log,
        HEADING,
        "20:35:20Z",
        "2 written — 50 sources.",
        "alchemy/1",
    );
    assert_eq!(entries(&log).len(), 3);
    assert!(log.ends_with("- 20:35:20Z 2 written — 50 sources. (alchemy/1)\n"));
}

#[test]
fn the_other_writers_heading_starts_a_fresh_entry() {
    let a = "## 2026-09-10 \u{2014} a";
    let b = "## 2026-09-10 \u{2014} b";
    let log = log_with_entry("", a, "10:00:00Z", "1 written.", "alchemy/1");
    let log = log_with_entry(&log, b, "10:01:00Z", "1 written.", "alchemy/1");
    // b's entry is under b's heading, not counted onto a's line.
    assert_eq!(entries(&log).len(), 2);
    assert!(log.contains(&format!("{b}\n\n- 10:01:00Z 1 written. (alchemy/1)\n")));
    // a's next repeat: the last line is b's, so a starts a line under its
    // own heading rather than counting onto b's.
    let log = log_with_entry(&log, a, "10:02:00Z", "1 written.", "alchemy/1");
    assert_eq!(entries(&log).len(), 3);
}

#[test]
fn the_cap_rolls_the_oldest_entries_off_and_their_empty_headings_with_them() {
    let mut log = String::from("# Log\n\n## 2026-09-01 \u{2014} me\n\n- 01:00:00Z old one (x)\n- 01:00:01Z old two (x)\n\n## 2026-09-02 \u{2014} me\n\nA line somebody else left.\n\n- 02:00:00Z mid (x)\n\n## 2026-09-03 \u{2014} me\n\n- 03:00:00Z new (x)\n");
    let capped = cap_log(&log, 2);
    assert_eq!(
        entries(&capped),
        vec!["- 02:00:00Z mid (x)", "- 03:00:00Z new (x)"]
    );
    assert!(!capped.contains("2026-09-01"), "the emptied day is gone");
    assert!(
        capped.contains("A line somebody else left."),
        "foreign lines are not ours to bound"
    );
    assert!(!capped.contains("\n\n\n"));
    // Under the cap: untouched, byte for byte.
    assert_eq!(cap_log(&log, 4), log);
    // The writer applies the cap as it appends.
    for i in 0..(LOG_CAP + 40) {
        log = log_with_entry(
            &log,
            HEADING,
            &format!("04:{:02}:{:02}Z", i / 60, i % 60),
            &format!("pass {i}"),
            "x",
        );
    }
    assert_eq!(entries(&log).len(), LOG_CAP);
    assert!(log.ends_with(&format!("pass {} (x)\n", LOG_CAP + 39)));
}

/// The 0.56 shape (a fenced block of overruled disk texts, one of them with
/// a rule of its own) followed by the 0.58 shape (marker entries).
fn bloated_log() -> String {
    "# Log\n\n## 2026-09-04 \u{2014} me\n\n- 09:00:00Z Kept the app's newer version of 2 file(s); the disk text follows.\n\n```\nnotes/plan.md\n\nAn older plan.\n\n---\n\nsources/architecture.md\n\nArch v1.\n\n---\n\nStill arch v1.\n``` (alchemy/0.56.2)\n\n## 2026-09-08 \u{2014} me\n\n- 10:00:00Z 3 written — 5 sources, 2 notes. (alchemy/0.58.1)\n\n<!-- alchemy-conflict:aaaa -->\n\n## Recovered local version of sources/architecture.md\n\n[Preserved copy](conflicts/aaaa.md)\n\n---\ntitle: \"Architecture\"\nalchemy:\n  id: \"x\"\n---\n\n# Architecture\n\n## Facts\n\nThe body, with headings of its own.\n\n<!-- alchemy-conflict:bbbb -->\n\n## Recovered remote version of notes/plan.md\n\n[Preserved copy](conflicts/bbbb.md)\n\n---\ntitle: Plan\n---\n\nThe plan.\n\n## 2026-09-09 \u{2014} me\n\n- 11:00:00Z 1 written — 5 sources, 2 notes. (alchemy/0.58.2)\n".to_string()
}

#[test]
fn a_fenced_block_of_old_losers_splits_per_file_and_keeps_a_documents_own_rule() {
    let block = "notes/plan.md\n\nAn older plan.\n\n---\n\nsources/architecture.md\n\nArch v1.\n\n---\n\nStill arch v1.";
    assert_eq!(
        split_old_losers(block),
        vec![
            ("notes/plan.md".to_string(), "An older plan.".to_string()),
            (
                "sources/architecture.md".to_string(),
                "Arch v1.\n\n---\n\nStill arch v1.".to_string()
            ),
        ]
    );
}

#[test]
fn inlined_conflict_entries_collapse_to_one_line_each_and_keep_their_text() {
    let (collapsed, found) = collapse_inlined_conflicts(&bloated_log());
    assert_eq!(
        entries(&collapsed),
        vec![
            "- 09:00:00Z Kept the app's newer version of 2 file(s); the disk text is under conflicts/. (alchemy/0.56.2)",
            "- 10:00:00Z 3 written — 5 sources, 2 notes. (alchemy/0.58.1)",
            "- Losing local version of sources/architecture.md kept in conflicts/aaaa.md",
            "- Losing remote version of notes/plan.md kept in conflicts/bbbb.md",
            "- 11:00:00Z 1 written — 5 sources, 2 notes. (alchemy/0.58.2)",
        ]
    );
    assert!(
        !collapsed.contains("## Facts"),
        "the document's own headings went with its text"
    );
    assert!(!collapsed.contains("```"), "the fenced block went too");
    assert!(
        collapsed.contains("## 2026-09-09 \u{2014} me"),
        "the next writer day is where the entry ends"
    );
    assert_eq!(found.len(), 4);
    // The 0.56 block: one entry per file, the remote side, an id the
    // conflict writer would have minted for the same text.
    assert_eq!(found[0].rel, "notes/plan.md");
    assert_eq!(found[0].side, "remote");
    assert_eq!(found[0].text, "An older plan.");
    assert_eq!(
        found[0].id,
        okf_hash(&serde_json::json!(["notes/plan.md", "remote", "An older plan."]).to_string())
    );
    assert_eq!(found[1].rel, "sources/architecture.md");
    assert_eq!(found[1].text, "Arch v1.\n\n---\n\nStill arch v1.");
    assert_eq!(found[2].id, "aaaa");
    assert_eq!(found[2].side, "local");
    assert_eq!(found[2].rel, "sources/architecture.md");
    assert!(found[2].text.starts_with("---\ntitle: \"Architecture\""));
    assert!(found[2]
        .text
        .ends_with("The body, with headings of its own."));
    assert_eq!(found[3].text, "---\ntitle: Plan\n---\n\nThe plan.");
    // A log without markers is returned as it is.
    let clean = "# Log\n\n- 10:00:00Z x (y)\n";
    assert_eq!(
        collapse_inlined_conflicts(clean),
        (clean.to_string(), vec![])
    );
}

#[tokio::test]
async fn trim_log_puts_a_missing_copy_back_before_its_text_leaves_the_log() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    std::fs::write(bundle.join("log.md"), bloated_log()).unwrap();
    // `aaaa` is under conflicts/ already; the other three only ever lived
    // in the log, and one of them — "An older plan." — is what notes/plan.md
    // says on disk right now, so the log's copy was never the only one.
    std::fs::create_dir_all(bundle.join("conflicts")).unwrap();
    std::fs::create_dir_all(bundle.join("notes")).unwrap();
    std::fs::write(bundle.join("conflicts/aaaa.md"), "# Recovered sync conflict\n\nOriginal document: `sources/architecture.md`\n\nPreserved version: local\n\nwhatever").unwrap();
    std::fs::write(
        bundle.join("notes/plan.md"),
        "---\ntitle: Plan\n---\n\nAn older plan.\n",
    )
    .unwrap();
    let manifest = OkfManifest::default();
    let done = trim_log(&state, &bundle, &manifest).await.unwrap();
    assert_eq!(done.collapsed, 4);
    assert_eq!(done.copies_restored, 2);
    assert_eq!(done.copies_dropped, 1);
    assert!(done.chars_removed > 100);
    let restored = std::fs::read_to_string(bundle.join("conflicts/bbbb.md")).unwrap();
    assert_eq!(
        restored,
        conflict_copy_text(
            "notes/plan.md",
            "remote",
            "---\ntitle: Plan\n---\n\nThe plan."
        )
    );
    let old_arch = okf_hash(
        &serde_json::json!([
            "sources/architecture.md",
            "remote",
            "Arch v1.\n\n---\n\nStill arch v1."
        ])
        .to_string(),
    );
    assert!(bundle.join(format!("conflicts/{old_arch}.md")).exists());
    let copies: Vec<String> = std::fs::read_dir(bundle.join("conflicts"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(copies.len(), 3, "{copies:?}");
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert!(!log.contains("The plan."));
    assert!(!log.contains("An older plan."));
    assert!(log.contains("kept in conflicts/bbbb.md"));
    assert!(log.contains("the disk text is under conflicts/."));
    // Already in shape: nothing to do, nothing rewritten.
    assert_eq!(
        trim_log(&state, &bundle, &manifest).await.unwrap(),
        LogTrim::default()
    );
}

/// The 0.55–0.57 dump with its bullet rolled off: two blocks under one
/// heading, the first closed by the writer, the second by a bare fence
/// at the end of the day. `sources/guide.md` is Markdown with fences of
/// its own — one bare pair followed by prose — and ends with a rule of
/// its own right before the separator.
fn bulletless_dump_log() -> String {
    "# Log\n\n## 2026-09-04 \u{2014} me\n\n```\nsources/gallery-tsx.md\n\nimport { x } from \"y\";\n\nexport const g = 1;\n\n---\n\nsources/guide.md\n\n# Guide\n\n## Seams\n\n```bash\nbrew install foo\n```\n\n```\nsources/<slug>.md   # resource: references/foo.pdf\n```\n\n**Named by the file.** More prose.\n\n- a bullet in the document\n\n---\n\n---\n\nnotes/plan.md\n\nAn older plan.\n``` (alchemy/0.57.0)\n\n```\nindex.md\n\n# Listing\n```\n\n## 2026-09-09 \u{2014} me\n\n- 11:00:00Z 1 written — 5 sources, 2 notes. (alchemy/0.58.2)\n".to_string()
}

const GUIDE_TEXT: &str = "# Guide\n\n## Seams\n\n```bash\nbrew install foo\n```\n\n```\nsources/<slug>.md   # resource: references/foo.pdf\n```\n\n**Named by the file.** More prose.\n\n- a bullet in the document\n\n---";

fn dump_id(rel: &str, text: &str) -> String {
    okf_hash(&serde_json::json!([rel, "remote", text]).to_string())
}

#[test]
fn a_dump_with_no_bullet_collapses_per_file_through_its_own_fences_and_rules() {
    let (collapsed, found) = collapse_inlined_conflicts(&bulletless_dump_log());
    let gallery = "import { x } from \"y\";\n\nexport const g = 1;";
    assert_eq!(
        entries(&collapsed),
        vec![
            format!(
                "- Old copy of sources/gallery-tsx.md ({} chars) put under conflicts/{}.md",
                gallery.chars().count(),
                dump_id("sources/gallery-tsx.md", gallery)
            ),
            format!(
                "- Old copy of sources/guide.md ({} chars) put under conflicts/{}.md",
                GUIDE_TEXT.chars().count(),
                dump_id("sources/guide.md", GUIDE_TEXT)
            ),
            format!(
                "- Old copy of notes/plan.md (14 chars) put under conflicts/{}.md",
                dump_id("notes/plan.md", "An older plan.")
            ),
            format!(
                "- Old copy of index.md (9 chars) put under conflicts/{}.md",
                dump_id("index.md", "# Listing")
            ),
            "- 11:00:00Z 1 written — 5 sources, 2 notes. (alchemy/0.58.2)".to_string(),
        ]
    );
    assert!(!collapsed.contains("```"), "every fence went: {collapsed}");
    assert!(
        !collapsed.contains("## Seams"),
        "the document's headings went"
    );
    assert!(collapsed.contains("## 2026-09-04 \u{2014} me\n\n- Old copy"));
    assert!(collapsed.contains("## 2026-09-09 \u{2014} me"));
    let texts: Vec<(&str, &str, &str)> = found
        .iter()
        .map(|e| (e.rel.as_str(), e.side.as_str(), e.text.as_str()))
        .collect();
    assert_eq!(
        texts,
        vec![
            ("sources/gallery-tsx.md", "remote", gallery),
            ("sources/guide.md", "remote", GUIDE_TEXT),
            ("notes/plan.md", "remote", "An older plan."),
            ("index.md", "remote", "# Listing"),
        ]
    );
    // Once collapsed there is nothing left to recognise.
    assert_eq!(
        collapse_inlined_conflicts(&collapsed),
        (collapsed.clone(), vec![])
    );
    // A bare fence that opens on something other than a path is somebody's
    // note, not a dump, and stays.
    let note = "# Log\n\n## 2026-09-04 \u{2014} me\n\n```\nnot a path\n```\n\n- 11:00:00Z x (y)\n";
    assert_eq!(collapse_inlined_conflicts(note), (note.to_string(), vec![]));
}

#[tokio::test]
async fn trim_log_keeps_a_dumps_only_copies_and_names_the_ones_the_notebook_has() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    std::fs::write(bundle.join("log.md"), bulletless_dump_log()).unwrap();
    // The guide on disk still says what the dump says (under frontmatter);
    // the listing too. The gallery source moved on, and the plan is gone.
    std::fs::create_dir_all(bundle.join("sources")).unwrap();
    std::fs::write(
        bundle.join("sources/guide.md"),
        format!("---\ntitle: Guide\n---\n\n{GUIDE_TEXT}\n"),
    )
    .unwrap();
    std::fs::write(bundle.join("index.md"), "# Listing\n").unwrap();
    std::fs::write(
        bundle.join("sources/gallery-tsx.md"),
        "import { x } from \"y\";\n\nexport const g = 2;\n",
    )
    .unwrap();
    let manifest = OkfManifest::default();
    let done = trim_log(&state, &bundle, &manifest).await.unwrap();
    assert_eq!(done.collapsed, 4);
    assert_eq!(done.copies_restored, 2);
    assert_eq!(done.copies_dropped, 2);
    assert_eq!(done.entries_dropped, 0);
    assert!(done.chars_removed > 0);
    let gallery = "import { x } from \"y\";\n\nexport const g = 1;";
    let gallery_id = dump_id("sources/gallery-tsx.md", gallery);
    let plan_id = dump_id("notes/plan.md", "An older plan.");
    assert_eq!(
        std::fs::read_to_string(bundle.join(format!("conflicts/{gallery_id}.md"))).unwrap(),
        conflict_copy_text("sources/gallery-tsx.md", "remote", gallery)
    );
    assert_eq!(
        std::fs::read_to_string(bundle.join(format!("conflicts/{plan_id}.md"))).unwrap(),
        conflict_copy_text("notes/plan.md", "remote", "An older plan.")
    );
    assert_eq!(
        std::fs::read_dir(bundle.join("conflicts")).unwrap().count(),
        2
    );
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert_eq!(
        entries(&log),
        vec![
            format!(
                "- Old copy of sources/gallery-tsx.md ({} chars) put under conflicts/{gallery_id}.md",
                gallery.chars().count()
            ),
            "- Old copy of sources/guide.md already in the notebook".to_string(),
            format!("- Old copy of notes/plan.md (14 chars) put under conflicts/{plan_id}.md"),
            "- Old copy of index.md already in the notebook".to_string(),
            "- 11:00:00Z 1 written — 5 sources, 2 notes. (alchemy/0.58.2)".to_string(),
        ],
        "{log}"
    );
    assert!(!log.contains("```"));
    // Already in shape: nothing to do, nothing rewritten.
    assert_eq!(
        trim_log(&state, &bundle, &manifest).await.unwrap(),
        LogTrim::default()
    );
    assert_eq!(std::fs::read_to_string(bundle.join("log.md")).unwrap(), log);
}

// ---- conflicts/ -------------------------------------------------------------

fn copy(rel: &str, text: &str, mtime: i64) -> ConflictCopy {
    ConflictCopy {
        path: PathBuf::from(format!(
            "conflicts/{}.md",
            okf_hash(&format!("{rel}{text}{mtime}"))
        )),
        rel: rel.into(),
        text_key: text_key(text),
        mtime,
    }
}

#[test]
fn a_conflict_copy_header_parses_and_its_key_ignores_every_frontmatter_block() {
    let text = "# Recovered sync conflict\n\nOriginal document: `sources/a.md`\n\nPreserved version: remote\n\n---\ntitle: A\nalchemy:\n  id: x\n---\n---\ntitle: A\n---\n\nThe words.\n";
    let parsed = parse_conflict_copy(Path::new("c.md"), text, 5).unwrap();
    assert_eq!(parsed.rel, "sources/a.md");
    assert_eq!(parsed.mtime, 5);
    assert_eq!(parsed.text_key, text_key("The words."));
    assert_eq!(
        parsed.text_key,
        text_key("---\ntitle: other\n---\n\n  The words.\n\n")
    );
    assert!(parse_conflict_copy(Path::new("c.md"), "# Somebody's note\n", 5).is_none());
}

#[test]
fn pruning_clears_redundant_copies_after_the_grace_and_never_the_only_copy() {
    let day = 24 * 60 * 60 * 1000;
    let now = 100 * day;
    let live = "The current text.";
    let mut known: HashMap<String, HashSet<String>> = HashMap::new();
    known.insert("notes/a.md".into(), HashSet::from([text_key(live)]));
    let copies = vec![
        // Redundant and old: goes.
        copy("notes/a.md", live, now - 8 * day),
        // Redundant and recent: waits out the grace.
        copy("notes/a.md", live, now - day),
        // Unique: stays, however old.
        copy("notes/a.md", "Words that are nowhere else.", now - 30 * day),
        // The same unique words in an older copy: redundant against the
        // newer kept one, and old enough to go.
        copy("notes/a.md", "Words that are nowhere else.", now - 40 * day),
        // A path the notebook knows nothing about: unique by definition.
        copy("sources/gone.md", "Only here.", now - 90 * day),
        // Empty is never somebody's text.
        copy("sources/gone.md", "---\ntitle: x\n---\n\n", now - 90 * day),
    ];
    let plan = prune_plan(&copies, &known, now);
    assert_eq!(
        plan.remove,
        vec![
            copies[0].path.clone(),
            copies[3].path.clone(),
            copies[5].path.clone()
        ]
    );
    assert_eq!(plan.unique, vec![copies[2].clone(), copies[4].clone()]);
}

#[test]
fn pruning_keeps_at_most_a_few_redundant_copies_per_path_inside_the_grace() {
    let now = 1_000_000_000_000;
    let mut known: HashMap<String, HashSet<String>> = HashMap::new();
    known.insert("notes/a.md".into(), HashSet::from([text_key("live")]));
    let copies: Vec<ConflictCopy> = (0..6)
        .map(|i| copy("notes/a.md", "live", now - i * 1000))
        .collect();
    let plan = prune_plan(&copies, &known, now);
    assert_eq!(plan.remove.len(), 6 - CONFLICTS_PER_PATH);
    // The newest wait; the oldest go.
    assert_eq!(
        plan.remove,
        copies[CONFLICTS_PER_PATH..]
            .iter()
            .map(|c| c.path.clone())
            .collect::<Vec<_>>()
    );
    assert!(plan.unique.is_empty());
}

#[tokio::test]
async fn pruning_reads_the_row_and_the_file_as_the_texts_the_notebook_holds() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    state
        .db
        .add_note(&Note {
            id: "note".into(),
            notebook_id: "shared-notebook".into(),
            title: "Example".into(),
            content: "Row text.".into(),
            kind: "audio_overview".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    write_bound(&state, "shared-notebook").await.unwrap();
    let old = now_ms() - CONFLICT_GRACE_MS - 1000;
    std::fs::create_dir_all(bundle.join("conflicts")).unwrap();
    for (name, text) in [
        ("row.md", "Row text."),
        ("unique.md", "Something a person wrote and lost."),
    ] {
        let path = bundle.join("conflicts").join(name);
        std::fs::write(&path, conflict_copy_text("notes/example.md", "local", text)).unwrap();
        set_mtime(&path, old);
    }
    let manifest_at = manifest_path(&app_data_dir(&state), "a");
    let manifest = load_manifest(&manifest_at);
    let done = prune_conflicts(&state, &bundle, &manifest).await;
    assert_eq!(done.removed, 1);
    assert_eq!(done.unique.len(), 1);
    assert!(!bundle.join("conflicts/row.md").exists());
    assert!(bundle.join("conflicts/unique.md").exists());
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert!(log.contains("Cleared 1 conflict copy whose text is back in the notebook."));
}

// ---- `<name> 2.md` ----------------------------------------------------------

#[test]
fn a_numbered_twin_names_its_original_and_owned_is_by_path() {
    assert_eq!(twin_canonical("index 2.md").as_deref(), Some("index.md"));
    assert_eq!(
        twin_canonical("my note 12.md").as_deref(),
        Some("my note.md")
    );
    assert_eq!(
        twin_canonical("7ad3-… 2.json").as_deref(),
        Some("7ad3-….json")
    );
    assert_eq!(twin_canonical("index.md"), None);
    assert_eq!(
        twin_canonical("chapter 1.md"),
        None,
        "somebody's own numbering"
    );
    assert_eq!(twin_canonical("2.md"), None);
    assert_eq!(twin_canonical("index 2"), None);
    for owned in [
        "index.md",
        "log.md",
        "sources/index.md",
        "notes/deep/index.md",
        "sync/protocol.json",
        "sync/deletions/x.json",
    ] {
        assert!(is_owned(owned), "{owned}");
    }
    for user in [
        "sources/architecture.md",
        "notes/index-of-terms.md",
        "references/paper.pdf",
        "README.md",
    ] {
        assert!(!is_owned(user), "{user}");
    }
    assert!(is_okf_reserved("/b/sources/index 2.md"));
    assert!(!is_okf_reserved("/b/sources/orders 2.md"));
}

#[test]
fn owned_twins_are_resolved_newer_wins_and_user_twins_are_named_once() {
    let bundle = scratch("twins");
    let aside = scratch("aside");
    std::fs::create_dir_all(bundle.join("sources")).unwrap();
    std::fs::create_dir_all(bundle.join("notes")).unwrap();
    std::fs::create_dir_all(bundle.join("sync/deletions")).unwrap();
    // Identical twin: the copy is set aside.
    std::fs::write(bundle.join("index.md"), "same").unwrap();
    std::fs::write(bundle.join("index 2.md"), "same").unwrap();
    // A newer twin takes the name; the older original is set aside.
    std::fs::write(bundle.join("sources/index.md"), "old listing").unwrap();
    std::fs::write(bundle.join("sources/index 2.md"), "new listing").unwrap();
    set_mtime(&bundle.join("sources/index.md"), 1_000_000);
    set_mtime(&bundle.join("sources/index 2.md"), 2_000_000);
    // An older twin is set aside and the original keeps its name.
    std::fs::write(bundle.join("log.md"), "# Log\n").unwrap();
    std::fs::write(bundle.join("log 2.md"), "# Log\nolder\n").unwrap();
    set_mtime(&bundle.join("log.md"), 2_000_000);
    set_mtime(&bundle.join("log 2.md"), 1_000_000);
    // A twin with no original just takes the name.
    std::fs::write(bundle.join("sync/deletions/abc 2.json"), "{}").unwrap();
    // A user's document: left alone, named once.
    std::fs::write(bundle.join("notes/plan.md"), "plan").unwrap();
    std::fs::write(bundle.join("notes/plan 2.md"), "plan, other Mac").unwrap();

    let done = tidy_cloud_twins(&bundle, &aside);
    assert_eq!(done.resolved, 4);
    assert_eq!(done.noted, vec!["notes/plan 2.md"]);
    assert_eq!(done.failed, 0);
    assert!(!bundle.join("index 2.md").exists());
    assert_eq!(
        std::fs::read_to_string(bundle.join("sources/index.md")).unwrap(),
        "new listing"
    );
    assert!(!bundle.join("sources/index 2.md").exists());
    assert!(!bundle.join("log 2.md").exists());
    assert!(bundle.join("sync/deletions/abc.json").exists());
    assert!(bundle.join("notes/plan 2.md").exists());
    assert!(bundle.join("notes/plan.md").exists());
    // Nothing was deleted: every loser is under the set-aside folder.
    let mut kept: Vec<String> = std::fs::read_dir(&aside)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    kept.sort();
    assert_eq!(kept.len(), 3);
    assert!(kept.iter().any(|n| n.ends_with("-index 2.md")));
    assert!(
        kept.iter().any(|n| n.ends_with("-index.md")),
        "the older sources listing"
    );
    assert!(kept.iter().any(|n| n.ends_with("-log 2.md")));
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert!(log.contains("The cloud left a second copy of notes/plan.md as notes/plan 2.md"));
    assert!(log.contains("Set aside the cloud's second copy of index.md (index 2.md)."));
    // A second pass: nothing owned is left, and the user twin is not
    // named again.
    let again = tidy_cloud_twins(&bundle, &aside);
    assert_eq!(again.resolved, 0);
    assert_eq!(again.noted, vec!["notes/plan 2.md"]);
    let log2 = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert_eq!(log2.matches("notes/plan 2.md").count(), 1);
    let _ = std::fs::remove_dir_all(&bundle);
    let _ = std::fs::remove_dir_all(&aside);
}

#[tokio::test]
async fn a_cloud_twin_of_a_deletion_record_does_not_stop_the_pass() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    write_bound(&state, "shared-notebook").await.unwrap();
    std::fs::create_dir_all(bundle.join("sync/deletions")).unwrap();
    let id = "7ad32a5d-905f-8cf9-a9e1-2c2441246376";
    let record = format!("{{\"version\":1,\"id\":\"{id}\"}}\n");
    std::fs::write(bundle.join(format!("sync/deletions/{id}.json")), &record).unwrap();
    std::fs::write(bundle.join(format!("sync/deletions/{id} 2.json")), &record).unwrap();
    std::fs::write(bundle.join("index 2.md"), "stale").unwrap();
    reconcile(&state, "shared-notebook").await.unwrap();
    assert!(!bundle.join(format!("sync/deletions/{id} 2.json")).exists());
    assert!(!bundle.join("index 2.md").exists());
    assert!(set_aside_dir(&app_data_dir(&state), "a").is_dir());
}

// ---- the launch heal --------------------------------------------------------

#[tokio::test]
async fn the_launch_heal_trims_every_bound_bundle_once() {
    let lab = Lab::new();
    let bundle = lab.0.join("bundle");
    let state = lab.replica("a", &bundle).await;
    state
        .db
        .add_note(&Note {
            id: "note".into(),
            notebook_id: "shared-notebook".into(),
            title: "Plan".into(),
            content: "The plan.".into(),
            kind: "audio_overview".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    write_bound(&state, "shared-notebook").await.unwrap();
    // The 0.56 and 0.58 state: a log carrying whole documents, none of
    // them with a copy under conflicts/, and an `index 2.md`.
    std::fs::write(bundle.join("log.md"), bloated_log()).unwrap();
    std::fs::write(bundle.join("index 2.md"), "stale").unwrap();
    let done = heal_bundle_bloat_checked(&state).await;
    assert_eq!(done.bundles, 1);
    assert_eq!(done.failed, 0);
    assert_eq!(done.twins_resolved, 1);
    assert_eq!(done.log.collapsed, 4);
    // `bbbb` held the plan, which the row holds — the log's copy was never
    // the only one, so nothing is written for it. The other three held
    // words nobody has: put under conflicts/, kept, and named.
    assert_eq!(done.log.copies_restored, 3);
    assert_eq!(done.log.copies_dropped, 1);
    assert_eq!(done.conflicts_removed, 0);
    assert_eq!(done.conflicts_unique, 3);
    let log = std::fs::read_to_string(bundle.join("log.md")).unwrap();
    assert!(
        log.contains("Kept 3 conflict copies holding text found nowhere else: conflicts/"),
        "{log}"
    );
    assert!(log.contains("conflicts/aaaa.md (sources/architecture.md)"));
    assert!(
        !log.contains("The body, with headings of its own."),
        "the document text left the log: {log}"
    );
    assert!(!log.contains("An older plan."));
    // A redundant copy past the grace: the next pass clears it and leaves
    // the unique ones.
    let stale = bundle.join("conflicts/cccc.md");
    std::fs::write(
        &stale,
        conflict_copy_text("notes/plan.md", "local", "The plan."),
    )
    .unwrap();
    set_mtime(&stale, now_ms() - CONFLICT_GRACE_MS - 1000);
    let again = heal_bundle_bloat_checked(&state).await;
    assert_eq!(again.conflicts_removed, 1);
    assert_eq!(again.conflicts_unique, 3);
    assert_eq!(again.log, LogTrim::default());
    assert!(bundle.join("conflicts/aaaa.md").exists());
    assert!(!stale.exists());
    // The stamped entry point runs once.
    heal_bundle_bloat(&state).await;
    assert_eq!(
        std::fs::read_to_string(app_data_dir(&state).join("okf-bloat-healed")).unwrap(),
        BLOAT_HEAL_VERSION
    );
}
