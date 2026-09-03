//! End-to-end data-path test: ingest → embed → LanceDB write → vector search →
//! grounded chat. Requires a running Ollama with `nomic-embed-text`.
//!
//! Opt-in. It used to run whenever Ollama happened to be listening, which on a
//! developer machine is always — a full embed-and-chat round trip on every
//! plain `cargo test`. It no-ops without the flag, so CI stays green with no
//! model server and a local run stays fast:
//!
//!   ALCHEMY_OLLAMA_TESTS=1 cargo test --lib rag_round_trip -- --nocapture

use crate::ai::{Ollama, OllamaConfig};
use crate::db::Db;
use crate::ingest;
use crate::models::{Notebook, Source};
use crate::rag;

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[tokio::test]
async fn rag_round_trip() {
    let ai = Ollama::new(OllamaConfig {
        base_url: "http://localhost:11434".into(),
        // Small local model to keep the chat step fast.
        chat_model: "digitsflow/bonsai-8b:latest".into(),
        embed_model: "nomic-embed-text".into(),
        vision_model: String::new(),
        effort: String::new(),
        // Eval-tier discipline (see evals::eval_ollama): short residency
        // and a runaway cap, so a rambling reply can't wedge the suite.
        keep_alive: Some("1m".into()),
        num_predict: Some(2_048),
        think: None,
    });
    if !crate::evals::ollama_tests_enabled() {
        eprintln!("SKIP: set ALCHEMY_OLLAMA_TESTS=1 to run the Ollama round trip");
        return;
    }
    if ai.list_models().await.is_err() {
        eprintln!("SKIP: Ollama not reachable on localhost:11434");
        return;
    }

    let dir = std::env::temp_dir().join(format!("nbl-test-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");

    // 1. Notebook
    let nb = Notebook {
        id: uuid::Uuid::new_v4().to_string(),
        title: "Photosynthesis".into(),
        created_at: now(),
        updated_at: now(),
        color: "#eb5757".into(),
        icon: String::new(),
        status: String::new(),
        growth_web: false,
        source_count: 0,
        note_count: 0,
        report_count: 0,
    };
    db.create_notebook(&nb).await.expect("create notebook");
    assert_eq!(
        db.list_notebooks().await.unwrap().len(),
        1,
        "notebook persisted"
    );

    // 2. Ingest + chunk + embed + write
    let text = "Photosynthesis is the process by which green plants and some bacteria \
        convert light energy into chemical energy. It occurs in the chloroplasts using \
        the green pigment chlorophyll. The light-dependent reactions occur in the \
        thylakoid membranes and produce ATP and NADPH. The Calvin cycle occurs in the \
        stroma and fixes carbon dioxide into glucose. The overall products are glucose \
        and oxygen.";
    let extracted = ingest::extract_pasted("Photosynthesis basics", text).expect("extract");
    let chunks = ingest::chunk_text(&extracted.title, &extracted.text);
    assert!(!chunks.is_empty(), "produced chunks");
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = ai.embed(&embed_inputs).await.expect("embed");
    assert_eq!(embeddings.len(), chunks.len(), "one vector per chunk");
    eprintln!(
        "embedded {} chunks, dim={}",
        chunks.len(),
        embeddings[0].len()
    );

    let chunk_tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (uuid::Uuid::new_v4().to_string(), i as i32, c.text.clone()))
        .collect();
    let source = Source {
        image_url: String::new(),
        author: String::new(),
        id: uuid::Uuid::new_v4().to_string(),
        notebook_id: nb.id.clone(),
        title: extracted.title,
        source_type: extracted.source_type,
        url: extracted.url,
        content: extracted.text.clone(),
        char_count: extracted.text.chars().count() as i64,
        chunk_count: chunk_tuples.len() as i64,
        created_at: now(),
        status: "ready".to_string(),
        error: String::new(),
        parent_id: String::new(),
        mtime: 0,
        tags: String::new(),
        note: String::new(),
        fetched_at: 0,
        fetch_failures: 0,
    };
    db.insert_source(&source, &chunk_tuples, &embeddings)
        .await
        .expect("insert source");
    assert_eq!(
        db.list_sources(&nb.id).await.unwrap().len(),
        1,
        "source persisted"
    );

    // 3. Vector search
    let qvec = ai
        .embed(&["Where do the light-dependent reactions happen?".to_string()])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let citations = db
        .search_chunks(
            &nb.id,
            qvec,
            "Where do the light-dependent reactions happen?",
            4,
            None,
        )
        .await
        .expect("search");
    assert!(!citations.is_empty(), "retrieved at least one chunk");
    eprintln!(
        "top citation: \"{}\" (dist={:.3})",
        citations[0].source_title, citations[0].distance
    );
    assert_eq!(citations[0].source_title, "Photosynthesis basics");
    assert!(
        citations[0].snippet.to_lowercase().contains("thylakoid"),
        "top hit should mention thylakoid; got: {}",
        citations[0].snippet
    );

    // 4. Grounded chat
    let no_expansion = std::collections::HashMap::new();
    let messages = rag::build_chat_messages(
        &[],
        "Where do the light-dependent reactions occur?",
        rag::Excerpts {
            citations: &citations,
            expanded: &no_expansion,
        },
        &[("src-1".to_string(), String::new(), String::new())],
        "",
        "",
        &crate::inference::ContextProfile::default(),
    );
    let answer = ai.chat(&messages).await.expect("chat").text;
    eprintln!("answer: {answer}");
    assert!(!answer.trim().is_empty(), "model produced an answer");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The OpenAI-compatible client (gateway path) verified against Ollama's own
/// /v1 gateway — same wire protocol, zero mocks. Skips when Ollama is down.
#[tokio::test]
async fn openai_gateway_round_trip() {
    use crate::ai::{ChatTurn, OpenAiClient};

    let probe = Ollama::new(OllamaConfig::default());
    let Ok(models) = probe.list_models().await else {
        eprintln!("SKIP: Ollama not reachable on localhost:11434");
        return;
    };
    let small = models
        .iter()
        .find(|m| m.contains("bonsai") || m.contains("12b-mlx"))
        .cloned()
        .unwrap_or_else(|| models[0].clone());

    let gw = OpenAiClient::new("http://localhost:11434/v1", "test-key", &small);

    // Non-streaming
    let out = gw
        .chat(&[ChatTurn::user("Reply with exactly: alchemy works")])
        .await
        .expect("gateway chat");
    eprintln!("gateway non-stream ({small}): {}", out.text.trim());
    assert!(!out.text.trim().is_empty(), "gateway returned text");

    // Streaming
    let mut streamed = String::new();
    let out = gw
        .chat_stream(&[ChatTurn::user("Count: 1 2 3")], |tok| {
            streamed.push_str(tok);
        })
        .await
        .expect("gateway stream");
    eprintln!(
        "gateway stream: {} chars, stats: {:?} tok",
        streamed.len(),
        out.stats.map(|s| s.eval_count)
    );
    assert!(!streamed.is_empty(), "tokens streamed via SSE");
    assert_eq!(streamed, out.text, "streamed text matches outcome");

    // Model listing through the gateway
    let listed = gw.list_models().await.expect("gateway /models");
    assert!(!listed.is_empty(), "gateway listed models");
}

/// Zero-Ollama data path: built-in Model2Vec embedder → LanceDB → search.
/// First run downloads ~30 MB from HF (cached afterwards); requires network
/// only for that. No Ollama involved anywhere.
#[tokio::test]
async fn builtin_embedder_round_trip() {
    use crate::ai::{Ai, AiConfig};

    let ai = Ai::new(
        AiConfig {
            embedder: "builtin".into(),
            ..Default::default()
        },
        crate::ai::AiRuntime::default(),
    );
    let Ok(dim) = ai.test_embed().await else {
        eprintln!("SKIP: built-in embedder unavailable (no network for first download?)");
        return;
    };
    assert!(dim > 0, "built-in embedder produced vectors");
    eprintln!("builtin dim: {dim}");

    let dir = std::env::temp_dir().join(format!("nbl-builtin-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb_id = uuid::Uuid::new_v4().to_string();

    let text = "The light-dependent reactions occur in the thylakoid membranes. \
        The Calvin cycle occurs in the stroma. Ferrari builds sports cars in Maranello.";
    let chunks = ingest::chunk_text("Biology notes", text);
    let embed_inputs: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let embeddings = ai.embed(&embed_inputs).await.expect("embed");
    let tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (uuid::Uuid::new_v4().to_string(), i as i32, c.text.clone()))
        .collect();
    db.add_chunks(&nb_id, "src-1", &tuples, &embeddings)
        .await
        .expect("write chunks");

    let qvec = ai
        .embed_one("Where do light-dependent reactions happen?")
        .await
        .expect("embed query");
    let hits = db
        .search_chunks(
            &nb_id,
            qvec,
            "Where do light-dependent reactions happen?",
            2,
            None,
        )
        .await
        .expect("search");
    assert!(!hits.is_empty(), "retrieved chunks with builtin embeddings");
    assert!(
        hits[0].snippet.to_lowercase().contains("thylakoid"),
        "top hit mentions thylakoid; got: {}",
        hits[0].snippet
    );
    eprintln!(
        "builtin round trip OK: top hit dist={:.3}",
        hits[0].distance
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Notes ride the chunk table under `source_id = "note:<id>"` (RFC-note-curator
/// phase 1): search must label note passages, and deleting the note must drop
/// its chunks. Uses the built-in embedder; skips offline like the test above.
#[tokio::test]
async fn note_retrieval_round_trip() {
    use crate::ai::{Ai, AiConfig};
    use crate::db::NOTE_CHUNK_PREFIX;
    use crate::models::Note;

    let ai = Ai::new(
        AiConfig {
            embedder: "builtin".into(),
            ..Default::default()
        },
        crate::ai::AiRuntime::default(),
    );
    if ai.test_embed().await.is_err() {
        eprintln!("SKIP: built-in embedder unavailable (no network for first download?)");
        return;
    }

    let dir = std::env::temp_dir().join(format!("nbl-notes-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb_id = uuid::Uuid::new_v4().to_string();

    // A source chunk so notes and sources coexist in one table.
    let src_text = "The Calvin cycle occurs in the stroma and fixes carbon dioxide.";
    let src_chunks = ingest::chunk_text("Biology", src_text);
    let src_inputs: Vec<String> = src_chunks.iter().map(|c| c.embed_text.clone()).collect();
    let src_embeds = ai.embed(&src_inputs).await.expect("embed source");
    let src_tuples: Vec<(String, i32, String)> = src_chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (uuid::Uuid::new_v4().to_string(), i as i32, c.text.clone()))
        .collect();
    db.add_chunks(&nb_id, "src-1", &src_tuples, &src_embeds)
        .await
        .expect("write source chunks");

    // An evidence note, indexed under the note prefix.
    let note = Note {
        id: uuid::Uuid::new_v4().to_string(),
        notebook_id: nb_id.clone(),
        title: "Deductible decision".into(),
        content: "We concluded the homeowner's hail deductible is twenty-five hundred \
                  dollars, based on the insurance policy PDF."
            .into(),
        kind: "evidence".into(),
        prompt: String::new(),
        origin: String::new(),
        status: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.add_note(&note).await.expect("add note");
    let note_chunks = ingest::chunk_text(&note.title, &note.content);
    let note_inputs: Vec<String> = note_chunks.iter().map(|c| c.embed_text.clone()).collect();
    let note_embeds = ai.embed(&note_inputs).await.expect("embed note");
    let note_tuples: Vec<(String, i32, String)> = note_chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (uuid::Uuid::new_v4().to_string(), i as i32, c.text.clone()))
        .collect();
    db.add_chunks(
        &nb_id,
        &format!("{NOTE_CHUNK_PREFIX}{}", note.id),
        &note_tuples,
        &note_embeds,
    )
    .await
    .expect("write note chunks");

    assert!(
        db.indexed_note_ids()
            .await
            .expect("indexed ids")
            .contains(&note.id),
        "note id visible in the index"
    );

    // Search: the note passage comes back labeled as a note, with its title.
    let q = "what is the hail deductible?";
    let qvec = ai.embed_one(q).await.expect("embed query");
    let hits = db
        .search_chunks(&nb_id, qvec, q, 4, None)
        .await
        .expect("search");
    let note_hit = hits
        .iter()
        .find(|c| c.note_id == note.id)
        .expect("note passage retrieved");
    assert!(note_hit.source_id.is_empty(), "note hit has no source id");
    assert_eq!(
        note_hit.source_title, note.title,
        "note hit carries note title"
    );

    // Narrowing to explicit sources excludes notes.
    let qvec = ai.embed_one(q).await.expect("embed query");
    let narrowed = db
        .search_chunks(&nb_id, qvec, q, 4, Some(&["src-1".to_string()]))
        .await
        .expect("scoped search");
    assert!(
        narrowed.iter().all(|c| c.note_id.is_empty()),
        "source-scoped search returns no note passages"
    );

    // Usage counters: first bump inserts, repeat bumps increment, and one
    // answer citing several passages of a note still counts once (deduped).
    let ids = vec![note.id.clone(), note.id.clone()];
    db.bump_note_usage(&ids, "retrieval_hits", now())
        .await
        .expect("bump insert");
    db.bump_note_usage(&ids, "retrieval_hits", now())
        .await
        .expect("bump update");
    db.bump_note_usage(std::slice::from_ref(&note.id), "cited", now())
        .await
        .expect("bump cited");
    assert!(
        db.bump_note_usage(std::slice::from_ref(&note.id), "nonsense", now())
            .await
            .is_err(),
        "unknown counter field rejected"
    );
    let usage = db.note_usage().await.expect("usage");
    assert_eq!(usage.len(), 1, "one usage row per note");
    assert_eq!(
        usage[0].retrieval_hits, 2,
        "deduped within a call, summed across calls"
    );
    assert_eq!(usage[0].cited, 1);
    assert_eq!(usage[0].reads, 0);
    assert!(usage[0].last_used_at > 0);

    // Deleting the note drops its chunks and its usage row.
    db.delete_note(&note.id).await.expect("delete note");
    assert!(
        db.indexed_note_ids().await.expect("indexed ids").is_empty(),
        "note chunks removed with the note"
    );
    assert!(
        db.note_usage().await.expect("usage").is_empty(),
        "usage row removed with the note"
    );

    eprintln!("note retrieval round trip OK");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The curator's deterministic pass (RFC-note-curator phase 4): auto notes
/// go stale after 30 unused APP-OPEN days, archive at 90, and any use
/// revives them. Owned notes are never touched. Pure DB — no embedder.
#[tokio::test]
async fn note_curator_round_trip() {
    use crate::commands::curate_notes;
    use crate::models::Note;

    let dir = std::env::temp_dir().join(format!("nbl-curator-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");

    const DAY: i64 = 86_400_000;
    let born_day: i64 = 20_000; // arbitrary epoch day number
    let born_ms = born_day * DAY + 1;
    let mk = |origin: &str, title: &str| Note {
        id: uuid::Uuid::new_v4().to_string(),
        notebook_id: "nb-1".into(),
        title: title.into(),
        content: "An old conclusion.".into(),
        kind: "evidence".into(),
        prompt: String::new(),
        origin: origin.into(),
        status: String::new(),
        created_at: born_ms,
        updated_at: born_ms,
    };
    let auto = mk("auto", "Auto claim");
    let owned = mk("", "Owned claim");
    db.add_note(&auto).await.expect("add auto");
    db.add_note(&owned).await.expect("add owned");

    let status_of = |id: String| {
        let db = &db;
        async move { db.get_note(&id).await.unwrap().unwrap().status }
    };

    // 31 app-open days since last use → stale (owned note untouched).
    let open_days: Vec<i64> = (1..=31).map(|i| born_day + i).collect();
    let actions = curate_notes(&db, &open_days).await.expect("curate");
    assert_eq!(actions.len(), 1, "only the auto note transitions");
    assert_eq!(actions[0].action, "stale");
    assert_eq!(status_of(auto.id.clone()).await, "stale");
    assert_eq!(status_of(owned.id.clone()).await, "");

    // Idempotent between thresholds: a rerun does nothing.
    assert!(curate_notes(&db, &open_days)
        .await
        .expect("rerun")
        .is_empty());

    // 95 open days → archived.
    let open_days: Vec<i64> = (1..=95).map(|i| born_day + i).collect();
    let actions = curate_notes(&db, &open_days).await.expect("curate");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].action, "archived");
    assert_eq!(status_of(auto.id.clone()).await, "archived");

    // Fresh usage (after every open day) revives it.
    db.bump_note_usage(
        std::slice::from_ref(&auto.id),
        "reads",
        (born_day + 200) * DAY,
    )
    .await
    .expect("bump");
    let actions = curate_notes(&db, &open_days).await.expect("curate");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].action, "revived");
    assert_eq!(status_of(auto.id.clone()).await, "");
    assert!(curate_notes(&db, &open_days)
        .await
        .expect("rerun")
        .is_empty());

    eprintln!("note curator round trip OK");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn okf_helpers() {
    use crate::okf::{okf_description, okf_slug};
    assert_eq!(
        okf_slug("Building macOS Apps with Tauri!"),
        "building-macos-apps-with-tauri"
    );
    assert_eq!(okf_slug("***"), "untitled");
    assert_eq!(okf_slug("Ünïcode — Títle"), "n-code-t-tle");
    let d = okf_description("# Heading\n\nSome **bold** text\nwith lines");
    assert_eq!(d, "Heading Some bold text with lines");
    let long = "word ".repeat(60);
    assert!(okf_description(&long).ends_with('…'));
}

#[test]
fn audio_script_parsing() {
    use crate::tts::{parse_script, Speaker};
    let script = "\
# Episode\n\
HOST: Welcome to the show!\n\
**GUEST:** Thanks — glad to be here.\n\
guest — Lowercase with a dash works too.\n\
Some narration line that is skipped.\n\
Hostile prose starting with host-ish words is skipped.\n\
HOST:\n\
HOST: Second real host line.";
    let lines = parse_script(script);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0].speaker, Speaker::Host);
    assert_eq!(lines[0].text, "Welcome to the show!");
    assert_eq!(lines[1].speaker, Speaker::Guest);
    assert_eq!(lines[1].text, "Thanks — glad to be here.");
    assert_eq!(lines[2].speaker, Speaker::Guest);
    assert_eq!(lines[3].text, "Second real host line.");
    assert!(parse_script("just prose, no dialogue").is_empty());
}

/// Kokoro end-to-end: downloads the model into the real app data dir on
/// first run (~93 MB — also pre-warms the app), then synthesizes one line
/// per voice. Ignored by default: needs network and a few minutes.
/// Run with: cargo test kokoro_smoke -- --ignored --nocapture
#[tokio::test]
#[ignore = "downloads ~93 MB and runs real inference"]
async fn kokoro_smoke() {
    use crate::tts::{ensure_kokoro_files, KokoroEngine, Speaker};
    let home = std::env::var("HOME").expect("HOME");
    let dir = std::path::PathBuf::from(home)
        .join("Library/Application Support/com.thrashr888.alchemy/kokoro");
    let cancel = tokio_util::sync::CancellationToken::new();
    ensure_kokoro_files(&dir, None, &cancel)
        .await
        .expect("download kokoro");
    let engine = KokoroEngine::load(&dir).await.expect("load kokoro");
    let out = std::env::temp_dir().join("alchemy-kokoro-smoke-host.wav");
    engine
        .synth(
            Speaker::Host,
            "Welcome back to the show. Today we're digging into something genuinely surprising.",
            &out,
        )
        .await
        .expect("synth host line");
    let host_len = std::fs::metadata(&out).unwrap().len();
    let out2 = std::env::temp_dir().join("alchemy-kokoro-smoke-guest.wav");
    engine
        .synth(
            Speaker::Guest,
            "Thanks for having me. The short version: the data doesn't say what everyone thinks.",
            &out2,
        )
        .await
        .expect("synth guest line");
    let guest_len = std::fs::metadata(&out2).unwrap().len();
    assert!(
        host_len > 50_000 && guest_len > 50_000,
        "audio suspiciously small"
    );

    // Stitch both lines into an episode m4a — the full pipeline shape.
    let m4a = std::env::temp_dir().join("alchemy-kokoro-smoke.m4a");
    crate::tts::assemble_episode(
        &[out.clone(), out2.clone()],
        &[350],
        &m4a,
        KokoroEngine::SAMPLE_RATE,
    )
    .await
    .expect("assemble episode");
    let episode_len = std::fs::metadata(&m4a).unwrap().len();
    assert!(episode_len > 20_000, "episode suspiciously small");
    eprintln!(
        "kokoro smoke OK: host {host_len} B, guest {guest_len} B, episode {episode_len} B ({})",
        m4a.display()
    );
}

#[test]
fn outro_stripping() {
    use crate::commands::strip_outro;
    let script = "HOST: Welcome!\nGUEST: Glad to be here.\nHOST: Deep point.\nGUEST: Indeed.\nHOST: That's a wrap — thanks for listening!\nGUEST: See you next time.";
    let trimmed = strip_outro(script);
    assert!(
        trimmed.ends_with("GUEST: Indeed."),
        "outro removed: {trimmed}"
    );
    // A "thanks for listening" far from the tail survives — only the last
    // few lines are outro territory.
    let long: String = "HOST: Thanks for listening tips came up early here.\n".to_string()
        + &(0..10)
            .map(|i| format!("GUEST: Substantive line {i}.\n"))
            .collect::<String>()
        + "HOST: Final point.";
    assert_eq!(strip_outro(&long), long);
    // No outro → unchanged.
    assert_eq!(strip_outro("HOST: A.\nGUEST: B."), "HOST: A.\nGUEST: B.");
}

/// Ledger rows survive a full write → read → update round trip. Regression:
/// ledger_batch once omitted the `origin` column, so every add failed with a
/// column-count mismatch — and the old drop-and-refill migration turned that
/// same failure into a wiped table.
#[tokio::test]
async fn ledger_round_trip() {
    use crate::models::{LedgerAnchor, LedgerEntry};
    let dir = std::env::temp_dir().join(format!("nbl-ledger-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let now = crate::commands::now();
    let entry = LedgerEntry {
        id: uuid::Uuid::new_v4().to_string(),
        notebook_id: "nb-1".into(),
        kind: "assertion".into(),
        text: "Kenya AA extracts best at 93C".into(),
        why: "Dial-in session".into(),
        status: "asserted".into(),
        origin: "auto".into(),
        anchors: vec![LedgerAnchor {
            source_id: "src-1".into(),
            quote: "start at 93C".into(),
        }],
        created_at: now,
        updated_at: now,
    };
    db.add_ledger_entry(&entry).await.expect("add entry");

    let listed = db.list_ledger("nb-1").await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].origin, "auto");
    assert_eq!(listed[0].anchors.len(), 1);
    assert_eq!(listed[0].text, entry.text);

    let mut updated = listed[0].clone();
    updated.status = "contradicted".into();
    updated.why = format!("{}\nWeave: contradicted", updated.why);
    db.update_ledger_entry(&updated).await.expect("update");
    let back = db.get_ledger_entry(&entry.id).await.expect("get").unwrap();
    assert_eq!(back.status, "contradicted");
    assert_eq!(back.origin, "auto");
    std::fs::remove_dir_all(&dir).ok();
}

/// The triage column survives the write → read round trip, and "keep
/// recommended" rules exactly the triage pass's picks: the recommended
/// suggestion joins the cast (triage cleared — it is queue metadata), the
/// routine one stays queued, and cards the user already owns are untouched.
#[tokio::test]
async fn keep_recommended_rules_only_the_marked_suggestions() {
    use crate::models::RegistryCard;
    use std::sync::Arc;

    let dir = std::env::temp_dir().join(format!("nbl-triage-{}", uuid::Uuid::new_v4()));
    let db = Arc::new(Db::open(&dir).await.expect("open db"));
    let card = |name: &str, origin: &str, triage: &str| RegistryCard {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "asset".into(),
        name: name.into(),
        origin: origin.into(),
        triage: triage.into(),
        identifiers: String::new(),
        note: String::new(),
        facts: vec![],
        attachments: vec![],
        created_at: now(),
        updated_at: now(),
    };
    db.add_registry_card(&card("Ducati Monster", "auto", "recommended"))
        .await
        .expect("add");
    db.add_registry_card(&card("Corley Automotive", "auto", "routine"))
        .await
        .expect("add");
    db.add_registry_card(&card("Sea Otter", "", ""))
        .await
        .expect("add");

    let ruled = crate::commands::rule_all_suggested_cards(&db, "", true)
        .await
        .expect("rule");
    assert_eq!(ruled, 1, "only the recommended suggestion is ruled");

    let cards = db.list_registry().await.expect("list");
    let by_name = |n: &str| cards.iter().find(|c| c.name == n).unwrap();
    let kept = by_name("Ducati Monster");
    assert_eq!(kept.origin, "");
    assert_eq!(kept.triage, "", "verdicts clear once ruled on");
    assert_eq!(by_name("Corley Automotive").origin, "auto");
    assert_eq!(by_name("Corley Automotive").triage, "routine");
    assert_eq!(by_name("Sea Otter").origin, "");
    std::fs::remove_dir_all(&dir).ok();
}

/// The heal pass collapses what a suggest race left behind — modeled on the
/// live pollution: the same 4Runner three times, one utility in two
/// casings, and auto cards restating a card the user owns or dismissed.
/// The oldest of each group survives (absorbing fact labels it lacked);
/// user-owned and dismissed cards are never touched.
#[tokio::test]
async fn heal_collapses_raced_suggestion_duplicates() {
    use crate::models::{CardFact, RegistryCard};

    let dir = std::env::temp_dir().join(format!("nbl-heal-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let card = |name: &str, origin: &str, created_at: i64, facts: Vec<CardFact>| RegistryCard {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "asset".into(),
        name: name.into(),
        origin: origin.into(),
        triage: String::new(),
        identifiers: String::new(),
        note: String::new(),
        facts,
        attachments: vec![],
        created_at,
        updated_at: created_at,
    };
    let vin = CardFact {
        label: "VIN".into(),
        value: "JTEBU5JR8K1234567".into(),
    };
    for c in [
        card("2019 Toyota 4Runner SR5", "auto", 1, vec![]),
        card("2019 Toyota 4Runner SR5", "auto", 2, vec![vin.clone()]),
        card("2019 Toyota 4Runner SR5", "auto", 3, vec![]),
        card("Pacific Light & Power", "auto", 4, vec![]),
        card("PACIFIC LIGHT & POWER", "auto", 5, vec![]),
        card("15217 Canyon Seven Rd", "", 6, vec![]),
        card("15217 Canyon Seven Road", "auto", 7, vec![]),
        card("Corley Automotive", "dismissed", 8, vec![]),
        card("Corley Automotive", "auto", 9, vec![]),
    ] {
        db.add_registry_card(&c).await.expect("add");
    }

    let removed = crate::commands::heal_suggested_duplicates(&db).await;
    assert_eq!(removed, 5, "2 extra 4Runners, 1 utility, road, corley");

    let cards = db.list_registry().await.expect("list");
    assert_eq!(cards.len(), 4);
    let runner = cards
        .iter()
        .find(|c| c.name == "2019 Toyota 4Runner SR5")
        .expect("oldest 4Runner survives");
    assert_eq!(
        runner.created_at, 1,
        "the oldest of the group is the keeper"
    );
    assert_eq!(runner.origin, "auto", "still suggested, still un-ruled");
    assert_eq!(
        runner.facts,
        vec![vin],
        "the keeper absorbs facts its duplicates carried"
    );
    assert!(
        cards
            .iter()
            .any(|c| c.name == "Pacific Light & Power" && c.origin == "auto"),
        "one casing of the utility survives"
    );
    let owned = cards
        .iter()
        .find(|c| c.name == "15217 Canyon Seven Rd")
        .expect("owned card untouched");
    assert_eq!(owned.origin, "");
    let dismissed = cards
        .iter()
        .find(|c| c.name == "Corley Automotive")
        .expect("refusal memory untouched");
    assert_eq!(dismissed.origin, "dismissed");
    std::fs::remove_dir_all(&dir).ok();
}

/// The prod error dump from the shared-store schema skew must come out as
/// one actionable sentence, and generic errors must lose their `location:`
/// code-path noise.
#[test]
fn ipc_errors_read_like_sentences() {
    use crate::commands::friendly_error;

    let lance = "lance error: Append with different schema: fields did not match, \
                 missing=[image_url], unexpected=[], location: /Users/x/.cargo/registry/src/\
                 lance-core-7.0.0/src/datatypes/schema.rs:186:17: Append with different schema: \
                 fields did not match, missing=[image_url], unexpected=[], location: \
                 /Users/x/.cargo/registry/src/lance-core-7.0.0/src/datatypes/schema.rs:186:17";
    let friendly = friendly_error(lance);
    assert!(friendly.contains("newer version"), "actionable: {friendly}");
    assert!(!friendly.contains("location:"), "no code paths: {friendly}");
    assert!(!friendly.contains(".rs:"), "no code paths: {friendly}");

    let generic = "could not reach https://x.test, location: /some/path/net.rs:10:2";
    let cleaned = friendly_error(generic);
    assert!(
        cleaned.starts_with("could not reach https://x.test"),
        "{cleaned}"
    );
    assert!(!cleaned.contains("location:"), "{cleaned}");

    // Ordinary errors pass through untouched.
    assert_eq!(friendly_error("Source not found"), "Source not found");
}

/// RFC-self-resolve phase 1: the known provider failure shapes come out as
/// the fix, phrased in the two grammars the frontend turns into buttons —
/// `` Fix: open Terminal, run `cmd`, then retry here. `` and the literal
/// "Settings → Models".
#[test]
fn model_errors_classify_to_fixes() {
    use crate::commands::friendly_error;

    // Ollama daemon down: connection refused on its port, chat path.
    let down = "ollama chat request failed: error sending request for url \
                (http://localhost:11434/api/chat): error trying to connect: \
                tcp connect error: Connection refused (os error 61)";
    let msg = friendly_error(down);
    assert!(msg.contains("Ollama isn't running"), "{msg}");
    assert!(msg.contains("run `ollama serve`"), "{msg}");
    assert!(msg.contains("Settings → Models"), "{msg}");

    // Same daemon-down shape through the embedding path (pre-stream failure,
    // surfaces as a toast) gets the same fix.
    let embed = "embedding request to Ollama failed or timed out — is `ollama serve` \
                 running and is the model `nomic-embed-text` available? (a large chat \
                 model loading can also stall this): error sending request for url \
                 (http://127.0.0.1:11434/api/embed): Connection refused";
    assert!(friendly_error(embed).contains("run `ollama serve`"));

    // Model not pulled: the exact pull command, with the name extracted from
    // the Ollama 404 body.
    let missing = r#"ollama chat 404 Not Found: {"error":"model \"gemma3:270m\" not found, try pulling it first"}"#;
    let msg = friendly_error(missing);
    assert!(msg.contains("run `ollama pull gemma3:270m`"), "{msg}");
    assert!(msg.contains("Settings → Models"), "{msg}");

    // A hostile "model name" in the body must never reach the pull hint —
    // the charset gate drops it and the generic advice shows instead.
    let hostile = r#"ollama chat 404 Not Found: {"error":"model \"x; rm -rf ~\" not found, try pulling it first"}"#;
    let msg = friendly_error(hostile);
    assert!(!msg.contains("rm -rf"), "{msg}");
    assert!(msg.contains("Settings → Models"), "{msg}");

    // Model-shaped timeout: loading/busy advice, not transport noise.
    let slow = "ollama chat request failed: error sending request for url \
                (http://localhost:11434/api/chat): operation timed out";
    let msg = friendly_error(slow);
    assert!(msg.contains("took too long"), "{msg}");

    // A slow *source fetch* is not a model problem — no model advice.
    let fetch = "could not read https://example.com/big.pdf: operation timed out";
    assert!(!friendly_error(fetch).contains("model"), "{fetch}");

    // Key rejection outside the gateway's own status translation.
    let key = "agent replied: 401 Unauthorized";
    assert!(friendly_error(key).contains("API key"));
}

/// The Terminal fix affordance stays strictly allowlisted: fixed commands
/// plus `ollama pull <name>` under the model-name charset — nothing that
/// could escape the AppleScript string or the shell.
#[test]
fn terminal_allowlist_covers_ollama_fixes() {
    use crate::commands::terminal_command_allowed;

    assert!(terminal_command_allowed("ollama serve"));
    assert!(terminal_command_allowed("ollama pull gemma3:270m"));
    assert!(terminal_command_allowed("ollama pull hf.co/org/model-1.5B"));
    assert!(terminal_command_allowed("claude"));
    assert!(terminal_command_allowed("codex login"));

    assert!(!terminal_command_allowed("ollama pull"));
    assert!(!terminal_command_allowed("ollama pull "));
    assert!(!terminal_command_allowed("ollama pull x; rm -rf ~"));
    assert!(!terminal_command_allowed("ollama pull a b"));
    assert!(!terminal_command_allowed("ollama pull \"x\""));
    assert!(!terminal_command_allowed("ollama run gemma3"));
    assert!(!terminal_command_allowed("rm -rf /"));
    assert!(!terminal_command_allowed(""));
}

/// Receipts round-trip through their own table and come back newest-first
/// (docs/RFC-night-shift-area.md §2). No model server needed: this is pure
/// storage, which is the point — the record must survive whatever the run did.
#[tokio::test]
async fn receipts_round_trip() {
    use crate::models::RunReceipt;

    let dir = std::env::temp_dir().join(format!("nbl-receipts-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");

    let base = now();
    let mk = |name: &str, sched: &str, status: &str, ended: i64| RunReceipt {
        id: uuid::Uuid::new_v4().to_string(),
        schedule_id: sched.into(),
        notebook_id: "nb-1".into(),
        name: name.into(),
        kind: "briefing".into(),
        trigger: "interval".into(),
        status: status.into(),
        detail: "Wrote a note".into(),
        error: String::new(),
        note_id: "note-1".into(),
        provider: "ollama".into(),
        model: "test-model".into(),
        cost_micros: 0,
        due_at: 0,
        started_at: ended - 1_000,
        ended_at: ended,
    };

    db.add_receipt(&mk("Older run", "sched-a", "ok", base - 60_000))
        .await
        .expect("add older");
    db.add_receipt(&mk("Newest run", "sched-a", "failed", base))
        .await
        .expect("add newest");
    db.add_receipt(&mk("Other order", "sched-b", "ok", base - 30_000))
        .await
        .expect("add other");

    let all = db.list_receipts(0, 10).await.expect("list");
    assert_eq!(all.len(), 3, "every receipt is readable");
    assert_eq!(all[0].name, "Newest run", "newest first");
    assert_eq!(
        all[0].status, "failed",
        "failures are recorded, not dropped"
    );

    // The limit truncates after ordering, so it keeps the newest.
    let capped = db.list_receipts(0, 1).await.expect("list capped");
    assert_eq!(capped.len(), 1);
    assert_eq!(capped[0].name, "Newest run");

    // A wall-clock floor excludes older runs.
    let recent = db
        .list_receipts(base - 45_000, 10)
        .await
        .expect("list since");
    assert_eq!(recent.len(), 2, "the 60s-old run falls outside the window");

    // One standing order's history, newest first.
    let for_a = db
        .receipts_for_schedule("sched-a", 5)
        .await
        .expect("per schedule");
    assert_eq!(for_a.len(), 2);
    assert!(
        for_a.iter().all(|r| r.schedule_id == "sched-a"),
        "no cross-order leakage"
    );
    assert_eq!(for_a[0].name, "Newest run");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The notebook counts cache must never serve a number it did not compute
/// (docs/RFC-professional-grade.md Pillar 2). Counting scans the sources and
/// notes tables end to end, so the result is cached — keyed on those tables'
/// Lance versions rather than invalidated by hand at each write site.
///
/// This is the test for the failure that would matter: a write that the
/// cache does not notice, leaving the shelf showing yesterday's totals.
#[tokio::test]
async fn notebook_counts_follow_writes_through_the_cache() {
    use crate::models::Note;

    let dir = std::env::temp_dir().join(format!("nbl-counts-{}", uuid::Uuid::new_v4()));
    let db = Db::open(&dir).await.expect("open db");
    let nb_id = uuid::Uuid::new_v4().to_string();
    db.create_notebook(&Notebook {
        id: nb_id.clone(),
        title: "Counts".into(),
        created_at: now(),
        updated_at: now(),
        color: String::new(),
        icon: String::new(),
        status: String::new(),
        growth_web: false,
        source_count: 0,
        note_count: 0,
        report_count: 0,
    })
    .await
    .expect("create notebook");

    let note = |kind: &str| Note {
        id: uuid::Uuid::new_v4().to_string(),
        notebook_id: nb_id.clone(),
        title: format!("A {kind}"),
        content: "body".into(),
        kind: kind.to_string(),
        prompt: String::new(),
        origin: String::new(),
        status: String::new(),
        created_at: now(),
        updated_at: now(),
    };

    // Cold: nothing counted yet.
    let first = db.list_notebooks().await.expect("list");
    assert_eq!(first[0].note_count, 0, "new notebook has no notes");

    // Warm: this read is served from the cache, and must still be right.
    assert_eq!(
        db.list_notebooks().await.expect("list")[0].note_count,
        0,
        "cached read agrees with the cold one"
    );

    db.add_note(&note("note")).await.expect("add note");
    assert_eq!(
        db.list_notebooks().await.expect("list")[0].note_count,
        1,
        "a write must invalidate the cache — this is the whole contract"
    );

    // Reports are counted out of the note total, not added to it.
    db.add_note(&note("report")).await.expect("add report");
    let after = db.list_notebooks().await.expect("list");
    assert_eq!(after[0].note_count, 1, "report is not a note");
    assert_eq!(after[0].report_count, 1, "report counted as a report");

    // A second Db over the same directory reads the persisted cache — the
    // path a cold app start takes.
    let reopened = Db::open(&dir).await.expect("reopen db");
    let cold = reopened.list_notebooks().await.expect("list");
    assert_eq!(cold[0].note_count, 1, "counts survive a reopen");
    assert_eq!(cold[0].report_count, 1, "report counts survive a reopen");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Chat and MCP both commission overnight work, and both go through
/// `build_commission` so a given request can never queue two different rows
/// depending on which mouth asked. These are the properties that divergence
/// used to break: the IPC copy that lived here before skipped kind validation
/// entirely, so a typo'd kind queued a run that wasted the night.
#[test]
fn a_commission_is_built_the_same_way_whoever_asks() {
    use crate::commands::build_commission;

    // A kind nothing can run is refused, not coerced into "custom".
    assert!(
        build_commission("nb", "Deep read", "not-a-generator", "", Some("now")).is_err(),
        "an unrunnable kind must refuse rather than silently substitute"
    );

    // "now" means the next pass; anything else means tonight.
    let now_run = build_commission("nb", "Deep read", "custom", "dig in", Some("now"))
        .expect("custom with a prompt is runnable");
    assert_eq!(now_run.not_before, 0, "\"now\" starts on the next pass");
    assert_eq!(now_run.trigger, "once");
    assert!(now_run.enabled);
    assert_eq!(now_run.last_run_at, 0, "a fresh commission has never run");

    for when in [Some("tonight"), None, Some("whenever")] {
        let queued = build_commission("nb", "Deep read", "custom", "dig in", when)
            .expect("runnable regardless of when");
        assert!(
            queued.not_before > 0,
            "{when:?} must wait for the night, never start immediately"
        );
    }

    // An unnamed commission still gets a name — an empty label in the record
    // is worse than a generic one.
    let unnamed = build_commission("nb", "   ", "custom", "dig in", Some("now"))
        .expect("a blank name is not a refusal");
    assert_eq!(unnamed.name, "Commissioned run");
}

/// The gap query's lexical gate (`commands::gap_gate`): an empty pool, a
/// linking question, and two unmentioned terms open it; a single-subject
/// question the pool already answers keeps the model call out of the path.
#[test]
fn gap_gate_opens_only_for_linked_or_unmentioned_subjects() {
    let cite = |snippet: &str| crate::models::Citation {
        chunk_id: "c".into(),
        source_id: "s".into(),
        source_title: "Home Network Guide".into(),
        source_path: String::new(),
        note_id: String::new(),
        gist: false,
        snote: false,
        ordinal: 0,
        snippet: snippet.into(),
        distance: 0.0,
        section: String::new(),
    };
    let pool = vec![cite(
        "Guests join the wifi through the captive portal; the passphrase rotates monthly.",
    )];
    assert_eq!(crate::commands::gap_gate("anything", &[]), Some("empty"));
    assert_eq!(
        crate::commands::gap_gate("how do guests get on the wifi?", &pool),
        None,
        "one subject the pool covers"
    );
    assert_eq!(
        crate::commands::gap_gate("compare guest wifi at home versus the office", &pool),
        Some("linked")
    );
    assert_eq!(
        crate::commands::gap_gate("what was the passphrase policy from 2024 to 2025?", &pool),
        Some("range")
    );
    assert_eq!(
        crate::commands::gap_gate(
            "what does the sourdough starter schedule say about hydration?",
            &pool
        ),
        Some("uncovered"),
        "two subjects the pool never mentions"
    );
}

/// A stuck Ollama runner (`Ollama::run_stream`'s deadlines) reads as a
/// Terminal fix the error row can offer — `ollama stop <model>` — and that
/// command passes the Terminal allowlist while an unsafe name does not.
#[test]
fn stuck_ollama_reads_as_a_stop_fix() {
    let advice = crate::commands::classify_model_error(
        "ollama: gemma4:12b-mlx sent nothing for 90s (model was loaded) — it looks stuck",
    )
    .expect("classified");
    assert!(
        advice.contains("run `ollama stop gemma4:12b-mlx`"),
        "{advice}"
    );
    assert!(advice.contains("Settings → Models"), "{advice}");
    let stalled = crate::commands::classify_model_error(
        "ollama: bonsai stalled mid-answer for 30s — it looks stuck",
    )
    .expect("classified");
    assert!(stalled.contains("`ollama stop bonsai`"), "{stalled}");
    assert!(crate::commands::terminal_command_allowed(
        "ollama stop gemma4:12b-mlx"
    ));
    assert!(!crate::commands::terminal_command_allowed(
        "ollama stop x; rm -rf ~"
    ));
}

/// `gap_query_from_reply`: NONE anywhere, a parroted instruction, and a
/// restated question all read as "no gap"; a targeted query passes.
#[test]
fn gap_reply_guards_reject_none_parrots_and_restatements() {
    let q = "compare how guests get wifi at home versus in the office";
    let gap = crate::commands::gap_query_from_reply;
    assert_eq!(gap(q, "NONE"), None);
    assert_eq!(
        gap(q, "Everything needed is here, so reply NONE instead."),
        None
    );
    assert_eq!(
        gap(
            q,
            "ONLY the search query text for the missing evidence — a search that merely \
             restates the question is useless; reply NONE instead"
        ),
        None,
        "the prompt's own instruction is not a query"
    );
    assert_eq!(
        gap(q, "how do guests get wifi at home versus in the office?"),
        None,
        "restatement"
    );
    assert_eq!(
        gap(q, "\"office guest network passphrase\".").as_deref(),
        Some("office guest network passphrase")
    );
}
/// A notebook's worth of concepts for the bundle-writer tests: two sources
/// and two notes, one of which names a source in its prose so the link graph
/// has an edge to turn into `sources:` provenance.
fn okf_fixture() -> (Vec<crate::okf::OkfConcept>, Vec<crate::okf::OkfConcept>) {
    use crate::okf::OkfConcept;
    let sources = vec![
        OkfConcept {
            id: "src-orders".into(),
            title: "Orders table".into(),
            content: "The orders table holds one row per placed order.".into(),
            type_label: "Source".into(),
            resource: "https://example.com/orders".into(),
            tags: vec!["url".into()],
            generated_at: 1_756_000_000_000,
            generated_by: "alchemy/test".into(),
            status: String::new(),
            derived_from: Vec::new(),
            alchemy: vec![
                ("id".into(), "src-orders".into()),
                ("source_type".into(), "url".into()),
                ("tags".into(), "billing ops".into()),
                ("author".into(), "Ops team".into()),
                ("image_url".into(), "https://example.com/o.png".into()),
            ],
            parent: String::new(),
            origin_uri: String::new(),
            reference: None,
            extra: serde_yaml_ng::Mapping::new(),
        },
        OkfConcept {
            id: "src-refunds".into(),
            title: "Refunds policy".into(),
            content: "Refunds are issued within 30 days.".into(),
            type_label: "Source".into(),
            resource: "file:///tmp/refunds.md".into(),
            tags: vec!["markdown".into()],
            generated_at: 1_756_000_100_000,
            generated_by: "alchemy/test".into(),
            status: String::new(),
            derived_from: Vec::new(),
            alchemy: vec![
                ("id".into(), "src-refunds".into()),
                ("source_type".into(), "markdown".into()),
            ],
            // A child of the folder source above, so the bundle records the
            // shape a folder gave the notebook.
            parent: "src-orders".into(),
            origin_uri: String::new(),
            reference: None,
            extra: serde_yaml_ng::Mapping::new(),
        },
    ];
    let notes = vec![
        OkfConcept {
            id: "note-summary".into(),
            title: "What the data says".into(),
            content: "Drawn from the Orders table and the Refunds policy.".into(),
            type_label: "Summary".into(),
            resource: String::new(),
            tags: Vec::new(),
            generated_at: 1_756_000_200_000,
            generated_by: "alchemy/test".into(),
            status: "draft".into(),
            derived_from: vec!["src-orders".into(), "src-refunds".into()],
            alchemy: vec![
                ("id".into(), "note-summary".into()),
                ("kind".into(), "summary".into()),
                ("origin".into(), "auto".into()),
            ],
            parent: String::new(),
            origin_uri: String::new(),
            reference: None,
            extra: serde_yaml_ng::Mapping::new(),
        },
        OkfConcept {
            id: "note-retired".into(),
            title: "Old thinking".into(),
            content: "Superseded.".into(),
            type_label: "Note".into(),
            resource: String::new(),
            tags: Vec::new(),
            generated_at: 1_756_000_300_000,
            // A note a person wrote, so the file says so.
            generated_by: "human:tester".into(),
            status: "deprecated".into(),
            derived_from: Vec::new(),
            alchemy: vec![
                ("id".into(), "note-retired".into()),
                ("kind".into(), "note".into()),
                ("status".into(), "archived".into()),
            ],
            parent: String::new(),
            origin_uri: String::new(),
            reference: None,
            extra: serde_yaml_ng::Mapping::new(),
        },
    ];
    (sources, notes)
}

/// The notebook a bundle describes, with an identity worth round-tripping.
fn okf_notebook(title: &str) -> crate::okf::OkfNotebook {
    crate::okf::OkfNotebook {
        id: "nb-data".into(),
        title: title.into(),
        color: "#5e6ad2".into(),
        icon: "beaker".into(),
        generated_at: 1_756_000_400_000,
    }
}

/// Where a test bundle's manifest lives. Beside the bundle, never inside
/// it: since §5.6 a bundle carries no machine state at all.
fn okf_manifest(bundle: &std::path::Path) -> std::path::PathBuf {
    bundle.with_extension("manifest.json")
}

/// A concept with nothing set, for tests that care about one field.
fn okf_blank_concept() -> crate::okf::OkfConcept {
    crate::okf::OkfConcept {
        id: String::new(),
        title: String::new(),
        content: String::new(),
        type_label: "Source".into(),
        resource: String::new(),
        tags: Vec::new(),
        generated_at: 0,
        generated_by: String::new(),
        status: String::new(),
        derived_from: Vec::new(),
        alchemy: Vec::new(),
        parent: String::new(),
        origin_uri: String::new(),
        reference: None,
        extra: serde_yaml_ng::Mapping::new(),
    }
}

fn okf_scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("alchemy-okf-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// The golden bundle (docs/RFC-okf-live.md §3): every v0.2 frontmatter family
/// present, timestamps `Z`-suffixed, and every `sources:` path resolving to a
/// file that is actually in the bundle.
#[test]
fn okf_bundle_is_v02() {
    use crate::okf::{parse_okf_doc, write_bundle};
    let dir = okf_scratch("golden");
    let bundle = dir.join("data-notebook");
    let (sources, notes) = okf_fixture();
    let written = write_bundle(
        &okf_notebook("Data notebook"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("write bundle");
    assert_eq!((written.sources, written.notes), (2, 2));

    // Sources: resource, tags, and a generated block naming this build.
    let orders = std::fs::read_to_string(bundle.join("sources/orders-table.md")).expect("orders");
    let doc = parse_okf_doc(&orders);
    assert_eq!(doc.str("type").as_deref(), Some("Source"));
    assert_eq!(doc.str("title").as_deref(), Some("Orders table"));
    assert_eq!(
        doc.str("resource").as_deref(),
        Some("https://example.com/orders")
    );
    assert_eq!(doc.tags(), vec!["url".to_string()]);
    // The by-line is the concept's own actor, not a constant: since §5.6 it
    // says whether a person or the app made this version.
    assert_eq!(
        doc.nested("generated", "by").as_deref(),
        Some("alchemy/test")
    );
    let at = doc.nested("generated", "at").expect("generated.at");
    assert!(at.ends_with('Z'), "timestamps carry an explicit Z: {at}");
    let stamp = doc.str("timestamp").expect("timestamp");
    assert!(
        stamp.ends_with('Z'),
        "timestamps carry an explicit Z: {stamp}"
    );
    assert!(doc.body.starts_with("The orders table"));
    // Everything the spec has no field for rides under `alchemy:`, where it
    // collides with nothing and a reader that does not know it skips it.
    assert_eq!(doc.nested("alchemy", "id").as_deref(), Some("src-orders"));
    assert_eq!(doc.nested("alchemy", "source_type").as_deref(), Some("url"));
    assert_eq!(
        doc.nested("alchemy", "tags").as_deref(),
        Some("billing ops")
    );
    assert_eq!(doc.nested("alchemy", "author").as_deref(), Some("Ops team"));
    assert_eq!(
        doc.nested("alchemy", "image_url").as_deref(),
        Some("https://example.com/o.png")
    );
    assert!(
        doc.nested("alchemy", "parent").is_none(),
        "a top-level source has no parent"
    );
    // A folder child names its parent by slug, so the shape a folder gave
    // the notebook survives.
    let child = std::fs::read_to_string(bundle.join("sources/refunds-policy.md")).expect("refunds");
    assert_eq!(
        parse_okf_doc(&child).nested("alchemy", "parent").as_deref(),
        Some("orders-table")
    );

    // Notes: status per origin, and provenance that resolves in the bundle.
    let summary =
        std::fs::read_to_string(bundle.join("notes/what-the-data-says.md")).expect("summary");
    let doc = parse_okf_doc(&summary);
    assert_eq!(doc.str("type").as_deref(), Some("Summary"));
    assert_eq!(doc.str("status").as_deref(), Some("draft"));
    assert_eq!(doc.nested("alchemy", "kind").as_deref(), Some("summary"));
    assert_eq!(doc.nested("alchemy", "origin").as_deref(), Some("auto"));
    // `alchemy:` is a key Alchemy owns, so it never lands in the
    // carry-through set — otherwise the manifest would hold a stale copy and
    // the next write would emit the block twice.
    assert!(
        !doc.extra().keys().any(|k| k.as_str() == Some("alchemy")),
        "the namespace is ours, not an outside key"
    );
    let listed = doc.get("sources").expect("sources block");
    let entries = listed.as_sequence().expect("sources is a list");
    assert_eq!(entries.len(), 2);
    for entry in entries {
        let path = entry
            .get("resource")
            .and_then(|v| v.as_str())
            .expect("resource path");
        assert!(
            bundle.join(path).exists(),
            "sources: path resolves inside the bundle: {path}"
        );
        assert!(entry.get("id").and_then(|v| v.as_str()).is_some());
        assert!(entry.get("title").and_then(|v| v.as_str()).is_some());
    }

    let retired = std::fs::read_to_string(bundle.join("notes/old-thinking.md")).expect("retired");
    assert_eq!(
        parse_okf_doc(&retired).str("status").as_deref(),
        Some("deprecated")
    );

    // Listings and the log.
    let index = std::fs::read_to_string(bundle.join("index.md")).expect("index");
    assert!(index.contains("[Orders table](sources/orders-table.md)"));
    assert!(index.contains("[What the data says](notes/what-the-data-says.md)"));
    // The root index carries frontmatter of its own, so the notebook's
    // identity survives the round trip rather than being guessed from the H1.
    let root = parse_okf_doc(&index);
    assert_eq!(root.str("type").as_deref(), Some("Notebook"));
    assert_eq!(root.str("title").as_deref(), Some("Data notebook"));
    assert_eq!(root.nested("alchemy", "id").as_deref(), Some("nb-data"));
    assert_eq!(root.nested("alchemy", "color").as_deref(), Some("#5e6ad2"));
    assert_eq!(root.nested("alchemy", "icon").as_deref(), Some("beaker"));
    assert!(
        root.body.starts_with("# Data notebook"),
        "the listing still follows the frontmatter: {}",
        root.body
    );
    let log = std::fs::read_to_string(bundle.join("log.md")).expect("log");
    assert!(log.starts_with("# Log\n"));
    assert!(
        log.contains("4 written"),
        "the log names what changed:\n{log}"
    );
    assert!(log.contains("2 sources, 2 notes."), "and the total:\n{log}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A bundle rewritten every night reads as a history, and stops carrying
/// concepts the notebook no longer has (§3, §5.2).
#[test]
fn okf_rewrite_appends_the_log_and_drops_orphans() {
    use crate::okf::write_bundle;
    let dir = okf_scratch("rewrite");
    let bundle = dir.join("data-notebook");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("Data notebook"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("first write");

    // The second night: one source is gone.
    let fewer = vec![sources[0].clone()];
    write_bundle(
        &okf_notebook("Data notebook"),
        &fewer,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("second write");

    assert!(bundle.join("sources/orders-table.md").exists());
    assert!(
        !bundle.join("sources/refunds-policy.md").exists(),
        "a deleted source leaves no orphan behind"
    );
    let log = std::fs::read_to_string(bundle.join("log.md")).expect("log");
    assert_eq!(
        log.matches("- ").count(),
        2,
        "the log accumulates rather than being rewritten:\n{log}"
    );
    assert_eq!(
        log.matches("## ").count(),
        1,
        "two writes on one day share a heading:\n{log}"
    );
    // The note that cited the removed source no longer points at a file that
    // is not there.
    let summary =
        std::fs::read_to_string(bundle.join("notes/what-the-data-says.md")).expect("summary");
    assert!(!summary.contains("sources/refunds-policy.md"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The v0.1 bundles Alchemy already wrote still parse, and a v0.2 file's
/// nested and unknown keys survive the reader (§3, round-trip).
#[test]
fn okf_reads_v01_and_preserves_unknown_keys() {
    use crate::okf::parse_okf_doc;

    // Exactly what the v0.1 exporter wrote: quoted scalars, an inline tag
    // list, and a bare unquoted timestamp.
    let v01 = "---\ntype: Source\ntitle: \"Orders table\"\ndescription: \"One row per order\"\nresource: \"https://example.com/orders\"\ntags: [url]\ntimestamp: 2026-08-24T09:00:00Z\n---\n\nThe orders table.\n";
    let doc = parse_okf_doc(v01);
    assert_eq!(doc.str("title").as_deref(), Some("Orders table"));
    assert_eq!(
        doc.str("resource").as_deref(),
        Some("https://example.com/orders")
    );
    assert_eq!(doc.tags(), vec!["url".to_string()]);
    assert_eq!(doc.body.trim(), "The orders table.");
    assert!(doc.extra().is_empty(), "v0.1 writes nothing we don't own");
    // No `alchemy:` block, and the readers fall back rather than failing:
    // the source type comes off the spec-facing `tags:`, and there is simply
    // no user tag, author, or cover to restore.
    assert!(doc.nested("alchemy", "source_type").is_none());
    assert!(doc.nested("alchemy", "tags").is_none());
    assert!(doc.nested("alchemy", "id").is_none());

    // A v0.1 root index.md has no frontmatter at all, so the notebook's title
    // still comes off the H1 and its colour and icon are simply not there.
    let v01_index = "# Orders notebook\n\nA research notebook exported from Alchemy as an Open Knowledge Format bundle.\n\n# Sources\n\n- [Orders table](sources/orders-table.md) — One row per order\n";
    let root = parse_okf_doc(v01_index);
    assert!(root.str("title").is_none(), "no frontmatter to read");
    assert!(root.nested("alchemy", "color").is_none());
    assert_eq!(
        root.body
            .lines()
            .find(|l| l.starts_with("# "))
            .map(|l| l[2..].trim()),
        Some("Orders notebook"),
        "the H1 fallback still names the notebook"
    );

    // And a v0.2 note without an `alchemy:` block still resolves its kind
    // from the human `type:` label, which is the pre-namespace behaviour.
    let note = "---\ntype: Study Guide\ntitle: \"Guide\"\n---\n\nBody.\n";
    let doc = parse_okf_doc(note);
    assert!(doc.nested("alchemy", "kind").is_none());
    assert_eq!(
        crate::commands::note_kind_from_label(doc.str("type").as_deref().unwrap_or("Note")),
        "study_guide"
    );

    // A v0.2 file from somewhere else: nested maps, a list of verified
    // entries, and a key Alchemy has never heard of.
    let v02 = "---\ntype: Source\ntitle: \"Orders table\"\ngenerated:\n  by: \"okf-pipeline/2.1\"\n  at: \"2026-08-24T09:00:00Z\"\nverified:\n  - by: \"reviewer@example.com\"\n    at: \"2026-08-25T10:00:00Z\"\nstale_after: \"2027-01-01T00:00:00Z\"\nconfidence: 0.8\n---\n\nBody.\n";
    let doc = parse_okf_doc(v02);
    assert_eq!(
        doc.nested("generated", "by").as_deref(),
        Some("okf-pipeline/2.1")
    );
    let extra = doc.extra();
    let keys: Vec<String> = extra
        .keys()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();
    assert!(keys.contains(&"verified".to_string()), "got {keys:?}");
    assert!(keys.contains(&"stale_after".to_string()), "got {keys:?}");
    assert!(keys.contains(&"confidence".to_string()), "got {keys:?}");
    let verified = extra
        .get(serde_yaml_ng::Value::String("verified".into()))
        .and_then(|v| v.as_sequence())
        .expect("verified survives as a list");
    assert_eq!(verified.len(), 1);

    // Frontmatter that is not valid YAML still gives up its title rather
    // than taking the document down with it.
    let broken = "---\ntitle: \"Half quoted\ntype: Note\n---\n\nBody.\n";
    assert_eq!(
        parse_okf_doc(broken).str("type").as_deref(),
        Some("Note"),
        "the quoted-scalar fallback still reads what it can"
    );
}

/// Unknown keys make it back out again: what an outside editor put in the
/// file is re-emitted verbatim on the next write (§3, §5.2).
#[test]
fn okf_writes_unknown_keys_back_out() {
    use crate::okf::{parse_okf_doc, write_bundle, OkfConcept};
    let dir = okf_scratch("preserve");
    let bundle = dir.join("nb");
    let mut extra = serde_yaml_ng::Mapping::new();
    extra.insert(
        serde_yaml_ng::Value::String("stale_after".into()),
        serde_yaml_ng::Value::String("2027-01-01T00:00:00Z".into()),
    );
    let concept = OkfConcept {
        id: "s1".into(),
        title: "Kept".into(),
        content: "Body.".into(),
        type_label: "Source".into(),
        resource: String::new(),
        tags: Vec::new(),
        generated_at: 1_756_000_000_000,
        generated_by: String::new(),
        status: String::new(),
        derived_from: Vec::new(),
        alchemy: Vec::new(),
        parent: String::new(),
        origin_uri: String::new(),
        reference: None,
        extra,
    };
    write_bundle(
        &okf_notebook("NB"),
        &[concept],
        &[],
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("write");
    let text = std::fs::read_to_string(bundle.join("sources/kept.md")).expect("read");
    let doc = parse_okf_doc(&text);
    assert_eq!(
        doc.str("stale_after").as_deref(),
        Some("2027-01-01T00:00:00Z")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A bundle's listings are not its knowledge (docs/RFC-okf-live.md §4):
/// `index.md` and `log.md` at any level are the table of contents and the
/// history, and neither ever becomes a source.
#[test]
fn okf_reserved_files_never_ingest() {
    use crate::okf::is_okf_reserved;
    assert!(is_okf_reserved("/bundle/index.md"));
    assert!(is_okf_reserved("/bundle/log.md"));
    assert!(is_okf_reserved("/bundle/sources/index.md"));
    assert!(is_okf_reserved("/bundle/notes/index.md"));
    // Concepts, not listings — including ones whose names merely contain them.
    assert!(!is_okf_reserved("/bundle/sources/orders.md"));
    assert!(!is_okf_reserved("/bundle/notes/changelog.md"));
    assert!(!is_okf_reserved("/bundle/sources/index-of-terms.md"));
}

/// Lifecycle and trust read out of a concept's own frontmatter (§4).
#[test]
fn okf_lifecycle_reads_status_staleness_and_trust() {
    use crate::okf::{okf_lifecycle_of, parse_okf_doc, OkfLifecycle};

    let plain = okf_lifecycle_of(&parse_okf_doc(
        "---\ntype: Source\ntitle: \"Plain\"\n---\n\nBody.\n",
    ));
    assert_eq!(
        plain,
        OkfLifecycle::default(),
        "a bare concept says nothing"
    );

    let retired = okf_lifecycle_of(&parse_okf_doc(
        "---\ntitle: \"Old\"\nstatus: deprecated\nstale_after: \"2026-01-01T00:00:00Z\"\n---\n\nBody.\n",
    ));
    assert_eq!(retired.status, "deprecated");
    assert_eq!(
        retired.stale_after,
        chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("parse")
            .timestamp_millis()
    );

    // A `by` written name/version is a tool; anything else is a person, and
    // a human review outranks a machine one however they are ordered.
    let machine = okf_lifecycle_of(&parse_okf_doc(
        "---\ntitle: \"T\"\nverified:\n  - by: \"okf-lint/1.2\"\n---\n\nBody.\n",
    ));
    assert_eq!(machine.trust, "machine");
    let both = okf_lifecycle_of(&parse_okf_doc(
        "---\ntitle: \"T\"\nverified:\n  - by: \"reviewer@example.com\"\n  - by: \"okf-lint/1.2\"\n---\n\nBody.\n",
    ));
    assert_eq!(
        both.trust, "human",
        "a person's review is the stronger claim"
    );
}

/// Frontmatter is provenance, not prose (§4): it never embeds as body text,
/// but what it says about the document rides every chunk's prefix.
#[test]
fn okf_frontmatter_rides_the_embed_prefix() {
    let extracted = crate::ingest::Extracted {
        feeds: Vec::new(),
        image_url: String::new(),
        author: String::new(),
        title: "Orders table".into(),
        source_type: "markdown".into(),
        url: String::new(),
        text: "---\ntype: Source\ntitle: \"Orders table\"\ndescription: \"One row per placed order\"\ntags: [url]\n---\n\n# Orders\n\nThe orders table holds one row per placed order.\n".into(),
    };
    let chunks = crate::ingest::chunk_source(&extracted, None);
    assert!(!chunks.is_empty());
    let embed = &chunks[0].embed_text;
    assert!(embed.contains("One row per placed order"), "got: {embed}");
    assert!(embed.contains("#url"), "tags still ride along: {embed}");
    assert!(
        !chunks[0].text.contains("description:"),
        "the frontmatter itself is never body text: {}",
        chunks[0].text
    );
}

/// The second write of an unchanged notebook touches nothing (§7) — the
/// property that makes a bound notebook safe to keep in git.
#[test]
fn okf_unchanged_write_is_a_no_op() {
    use crate::okf::write_bundle;
    let dir = okf_scratch("noop");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();

    let first = write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("first");
    assert_eq!(first.written, 4, "the seed pass writes everything");
    let stamps: Vec<std::time::SystemTime> = ["sources/orders-table.md", "notes/old-thinking.md"]
        .iter()
        .map(|p| {
            std::fs::metadata(bundle.join(p))
                .and_then(|m| m.modified())
                .expect("mtime")
        })
        .collect();

    let second = write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("second");
    assert_eq!(
        (second.written, second.moved, second.removed),
        (0, 0, 0),
        "nothing changed, so nothing is rewritten"
    );
    assert!(!second.changed());
    for (path, before) in ["sources/orders-table.md", "notes/old-thinking.md"]
        .iter()
        .zip(stamps)
    {
        let after = std::fs::metadata(bundle.join(path))
            .and_then(|m| m.modified())
            .expect("mtime");
        assert_eq!(after, before, "{path} was not rewritten");
    }
    // And a no-op pass says nothing in the log.
    let log = std::fs::read_to_string(bundle.join("log.md")).expect("log");
    assert_eq!(
        log.matches("- ").count(),
        1,
        "one entry, from the seed:\n{log}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Retitling a concept moves its file rather than deleting and recreating it,
/// and the manifest follows (§7).
#[test]
fn okf_retitle_moves_the_file() {
    use crate::okf::{load_manifest, write_bundle};
    let dir = okf_scratch("rename");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("first");
    assert!(bundle.join("notes/old-thinking.md").exists());

    let mut renamed = notes.clone();
    renamed[1].title = "New thinking".into();
    let out = write_bundle(
        &okf_notebook("NB"),
        &sources,
        &renamed,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("second");
    assert_eq!(out.moved, 1, "one rename(2), not a delete and an add");
    assert_eq!(out.removed, 0, "a move is never a removal");
    assert!(!bundle.join("notes/old-thinking.md").exists());
    assert!(bundle.join("notes/new-thinking.md").exists());

    let manifest = load_manifest(&okf_manifest(&bundle));
    assert_eq!(
        manifest.concepts["note-retired"].path, "notes/new-thinking.md",
        "the manifest followed the file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Keys an outside editor added survive every write since, not just the one
/// that read them (§7, preservation).
#[test]
fn okf_outside_keys_survive_later_writes() {
    use crate::okf::{load_manifest, parse_okf_doc, write_bundle, OkfManifestEntry};
    let dir = okf_scratch("preserve2");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");

    // Stand in for the reconciler: an outside edit added `verified:`, and the
    // manifest is where it is remembered.
    let mut manifest = load_manifest(&okf_manifest(&bundle));
    let mut extra = serde_yaml_ng::Mapping::new();
    let mut entry = serde_yaml_ng::Mapping::new();
    entry.insert(
        serde_yaml_ng::Value::String("by".into()),
        serde_yaml_ng::Value::String("reviewer@example.com".into()),
    );
    extra.insert(
        serde_yaml_ng::Value::String("verified".into()),
        serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::Mapping(entry)]),
    );
    manifest.concepts.insert(
        "src-orders".into(),
        OkfManifestEntry {
            path: "sources/orders-table.md".into(),
            hash: String::new(),
            wrote_at: 0,
            extra,
        },
    );
    let json = serde_json::to_string(&manifest).expect("json");
    std::fs::write(okf_manifest(&bundle), json).expect("write manifest");

    // Two more writes: the key is still there after both.
    for pass in 1..=2 {
        write_bundle(
            &okf_notebook("NB"),
            &sources,
            &notes,
            &bundle,
            Some(&okf_manifest(&bundle)),
        )
        .expect("rewrite");
        let text = std::fs::read_to_string(bundle.join("sources/orders-table.md")).expect("read");
        let doc = parse_okf_doc(&text);
        assert!(
            doc.get("verified").is_some(),
            "pass {pass} dropped the outside key:\n{text}"
        );
        assert_eq!(
            doc.str("title").as_deref(),
            Some("Orders table"),
            "and Alchemy's own keys still lead"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A document someone else left in the bundle is not Alchemy's to remove
/// (§5.2): only files the manifest claims are ever deleted.
#[test]
fn okf_leaves_files_it_did_not_write() {
    use crate::okf::write_bundle;
    let dir = okf_scratch("foreign");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");

    std::fs::write(bundle.join("notes/theirs.md"), "# Not ours\n").expect("write");
    let fewer = vec![sources[0].clone()];
    write_bundle(
        &okf_notebook("NB"),
        &fewer,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("second");

    assert!(
        bundle.join("notes/theirs.md").exists(),
        "a file Alchemy never wrote stays"
    );
    assert!(
        !bundle.join("sources/refunds-policy.md").exists(),
        "a file it did write, for a source that is gone, does not"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The bindings sidecar is per-machine state, and unbinding removes the
/// record without touching anything else (§5.1).
#[test]
fn okf_bindings_round_trip() {
    use crate::okf::{binding_for, load_bindings, set_binding, OkfBinding};
    let dir = okf_scratch("bindings");
    assert!(binding_for(&dir, "nb1").is_none());

    set_binding(
        &dir,
        "nb1",
        Some(OkfBinding {
            path: "/tmp/one".into(),
            id: "bind-one".into(),
            last_write_at: 42,
        }),
    );
    set_binding(
        &dir,
        "nb2",
        Some(OkfBinding {
            path: "/tmp/two".into(),
            id: "bind-two".into(),
            last_write_at: 0,
        }),
    );
    assert_eq!(binding_for(&dir, "nb1").expect("nb1").path, "/tmp/one");
    assert_eq!(load_bindings(&dir).len(), 2);

    set_binding(&dir, "nb1", None);
    assert!(binding_for(&dir, "nb1").is_none(), "unbound");
    assert!(binding_for(&dir, "nb2").is_some(), "and only that one");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A write-through never looks like an outside edit (§7, echo): every file
/// the writer just laid down classifies as our own echo, so the watcher event
/// it causes reconciles to nothing.
#[test]
fn okf_write_through_never_reads_back() {
    use crate::okf::{classify, load_manifest, okf_hash, write_bundle, OkfAction};
    let dir = okf_scratch("echo");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");
    let manifest = load_manifest(&okf_manifest(&bundle));

    for rel in [
        "sources/orders-table.md",
        "sources/refunds-policy.md",
        "notes/what-the-data-says.md",
        "notes/old-thinking.md",
    ] {
        let text = std::fs::read_to_string(bundle.join(rel)).expect(rel);
        assert_eq!(
            classify(rel, &okf_hash(&text), &manifest),
            OkfAction::Echo,
            "{rel} should read as our own write"
        );
    }

    // An outside edit to one of them is not an echo — it is an update to the
    // entity the manifest already knows.
    let rel = "notes/what-the-data-says.md";
    let edited = format!(
        "{}\n\nSomeone appended a line.\n",
        std::fs::read_to_string(bundle.join(rel)).expect("read")
    );
    match classify(rel, &okf_hash(&edited), &manifest) {
        OkfAction::Update(id) => assert_eq!(id, "note-summary"),
        other => panic!("expected an update, got {other:?}"),
    }

    // And a file nobody wrote is somebody's new document.
    assert_eq!(
        classify("notes/theirs.md", "whatever", &manifest),
        OkfAction::Create
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Last writer wins by clock, and a tie goes to disk (§5.4).
#[test]
fn okf_conflicts_go_to_the_newer_side() {
    use crate::okf::disk_wins;
    assert!(disk_wins(2_000, 1_000), "the file is newer");
    assert!(!disk_wins(1_000, 2_000), "the entity is newer");
    assert!(
        disk_wins(1_000, 1_000),
        "a tie goes to the file: it is what someone just saved"
    );
}

/// The hash is stable across runs, which is the whole basis of echo
/// suppression — a `DefaultHasher` would not be.
#[test]
fn okf_hash_is_stable_and_content_sensitive() {
    use crate::okf::okf_hash;
    assert_eq!(okf_hash("hello"), okf_hash("hello"));
    assert_ne!(okf_hash("hello"), okf_hash("hello "));
    assert_eq!(okf_hash(""), "cbf29ce484222325", "the FNV-1a offset basis");
    assert_eq!(okf_hash("a").len(), 16);
}

/// A bundle someone ran `ok init` in: `.ok/`, `.claude/`, `.github/`,
/// `.mcp.json`, `opencode.json`, and the rest. None of it is knowledge, and
/// only `sources/**.md` and `notes/**.md` are read (RFC-okf-live §4).
#[test]
fn okf_skips_openknowledge_scaffolding() {
    use crate::okf::is_okf_concept;
    let root = std::path::Path::new("/bundle");
    let concept = |rel: &str| is_okf_concept(root, &format!("/bundle/{rel}"));

    assert!(concept("sources/orders.md"));
    assert!(concept("notes/summary.md"));
    // Nested is still knowledge; `sources/**.md` is the rule, not one level.
    assert!(concept("sources/tables/orders.md"));

    // Listings, not concepts.
    assert!(!concept("index.md"));
    assert!(!concept("log.md"));
    assert!(!concept("sources/index.md"));
    assert!(!concept("notes/index.md"));

    // Everything `ok init` adds. The dot entries and the plain ones alike:
    // a .gitignore would have caught almost none of these.
    for rel in [
        ".ok/config.yml",
        ".ok/local/state.json",
        ".okignore",
        ".claude/skills/open-knowledge/SKILL.md",
        ".codex/config.toml",
        ".cursor/rules.md",
        ".pi/config.json",
        ".opencode/agent.md",
        ".github/workflows/ok.yml",
        ".mcp.json",
        "opencode.json",
        ".gitignore",
        ".alchemy/manifest.json",
    ] {
        assert!(!concept(rel), "{rel} is tooling, not knowledge");
    }

    // Nor is anything else at the root, whatever its extension.
    assert!(!concept("README.md"));
    assert!(!concept("sources/notes.txt"));
    // A hidden directory *under* sources/ is still hidden.
    assert!(!concept("sources/.drafts/wip.md"));
    // And a path outside the bundle never qualifies.
    assert!(!is_okf_concept(root, "/elsewhere/sources/orders.md"));
}

/// The same rule against the real thing: a copy of an exported bundle that
/// was later `ok init`ed. Read-only — the fixture is copied, never bound.
#[test]
fn okf_reads_only_concepts_from_a_real_ok_project() {
    use crate::okf::is_okf_concept;
    let src = std::path::Path::new("/Users/thrashr888/Downloads/ferrari-458-488-research");
    if !src.is_dir() {
        // The fixture is one developer's Downloads folder; skip elsewhere.
        return;
    }
    let root = okf_scratch("okproject").join("bundle");
    let copied = std::process::Command::new("/bin/cp")
        .arg("-R")
        .arg(src)
        .arg(&root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(copied, "copy the fixture aside rather than touching it");

    // Walk it with every ignore mechanism switched off. `ok init` writes its
    // scaffolding into `.git/info/exclude`, which the real walker honours —
    // so a walk that respected it would prove nothing about the rule. The
    // allowlist has to stand on its own: in a folder that is not a git repo,
    // or for the entries OK does not exclude (`.github/`, `.pi/extensions/`),
    // it is the only thing standing between tooling and the corpus.
    let mut kept: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for entry in ignore::WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path().to_string_lossy().to_string();
        let rel = entry
            .path()
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_okf_concept(&root, &path) {
            kept.push(rel);
        } else {
            rejected.push(rel);
        }
    }
    kept.sort();

    assert!(
        kept.iter()
            .all(|r| r.starts_with("sources/") || r.starts_with("notes/")),
        "only sources/ and notes/ survive: {kept:?}"
    );
    assert!(
        kept.iter().any(|r| r.starts_with("sources/")),
        "found sources"
    );
    assert!(kept.iter().any(|r| r.starts_with("notes/")), "found notes");
    assert!(
        !kept
            .iter()
            .any(|r| r.ends_with("index.md") || r.ends_with("log.md")),
        "no listings: {kept:?}"
    );
    // The scaffolding really is present in the fixture, and really is out.
    for tooling in [".mcp.json", "opencode.json", ".okignore", ".gitignore"] {
        assert!(
            rejected.iter().any(|r| r == tooling),
            "{tooling} should be in the fixture and rejected; rejected: {rejected:?}"
        );
    }
    for dir in [".ok/", ".claude/", ".github/", ".pi/", ".git/"] {
        assert!(
            rejected.iter().any(|r| r.starts_with(dir)),
            "{dir} is in the fixture and rejected; rejected: {rejected:?}"
        );
    }

    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}

/// Two Macs, one shared folder (docs/RFC-okf-live.md §5.6). Each install
/// keeps its own manifest outside the bundle, writes under its own log
/// heading, and reads the other's files as changes rather than as echoes.
///
/// The two sides are two manifests and two binding ids driving the same
/// writer and the same classifier against one folder — which is exactly what
/// two installs are. It is not two `AppState`s: one of those needs an `Ai`,
/// a config path, a generation queue and a Tauri handle, none of which a
/// unit test can stand up, and none of which this behaviour depends on.
#[test]
fn okf_two_machines_share_one_folder() {
    use crate::okf::{classify, load_manifest, okf_hash, write_bundle, OkfAction};
    let dir = okf_scratch("twomacs");
    let shared = dir.join("shared-bundle");
    // Two data dirs, the way two Macs have two app-data directories.
    let mac_a = dir.join("mac-a").join("okf").join("binding-a.json");
    let mac_b = dir.join("mac-b").join("okf").join("binding-b.json");

    let (sources, notes) = okf_fixture();
    // A writes first: the seed pass.
    let first = write_bundle(
        &okf_notebook("Shared"),
        &sources,
        &notes,
        &shared,
        Some(&mac_a),
    )
    .expect("A writes");
    assert_eq!(first.written, 4);

    // Each side's record is its own, and neither is in the bundle.
    assert!(mac_a.exists(), "A's manifest lives in A's data dir");
    assert!(!mac_b.exists(), "B has not written yet");
    assert!(
        !shared.join(".alchemy").exists(),
        "a shared bundle carries nothing machine-shaped"
    );

    // B reads the folder. Nothing here is B's own write, so every file is
    // news — B has no record of any of it.
    let manifest_b = load_manifest(&mac_b);
    for rel in ["sources/orders-table.md", "notes/what-the-data-says.md"] {
        let text = std::fs::read_to_string(shared.join(rel)).expect(rel);
        assert_eq!(
            classify(rel, &okf_hash(&text), &manifest_b),
            OkfAction::Create,
            "{rel} is new to B"
        );
        // And it is emphatically not news to A.
        assert_eq!(
            classify(rel, &okf_hash(&text), &load_manifest(&mac_a)),
            OkfAction::Echo,
            "{rel} is A's own write echoing back"
        );
    }

    // B takes the folder on and writes its own pass. Its ids differ from A's
    // — they are its own store's — so its concepts land at the same paths
    // with a manifest of its own.
    let b_sources: Vec<crate::okf::OkfConcept> = sources
        .iter()
        .map(|c| crate::okf::OkfConcept {
            id: format!("b-{}", c.id),
            ..c.clone()
        })
        .collect();
    let mut b_notes: Vec<crate::okf::OkfConcept> = notes
        .iter()
        .map(|c| crate::okf::OkfConcept {
            id: format!("b-{}", c.id),
            ..c.clone()
        })
        .collect();
    // B edits the summary.
    b_notes[0].content = "Edited on the other Mac.".into();
    b_notes[0].generated_by = "human:other".into();
    let second = write_bundle(
        &okf_notebook("Shared"),
        &b_sources,
        &b_notes,
        &shared,
        Some(&mac_b),
    )
    .expect("B writes");
    assert!(second.written > 0, "B's edit reaches the folder");

    let a = load_manifest(&mac_a);
    let b = load_manifest(&mac_b);
    assert!(
        a.concepts.contains_key("note-summary") && b.concepts.contains_key("b-note-summary"),
        "each side keeps its own ids"
    );
    assert!(
        !a.concepts.keys().any(|k| b.concepts.contains_key(k)),
        "and never reads the other's"
    );

    // A now sees B's edit as a change to the entity A already knows, not as
    // an echo and not as a new concept.
    let edited = std::fs::read_to_string(shared.join("notes/what-the-data-says.md")).expect("read");
    match classify("notes/what-the-data-says.md", &okf_hash(&edited), &a) {
        OkfAction::Update(id) => assert_eq!(id, "note-summary"),
        other => panic!("A should see an update, got {other:?}"),
    }
    assert!(edited.contains("Edited on the other Mac."));
    assert!(
        edited.contains("human:other"),
        "and can see who made it: {edited}"
    );

    // The log carries both writers. Same day, same folder, separate blocks —
    // which is what keeps two Macs from racing over one heading.
    let log = std::fs::read_to_string(shared.join("log.md")).expect("log");
    let headings = log.matches("\n## ").count();
    assert_eq!(headings, 1, "one account wrote both passes here:\n{log}");
    assert!(
        log.contains(&format!("\u{2014} {}", crate::okf::okf_account())),
        "the heading names the writer, not just the day:\n{log}"
    );
    assert!(log.matches("- ").count() >= 2, "both passes logged:\n{log}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `log.md` is history, never a concept — a change to it arriving from the
/// other side is text, and the reconciler must not try to make a note of it
/// (§5.6).
#[test]
fn okf_log_is_never_a_concept() {
    use crate::okf::is_okf_concept;
    let root = std::path::Path::new("/bundle");
    assert!(!is_okf_concept(root, "/bundle/log.md"));
    assert!(!is_okf_concept(root, "/bundle/notes/log.md"));
    assert!(!is_okf_concept(root, "/bundle/sources/log.md"));
    // Nor does a second writer's block make it one.
    assert!(!is_okf_concept(root, "/bundle/index.md"));
}

/// A cloud tool resolving a clash writes `<name> (conflicted copy).md` or
/// `<name> 2.md` beside the original. Nothing special-cases those away: they
/// are ordinary new concepts, which is the "keep both" outcome (§5.6).
#[test]
fn okf_conflict_copies_are_ordinary_notes() {
    use crate::okf::{classify, is_okf_concept, OkfManifest};
    let root = std::path::Path::new("/bundle");
    for rel in [
        "notes/old-thinking (conflicted copy).md",
        "notes/old-thinking 2.md",
        "notes/old-thinking (Paul's conflicted copy 2026-09-02).md",
        "sources/orders-table (conflicted copy).md",
    ] {
        assert!(
            is_okf_concept(root, &format!("/bundle/{rel}")),
            "{rel} is a document like any other"
        );
        // And with no manifest entry it reads as a new concept to take in,
        // not as something to reconcile against the original.
        assert_eq!(
            classify(rel, "any-hash", &OkfManifest::default()),
            crate::okf::OkfAction::Create
        );
    }
}

/// Who made a version, from what the store records (§5.6).
#[test]
fn okf_actors_name_a_person_or_the_app() {
    use crate::okf::{okf_actor_is_machine, okf_human, okf_is_ours, okf_writer};

    assert!(okf_human().starts_with("human:"));
    assert!(okf_is_ours(&okf_writer()), "the app is ours");
    assert!(okf_is_ours(&okf_human()), "so is this person");
    assert!(!okf_is_ours("human:kim"), "someone else is not");
    assert!(!okf_is_ours("okf-pipeline/2.1"), "nor is another producer");

    // A machine actor earns a draft; a person's edit does not.
    assert!(okf_actor_is_machine("auto"));
    assert!(okf_actor_is_machine(&okf_writer()));
    assert!(okf_actor_is_machine("okf-pipeline/2.1"));
    assert!(!okf_actor_is_machine("human:kim"));
    assert!(!okf_actor_is_machine(""));
}

/// The by-line reaches the file, and a bundle keeps no machine state.
#[test]
fn okf_files_name_their_author() {
    use crate::okf::{parse_okf_doc, write_bundle};
    let dir = okf_scratch("actors");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("write");

    let imported = std::fs::read_to_string(bundle.join("sources/orders-table.md")).expect("source");
    assert_eq!(
        parse_okf_doc(&imported)
            .nested("generated", "by")
            .as_deref(),
        Some("alchemy/test"),
        "an import is the app's doing"
    );
    let written = std::fs::read_to_string(bundle.join("notes/old-thinking.md")).expect("note");
    assert_eq!(
        parse_okf_doc(&written).nested("generated", "by").as_deref(),
        Some("human:tester"),
        "a note a person wrote says so"
    );

    // §5.6's headline promise.
    assert!(
        !bundle.join(".alchemy").exists(),
        "the bundle carries nothing machine-shaped"
    );
    let stray: Vec<String> = std::fs::read_dir(&bundle)
        .expect("read bundle")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(stray.is_empty(), "no dot entries at all: {stray:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// An in-bundle manifest from this branch's earlier builds is adopted once
/// and the directory removed, so an already-bound folder keeps its hashes
/// and stops carrying machine state (§5.6).
#[test]
fn okf_adopts_and_retires_the_in_bundle_manifest() {
    use crate::okf::{load_manifest, write_bundle};
    let dir = okf_scratch("adopt");
    let bundle = dir.join("nb");
    let legacy_dir = bundle.join(".alchemy");
    let moved_to = dir.join("data").join("okf").join("binding-x.json");

    // Write a bundle, then move its record where the old builds kept it.
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");
    std::fs::create_dir_all(&legacy_dir).expect("mkdir");
    std::fs::rename(okf_manifest(&bundle), legacy_dir.join("manifest.json")).expect("stage");

    crate::okf::adopt_legacy_manifest(&bundle, &moved_to);

    assert!(!legacy_dir.exists(), "the in-bundle copy is gone");
    let adopted = load_manifest(&moved_to);
    assert!(
        adopted.concepts.contains_key("src-orders"),
        "and its hashes came along, so nothing is rewritten"
    );

    // A second write against the adopted record touches nothing.
    let again = write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&moved_to),
    )
    .expect("rewrite");
    assert_eq!((again.written, again.moved, again.removed), (0, 0, 0));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A source as the reference planner reads it.
fn okf_src(id: &str, url: &str) -> crate::models::Source {
    crate::models::Source {
        id: id.into(),
        notebook_id: "nb".into(),
        title: id.into(),
        source_type: "pdf".into(),
        url: url.into(),
        content: String::new(),
        char_count: 0,
        chunk_count: 0,
        created_at: 0,
        status: "ready".into(),
        error: String::new(),
        parent_id: String::new(),
        mtime: 0,
        author: String::new(),
        image_url: String::new(),
        tags: String::new(),
        note: String::new(),
        fetched_at: 0,
        fetch_failures: 0,
    }
}

/// §6's table, decided: whether the bundle is the sensible home for the bytes.
#[test]
fn okf_reference_plan_follows_the_table() {
    use crate::okf::{plan_reference, ReferencePlan};
    let dir = okf_scratch("refplan");
    let bundle = dir.join("nb");
    std::fs::create_dir_all(&bundle).expect("mkdir");
    let cap = 50 * 1024 * 1024;

    // A file the user dragged in: the notebook is its only home in Alchemy,
    // and the other Mac cannot reach this path.
    let pdf = dir.join("paper.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 pretend").expect("write");
    match plan_reference(&okf_src("s1", &pdf.to_string_lossy()), &bundle, cap) {
        ReferencePlan::Copy { name, .. } => {
            assert!(name.ends_with(".pdf"));
            assert_eq!(name.len(), 16 + 4, "sixteen hex characters plus the suffix");
        }
        other => panic!("a dragged PDF copies, got {other:?}"),
    }

    // Pasted text and clipped pages are their own capture; a URL re-fetches.
    assert!(matches!(
        plan_reference(&okf_src("s2", ""), &bundle, cap),
        ReferencePlan::Link { .. }
    ));
    assert!(matches!(
        plan_reference(&okf_src("s3", "https://example.com/a"), &bundle, cap),
        ReferencePlan::Link { .. }
    ));

    // A folder child's parent is the origin and resyncs; copying a synced
    // folder into a synced folder duplicates it forever.
    let mut child = okf_src("s4", &pdf.to_string_lossy());
    child.parent_id = "folder-1".into();
    assert!(matches!(
        plan_reference(&child, &bundle, cap),
        ReferencePlan::Link { .. }
    ));

    // A file already inside the bundle is cited where it lies.
    let inside = bundle.join("attachments").join("here.pdf");
    std::fs::create_dir_all(inside.parent().expect("parent")).expect("mkdir");
    std::fs::write(&inside, b"%PDF here").expect("write");
    assert_eq!(
        plan_reference(&okf_src("s5", &inside.to_string_lossy()), &bundle, cap),
        ReferencePlan::Inside {
            rel: "attachments/here.pdf".into()
        }
    );

    // Over the cap, one video does not make a bundle undeliverable.
    match plan_reference(&okf_src("s6", &pdf.to_string_lossy()), &bundle, 4) {
        ReferencePlan::Link { reason } => assert_eq!(reason, "over the size cap"),
        other => panic!("expected a link, got {other:?}"),
    }
    // And the cap set to zero turns copying off entirely.
    assert!(matches!(
        plan_reference(&okf_src("s7", &pdf.to_string_lossy()), &bundle, 0),
        ReferencePlan::Link { .. }
    ));

    // Markdown and text are their own extraction: copying only duplicates
    // the concept body.
    let md = dir.join("notes.md");
    std::fs::write(&md, b"# hi").expect("write");
    assert!(matches!(
        plan_reference(&okf_src("s8", &md.to_string_lossy()), &bundle, cap),
        ReferencePlan::Link { .. }
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

/// One original per distinct file, written once, and removed only when the
/// last concept pointing at it goes (§6, §8's "Originals" bullet).
#[test]
fn okf_originals_are_written_once_and_shared_by_hash() {
    use crate::okf::{parse_okf_doc, plan_reference, write_bundle, OkfConcept};
    let dir = okf_scratch("refs");
    let bundle = dir.join("nb");
    let pdf = dir.join("paper.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 the same bytes").expect("write");
    let cap = 50 * 1024 * 1024;
    let uri = format!("file://{}", pdf.display());

    let concept = |id: &str, title: &str| OkfConcept {
        id: id.into(),
        title: title.into(),
        content: "Extracted text.".into(),
        type_label: "Source".into(),
        generated_at: 1_756_000_000_000,
        generated_by: "alchemy/test".into(),
        origin_uri: uri.clone(),
        reference: Some(plan_reference(
            &okf_src(id, &pdf.to_string_lossy()),
            &bundle,
            cap,
        )),
        ..okf_blank_concept()
    };

    // Two sources over the same file share one copy.
    let both = vec![concept("s1", "First read"), concept("s2", "Second read")];
    let first = write_bundle(
        &okf_notebook("NB"),
        &both,
        &[],
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("write");
    assert_eq!(first.referenced, 1, "one original, not two");
    let refs: Vec<String> = std::fs::read_dir(bundle.join("references"))
        .expect("references/")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(refs.len(), 1, "one file on disk: {refs:?}");

    // `resource:` says the bytes are here; `alchemy.origin` keeps the path.
    let doc =
        parse_okf_doc(&std::fs::read_to_string(bundle.join("sources/first-read.md")).unwrap());
    let resource = doc.str("resource").expect("resource");
    assert!(resource.starts_with("references/"), "got {resource}");
    assert!(bundle.join(&resource).exists(), "and it resolves");
    assert_eq!(
        doc.nested("alchemy", "origin").as_deref(),
        Some(uri.as_str())
    );
    assert_eq!(
        parse_okf_doc(&std::fs::read_to_string(bundle.join("sources/second-read.md")).unwrap())
            .str("resource")
            .as_deref(),
        Some(resource.as_str()),
        "the second source cites the same original"
    );

    // A second pass adds nothing: a reference that exists is skipped.
    let mtime = std::fs::metadata(bundle.join(&resource))
        .and_then(|m| m.modified())
        .expect("mtime");
    let again = write_bundle(
        &okf_notebook("NB"),
        &both,
        &[],
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("rewrite");
    assert_eq!((again.written, again.removed), (0, 0), "nothing to redo");
    assert_eq!(
        std::fs::metadata(bundle.join(&resource))
            .and_then(|m| m.modified())
            .expect("mtime"),
        mtime,
        "the original was not recopied"
    );

    // One owner leaving is not enough — the other still points at it.
    let one = vec![both[0].clone()];
    write_bundle(
        &okf_notebook("NB"),
        &one,
        &[],
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("drop one");
    assert!(bundle.join(&resource).exists(), "still claimed");

    // The last owner takes it with them.
    write_bundle(
        &okf_notebook("NB"),
        &[],
        &[],
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("drop both");
    assert!(
        !bundle.join(&resource).exists(),
        "no concept points at it any more"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A file a person put in `references/` themselves is not the writer's to
/// remove — only the content-addressed names it wrote are.
#[test]
fn okf_leaves_references_it_did_not_name() {
    use crate::okf::write_bundle;
    let dir = okf_scratch("refkeep");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");

    std::fs::create_dir_all(bundle.join("references")).expect("mkdir");
    std::fs::write(bundle.join("references/handout.pdf"), b"theirs").expect("write");
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("rewrite");
    assert!(
        bundle.join("references/handout.pdf").exists(),
        "a name the writer never chose is somebody else's file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Only a reference the bundle actually holds is a file to read; everything
/// else in `resource:` is provenance, and a path climbing out of the bundle
/// is refused (§6).
#[test]
fn okf_reference_paths_resolve_inside_the_bundle() {
    use crate::commands::okf_reference_path;
    let dir = okf_scratch("refpath");
    let bundle = dir.join("nb");
    std::fs::create_dir_all(bundle.join("references")).expect("mkdir");
    std::fs::write(bundle.join("references/abc.pdf"), b"bytes").expect("write");
    std::fs::write(dir.join("outside.pdf"), b"bytes").expect("write");

    assert_eq!(
        okf_reference_path(&bundle, "references/abc.pdf"),
        Some(bundle.join("references/abc.pdf"))
    );
    // Provenance, not files here.
    assert!(okf_reference_path(&bundle, "").is_none());
    assert!(okf_reference_path(&bundle, "https://example.com/a.pdf").is_none());
    assert!(okf_reference_path(&bundle, "/Users/someone/paper.pdf").is_none());
    // A reference the bundle does not carry falls back to the concept body.
    assert!(okf_reference_path(&bundle, "references/missing.pdf").is_none());
    // And no climbing out.
    assert!(okf_reference_path(&bundle, "../outside.pdf").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

/// The Notebooks folder resolves once, and iCloud Drive wins when it is
/// there (docs/RFC-okf-live.md §5.7).
#[test]
fn okf_notebooks_dir_prefers_icloud_when_it_is_on() {
    use crate::ai::default_notebooks_dir;
    let home = std::env::var("HOME").unwrap_or_default();
    let resolved = default_notebooks_dir();
    assert!(resolved.ends_with("/Alchemy"), "got {resolved}");

    let icloud = std::path::Path::new(&home).join("Library/Mobile Documents/com~apple~CloudDocs");
    if icloud.is_dir() {
        assert_eq!(
            resolved,
            icloud.join("Alchemy").to_string_lossy(),
            "iCloud Drive is on, so that is where notebooks go"
        );
    } else {
        // Not Documents/Alchemy when iCloud is on: Desktop & Documents
        // syncing is a separate switch most people leave off.
        assert_eq!(
            resolved,
            std::path::Path::new(&home)
                .join("Documents/Alchemy")
                .to_string_lossy(),
            "with iCloud Drive off, a local folder that works forever"
        );
    }
    // And the default is on, because the bundle is where a notebook lives.
    assert!(crate::ai::AiConfig::fresh().keep_on_disk);
    assert!(!crate::ai::AiConfig::fresh().keep_on_disk_asked);
}

/// A notebook gets its own folder under the root, deduped the way the
/// exporter's slugs are (§5.7).
#[test]
fn okf_notebook_folders_dedupe() {
    use crate::okf::claim_notebook_folder;
    let dir = okf_scratch("home");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut taken = std::collections::HashSet::new();

    let first = claim_notebook_folder(&dir, "Ferrari research", &taken);
    assert_eq!(first.file_name().unwrap(), "ferrari-research");
    taken.insert("ferrari-research".to_string());

    // A second notebook of the same name does not land on the first.
    let second = claim_notebook_folder(&dir, "Ferrari research", &taken);
    assert_eq!(second.file_name().unwrap(), "ferrari-research-2");

    // Nor does one whose folder is already on disk but not in a binding.
    std::fs::create_dir_all(dir.join("orders")).expect("mkdir");
    assert_eq!(
        claim_notebook_folder(&dir, "Orders", &std::collections::HashSet::new())
            .file_name()
            .unwrap(),
        "orders-2"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A system notebook (Briefs) is the app's own infrastructure and never
/// lands in the Notebooks folder (§5.7).
#[test]
fn okf_system_notebooks_never_bind() {
    use crate::okf::is_system_notebook;
    let nb = |status: &str| crate::models::Notebook {
        id: "n".into(),
        title: "Briefs".into(),
        created_at: 0,
        updated_at: 0,
        color: String::new(),
        icon: String::new(),
        status: status.into(),
        growth_web: false,
        source_count: 0,
        note_count: 0,
        report_count: 0,
    };
    assert!(is_system_notebook(&nb("system")));
    assert!(!is_system_notebook(&nb("")));
    assert!(!is_system_notebook(&nb("archived")));
}

/// A bundle in the Notebooks folder is opened once; a folder that is not a
/// bundle is left alone (§5.7).
#[test]
fn okf_finds_unopened_bundles_only() {
    use crate::okf::{unopened_bundles, write_bundle};
    let dir = okf_scratch("found");
    let root = dir.join("Alchemy");
    std::fs::create_dir_all(&root).expect("mkdir");

    // A real bundle.
    let bundle = root.join("ferrari-research");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("Ferrari research"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");
    // Somebody else's folder, and a dot directory.
    std::fs::create_dir_all(root.join("tax-receipts")).expect("mkdir");
    std::fs::write(root.join("tax-receipts/2026.txt"), b"x").expect("write");
    std::fs::create_dir_all(root.join(".Trash")).expect("mkdir");

    let none = std::collections::HashSet::new();
    let found = unopened_bundles(&root, &none);
    assert_eq!(found.len(), 1, "only the bundle: {found:?}");
    assert_eq!(found[0], bundle);

    // Already bound is already open — it is not found a second time.
    let bound: std::collections::HashSet<String> =
        [bundle.to_string_lossy().to_string()].into_iter().collect();
    assert!(unopened_bundles(&root, &bound).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

/// A bundle carries its notebook's id, which is what lets a second Mac
/// recognize the same notebook instead of duplicating it (§5.7).
#[test]
fn okf_bundles_carry_the_notebook_id_for_rebinding() {
    use crate::okf::{parse_okf_doc, write_bundle};
    let dir = okf_scratch("rebind");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("Shared"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");

    let index = std::fs::read_to_string(bundle.join("index.md")).expect("index");
    assert_eq!(
        parse_okf_doc(&index).nested("alchemy", "id").as_deref(),
        Some("nb-data"),
        "the id the second Mac matches on"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An evicted file is not a missing one: the writer asks for it and leaves
/// it alone rather than writing over the placeholder (§5.7).
#[test]
fn okf_writer_treats_a_stub_as_absent() {
    use crate::okf::{is_evicted_stub, write_bundle};
    let dir = okf_scratch("stub");
    let bundle = dir.join("nb");
    let (sources, notes) = okf_fixture();
    write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("seed");

    // Evict one concept the way iCloud does: the file goes, a hidden
    // placeholder stands in its place.
    let real = bundle.join("notes/old-thinking.md");
    let kept = std::fs::read_to_string(&real).expect("read");
    std::fs::remove_file(&real).expect("evict");
    std::fs::write(bundle.join("notes/.old-thinking.md.icloud"), b"").expect("stub");
    assert!(is_evicted_stub(&real), "that is what eviction looks like");
    assert!(
        !is_evicted_stub(&bundle.join("notes/what-the-data-says.md")),
        "a file that is here is not a stub"
    );
    assert!(
        !is_evicted_stub(&bundle.join("notes/never-existed.md")),
        "and neither is one that never existed"
    );

    // A pass over the evicted file writes nothing and does not lose it.
    let out = write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("rewrite");
    assert_eq!(out.written, 0, "nothing was written over the placeholder");
    assert!(!real.exists(), "and the file is still not downloaded");
    assert_eq!(out.removed, 0, "the concept was not treated as deleted");

    // Once it lands, the next pass sees it unchanged.
    std::fs::write(&real, &kept).expect("hydrate");
    std::fs::remove_file(bundle.join("notes/.old-thinking.md.icloud")).expect("clear stub");
    let after = write_bundle(
        &okf_notebook("NB"),
        &sources,
        &notes,
        &bundle,
        Some(&okf_manifest(&bundle)),
    )
    .expect("after");
    assert_eq!(
        after.written, 0,
        "the file that arrived is the one we wrote"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
