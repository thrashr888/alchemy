//! End-to-end data-path test: ingest → embed → LanceDB write → vector search →
//! grounded chat. Requires a running Ollama with `nomic-embed-text`. If Ollama
//! isn't reachable the test no-ops so it never fails CI without a model server.
//!
//! Run with:  cargo test --lib rag_round_trip -- --nocapture

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
    });
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
    use crate::commands::{okf_description, okf_slug};
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
