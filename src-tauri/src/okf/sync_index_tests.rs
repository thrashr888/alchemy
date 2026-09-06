//! Exercise ordinary note sync with the real HTTP embedder deliberately held.
use super::{sync_tests::Lab, *};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Each request is explicitly released by the test. The model is genuinely
// blocked on HTTP, rather than bypassed through the audio-note fixture kind.
async fn held_embedder() -> (
    String,
    tokio::sync::mpsc::Receiver<(Vec<String>, tokio::sync::oneshot::Sender<()>)>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (requests_tx, requests_rx) = tokio::sync::mpsc::channel(2);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let (body_start, body_len) = loop {
                let mut buffer = [0; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "embedding request ended before headers");
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    assert!(headers.starts_with("POST /api/embed "));
                    let len = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, len);
                }
            };
            while request.len() < body_start + body_len {
                let mut buffer = [0; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "embedding request ended before body");
                request.extend_from_slice(&buffer[..read]);
            }
            let json: serde_json::Value =
                serde_json::from_slice(&request[body_start..body_start + body_len]).unwrap();
            let inputs: Vec<String> = serde_json::from_value(json["input"].clone()).unwrap();
            let count = inputs.len();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            requests_tx.send((inputs, release_tx)).await.unwrap();
            release_rx.await.unwrap();
            let body = serde_json::json!({"embeddings": vec![vec![0.1; 8]; count]}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body,
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, requests_rx, server)
}

#[tokio::test]
async fn ordinary_note_import_and_update_release_sync_lock_before_embedding() {
    let lab = Lab::new();
    let bundle = lab.0.join("shared");
    let replica = lab.replica("ordinary-reader", &bundle).await;
    let (url, mut requests, server) = held_embedder().await;
    let mut config = replica.ai.read().await.config().clone();
    config.base_url = url;
    config.embedder = "ollama".into();
    *replica.ai.write().await = crate::ai::Ai::new(
        config,
        crate::ai::AiRuntime {
            data_dir: app_data_dir(&replica),
            ..Default::default()
        },
    );
    std::fs::create_dir_all(bundle.join("notes")).unwrap();
    let path = bundle.join("notes/ordinary.md");
    let original = "---\ntitle: Ordinary\ntype: Note\n---\nOriginal ordinary note body.\n";
    std::fs::write(&path, original).unwrap();
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        reconcile(&replica, "shared-notebook"),
    )
    .await
    .expect("ordinary note import waited on embedding")
    .unwrap();
    assert_eq!(first.created, 1);
    let (first_inputs, release_first) =
        tokio::time::timeout(Duration::from_secs(5), requests.recv())
            .await
            .unwrap()
            .unwrap();
    assert!(first_inputs
        .iter()
        .any(|text| text.contains("Original ordinary note body")));
    let note = replica
        .db
        .list_notes("shared-notebook")
        .await
        .unwrap()
        .remove(0);
    assert_eq!(note.kind, "note");
    assert!(note.content.contains("Original ordinary note body"));
    assert!(!tokio::time::timeout(
        Duration::from_secs(5),
        reconcile(&replica, "shared-notebook")
    )
    .await
    .expect("blocked embedding retained the sync lock")
    .unwrap()
    .changed());

    std::fs::write(
        &path,
        original.replace(
            "Original ordinary note body",
            "Updated ordinary note body after remote edit",
        ),
    )
    .unwrap();
    let edit = tokio::time::timeout(
        Duration::from_secs(5),
        reconcile(&replica, "shared-notebook"),
    )
    .await
    .expect("ordinary note update waited on previous embedding")
    .unwrap();
    assert_eq!(edit.updated, 1);
    assert!(replica
        .db
        .get_note(&note.id)
        .await
        .unwrap()
        .unwrap()
        .content
        .contains("Updated ordinary note body"));
    release_first.send(()).unwrap();
    let (second_inputs, release_second) =
        tokio::time::timeout(Duration::from_secs(5), requests.recv())
            .await
            .unwrap()
            .unwrap();
    assert!(second_inputs
        .iter()
        .any(|text| text.contains("Updated ordinary note body")));
    // The stale first result cannot publish chunks while the fresh job waits.
    assert!(replica
        .db
        .source_chunk_rows(&format!("note:{}", note.id))
        .await
        .unwrap()
        .is_empty());
    assert!(!tokio::time::timeout(
        Duration::from_secs(5),
        reconcile(&replica, "shared-notebook")
    )
    .await
    .expect("updated note embedding retained the sync lock")
    .unwrap()
    .changed());
    release_second.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !replica
                .db
                .pending_note_index_ids()
                .await
                .unwrap()
                .contains(&note.id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("index worker did not complete");
    let rows = replica
        .db
        .source_chunk_rows(&format!("note:{}", note.id))
        .await
        .unwrap();
    assert!(!rows.is_empty());
    assert!(rows
        .iter()
        .all(|(_, _, body)| body.contains("Updated ordinary note body")));
}
