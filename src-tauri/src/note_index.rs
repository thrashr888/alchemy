//! Derived note retrieval work. Sync persists rows first; one worker per
//! database coalesces queued IDs and fetches their latest text only when ready.
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use crate::{ai::Ai, db::Db, ingest, models::Note};

#[derive(Default)]
pub(crate) struct Queue {
    ids: VecDeque<String>,
    pending: HashSet<String>,
    running: bool,
}

impl Queue {
    fn push(&mut self, id: &str) -> bool {
        if self.pending.insert(id.to_owned()) {
            self.ids.push_back(id.to_owned());
        }
        !std::mem::replace(&mut self.running, true)
    }

    fn pop(&mut self) -> Option<String> {
        let id = self.ids.pop_front();
        if let Some(id) = &id {
            self.pending.remove(id);
        } else {
            self.running = false;
        }
        id
    }
}

pub(crate) fn pending_key(id: &str) -> String {
    format!("note-index-pending:{id}")
}

pub(crate) async fn mark_pending(db: &Db, id: &str) -> anyhow::Result<()> {
    let _guard = db.note_index_lock.lock().await;
    db.kv_set(&pending_key(id), "pending").await
}

pub(crate) fn enqueue(db: Arc<Db>, ai: Ai, id: &str) {
    let start = db
        .note_index_queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(id);
    if !start {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            let id = db
                .note_index_queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop();
            let Some(id) = id else { break };
            let result = async {
                if let Some(note) = db.get_note(&id).await? {
                    index(&db, &ai, &note).await?;
                } else {
                    db.kv_set(&pending_key(&id), "").await?;
                }
                anyhow::Ok(())
            }
            .await;
            if let Err(err) = result {
                crate::diagnostics::error(
                    "note-index",
                    format!("Indexing note {id} failed; the note remains available and indexing will retry next launch or edit: {err:#}"),
                );
            }
        }
    });
}

fn same_version(current: &Note, expected: &Note) -> bool {
    current.updated_at == expected.updated_at
        && current.notebook_id == expected.notebook_id
        && current.title == expected.title
        && current.content == expected.content
        && current.kind == expected.kind
}

pub(crate) async fn index(db: &Db, ai: &Ai, note: &Note) -> anyhow::Result<()> {
    index_with(db, note, |inputs| async move {
        tokio::time::timeout(Duration::from_secs(30), ai.embed(&inputs))
            .await
            .map_err(|_| anyhow::anyhow!("note embedding timed out after 30 seconds"))?
    })
    .await
}

