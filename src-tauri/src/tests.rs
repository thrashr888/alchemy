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
        source_count: 0,
    };
    db.create_notebook(&nb).await.expect("create notebook");
    assert_eq!(db.list_notebooks().await.unwrap().len(), 1, "notebook persisted");

    // 2. Ingest + chunk + embed + write
    let text = "Photosynthesis is the process by which green plants and some bacteria \
        convert light energy into chemical energy. It occurs in the chloroplasts using \
        the green pigment chlorophyll. The light-dependent reactions occur in the \
        thylakoid membranes and produce ATP and NADPH. The Calvin cycle occurs in the \
        stroma and fixes carbon dioxide into glucose. The overall products are glucose \
        and oxygen.";
    let extracted = ingest::extract_pasted("Photosynthesis basics", text).expect("extract");
    let chunks = ingest::chunk_text(&extracted.text);
    assert!(!chunks.is_empty(), "produced chunks");
    let embeddings = ai.embed(&chunks).await.expect("embed");
    assert_eq!(embeddings.len(), chunks.len(), "one vector per chunk");
    eprintln!("embedded {} chunks, dim={}", chunks.len(), embeddings[0].len());

    let chunk_tuples: Vec<(String, i32, String)> = chunks
        .iter()
        .enumerate()
        .map(|(i, t)| (uuid::Uuid::new_v4().to_string(), i as i32, t.clone()))
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
    };
    db.insert_source(&source, &chunk_tuples, &embeddings).await.expect("insert source");
    assert_eq!(db.list_sources(&nb.id).await.unwrap().len(), 1, "source persisted");

    // 3. Vector search
    let qvec = ai.embed_one("Where do the light-dependent reactions happen?").await.unwrap();
    let citations = db.search_chunks(&nb.id, qvec, 4).await.expect("search");
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
    let messages = rag::build_chat_messages(&[], "Where do the light-dependent reactions occur?", &citations);
    let answer = ai.chat(&messages).await.expect("chat");
    eprintln!("answer: {answer}");
    assert!(!answer.trim().is_empty(), "model produced an answer");

    let _ = std::fs::remove_dir_all(&dir);
}
