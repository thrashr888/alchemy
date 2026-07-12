//! End-to-end data-path test: ingest → embed → LanceDB write → vector search →
//! grounded chat. Requires a running Ollama with `nomic-embed-text`. If Ollama
//! isn't reachable the test no-ops so it never fails CI without a model server.
//!
//! Run with:  cargo test --lib rag_round_trip -- --nocapture

use crate::ai::{AiConfig, Ollama};
use crate::db::Db;
use crate::ingest;
use crate::models::{Notebook, Source};
use crate::rag;

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[tokio::test]
async fn rag_round_trip() {
    let ai = Ollama::new(AiConfig {
        base_url: "http://localhost:11434".into(),
        // Small local model to keep the chat step fast.
        chat_model: "digitsflow/bonsai-8b:latest".into(),
        embed_model: "nomic-embed-text".into(),
        vision_model: String::new(),
        ..Default::default()
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
        source_count: 0,
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
    let messages = rag::build_chat_messages(
        &[],
        "Where do the light-dependent reactions occur?",
        &citations,
        &[("src-1".to_string(), String::new())],
        "",
        "",
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

    let probe = Ollama::new(AiConfig::default());
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