async fn index_with<F, Fut>(db: &Db, note: &Note, embed: F) -> anyhow::Result<()>
where
    F: FnOnce(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<Vec<f32>>>>,
{
    if !db
        .get_note(&note.id)
        .await?
        .is_some_and(|current| same_version(&current, note))
    {
        return Ok(());
    }
    // Keep previous retrieval chunks until replacement embeddings succeed.
    let chunks = if note.kind == "audio_overview" {
        Vec::new()
    } else {
        ingest::chunk_text(&note.title, &note.content)
    };
    let embeddings = if chunks.is_empty() {
        Vec::new()
    } else {
        embed(chunks.iter().map(|c| c.embed_text.clone()).collect()).await?
    };
    anyhow::ensure!(
        embeddings.len() == chunks.len(),
        "embedder returned the wrong number of vectors"
    );
    let _guard = db.note_index_lock.lock().await;
    if !db
        .get_note(&note.id)
        .await?
        .is_some_and(|current| same_version(&current, note))
    {
        return Ok(());
    }
    db.delete_note_chunks(&note.id).await?;
    if !chunks.is_empty() {
        let tuples: Vec<_> = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (uuid::Uuid::new_v4().to_string(), i as i32, c.text.clone()))
            .collect();
        let contexts: Vec<_> = chunks.iter().map(|c| c.context.clone()).collect();
        db.add_chunks_ctx(
            &note.notebook_id,
            &format!("{}{}", crate::db::NOTE_CHUNK_PREFIX, note.id),
            &tuples,
            &contexts,
            &embeddings,
        )
        .await?;
    }
    db.kv_set(&pending_key(&note.id), "").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (std::path::PathBuf, Arc<Db>, Note) {
        let path =
            std::env::temp_dir().join(format!("alchemy-note-index-{}", uuid::Uuid::new_v4()));
        let db = Arc::new(Db::open(&path).await.unwrap());
        let note = Note {
            id: "ordinary-note".into(),
            notebook_id: "notebook".into(),
            title: "A note".into(),
            content: "The original body is stored without a model.".into(),
            kind: "note".into(),
            prompt: String::new(),
            origin: String::new(),
            status: String::new(),
            created_at: 1,
            updated_at: 1,
        };
        db.add_note(&note).await.unwrap();
        (path, db, note)
    }

    #[test]
    fn queue_coalesces_ids_and_starts_one_worker() {
        let mut queue = Queue::default();
        assert!(queue.push("a"));
        for _ in 0..10_000 {
            assert!(!queue.push("a"));
        }
        assert!(!queue.push("b"));
        assert_eq!(queue.ids.len(), 2);
        assert_eq!(queue.pop().as_deref(), Some("a"));
        assert!(!queue.push("a")); // New edit while a previous version is active.
        assert_eq!(queue.pop().as_deref(), Some("b"));
        assert_eq!(queue.pop().as_deref(), Some("a"));
        assert_eq!(queue.pop(), None);
        assert!(queue.push("c"));
    }

    #[tokio::test]
    async fn unavailable_embedder_preserves_note_and_previous_index() {
        let (path, db, note) = fixture().await;
        index_with(&db, &note, |inputs| async move {
            Ok(vec![vec![0.1; 8]; inputs.len()])
        })
        .await
        .unwrap();
        let before = db.source_chunk_rows("note:ordinary-note").await.unwrap();
        db.kv_set(&pending_key(&note.id), "pending").await.unwrap();
        let ai = Ai::new(
            crate::ai::AiConfig {
                base_url: "http://127.0.0.1:1".into(),
                embedder: "ollama".into(),
                ..Default::default()
            },
            crate::ai::AiRuntime {
                data_dir: path.clone(),
                ..Default::default()
            },
        );
        assert!(index(&db, &ai, &note).await.is_err());
        assert_eq!(
            db.get_note(&note.id).await.unwrap().unwrap().content,
            note.content
        );
        assert_eq!(
            db.source_chunk_rows("note:ordinary-note").await.unwrap(),
            before
        );
        assert_eq!(
            db.kv_get(&pending_key(&note.id)).await.unwrap().as_deref(),
            Some("pending")
        );
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn retry_marker_survives_restart_and_clears_after_success() {
        let (path, db, note) = fixture().await;
        mark_pending(&db, &note.id).await.unwrap();
        drop(db);
        let db = Db::open(&path).await.unwrap();
        assert!(db
            .pending_note_index_ids()
            .await
            .unwrap()
            .contains(&note.id));
        index_with(&db, &note, |inputs| async move {
            Ok(vec![vec![0.1; 8]; inputs.len()])
        })
        .await
        .unwrap();
        assert!(db.pending_note_index_ids().await.unwrap().is_empty());
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn slow_old_version_cannot_overwrite_new_index() {
        let (path, db, note) = fixture().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let old_db = db.clone();
        let old_note = note.clone();
        let old = tokio::spawn(async move {
            index_with(&old_db, &old_note, |inputs| async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(vec![vec![0.1; 8]; inputs.len()])
            })
            .await
            .unwrap();
        });
        started_rx.await.unwrap();
        // Even an edit with a tied timestamp must supersede the old body.
        tokio::time::timeout(
            Duration::from_secs(2),
            db.update_note(&note.id, &note.title, "New body", 1),
        )
        .await
        .unwrap()
        .unwrap();
        let fresh = db.get_note(&note.id).await.unwrap().unwrap();
        index_with(&db, &fresh, |inputs| async move {
            Ok(vec![vec![0.2; 8]; inputs.len()])
        })
        .await
        .unwrap();
        let current = db.source_chunk_rows("note:ordinary-note").await.unwrap();
        release_tx.send(()).unwrap();
        old.await.unwrap();
        assert_eq!(
            db.source_chunk_rows("note:ordinary-note").await.unwrap(),
            current
        );
        assert!(current.iter().all(|(_, _, body)| body.contains("New body")));
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn deleted_note_is_not_reindexed_after_slow_embed() {
        let (path, db, note) = fixture().await;
        let deleting_db = db.clone();
        let deleting_id = note.id.clone();
        index_with(&db, &note, |inputs| async move {
            deleting_db.delete_note(&deleting_id).await.unwrap();
            Ok(vec![vec![0.1; 8]; inputs.len()])
        })
        .await
        .unwrap();
        assert!(db.get_note(&note.id).await.unwrap().is_none());
        assert!(db
            .source_chunk_rows("note:ordinary-note")
            .await
            .unwrap()
            .is_empty());
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }
}
