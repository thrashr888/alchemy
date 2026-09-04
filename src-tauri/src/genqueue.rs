//! The generation queue (docs/RFC-generation-queue.md).
//!
//! Enqueueing a generation creates its note immediately (status
//! "generating") and parks a job here; a worker task the backend owns
//! drains the queue, so the webview is a spectator — reload or close the
//! window and the run continues. Jobs persist to a JSON sidecar (not a
//! Lance column: the store is shared dev/prod and schema changes brick
//! older binaries), so an app restart re-queues whatever was mid-flight.
//!
//! Concurrency is one job per engine: local engines serialize because
//! parallel decodes thrash the same GPU/RAM, while a per-job provider
//! override (MCP) runs beside the default engine. An engine that refuses
//! connections parks the job as "waiting" and the worker's 30s tick
//! re-tries until it answers — Ollama being off means paused, not dead.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tauri::{Emitter, Manager};
use tokio_util::sync::CancellationToken;

pub const QUEUE_FILE: &str = "generation-queue.json";

/// Terminal jobs older than this prune from the file on save.
const PRUNE_MS: i64 = 86_400_000;

/// Hard deadline over one run — "generating" must be a bounded state.
const RUN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenJob {
    pub id: String,
    pub notebook_id: String,
    pub kind: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub source_ids: Option<Vec<String>>,
    pub note_id: String,
    /// queued | running | waiting | done | error | cancelled
    pub status: String,
    #[serde(default)]
    pub error: String,
    /// MCP's per-call provider override; None routes the Generate role.
    #[serde(default)]
    pub provider: Option<String>,
    /// Concurrency key, stamped at dispatch (engine id or provider id).
    #[serde(default)]
    pub engine_key: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusEvent {
    job_id: String,
    note_id: String,
    notebook_id: String,
    status: String,
    detail: String,
    title: String,
}

pub struct GenQueue {
    jobs: std::sync::Mutex<Vec<GenJob>>,
    cancels: std::sync::Mutex<HashMap<String, CancellationToken>>,
    pub notify: tokio::sync::Notify,
    path: PathBuf,
}

impl GenQueue {
    /// Load the persisted queue. Jobs found running or waiting were
    /// interrupted by the last shutdown — they re-enter as queued and
    /// restart from scratch (mid-stream checkpoints are v2).
    pub fn load(data_dir: &std::path::Path) -> Self {
        let path = data_dir.join(QUEUE_FILE);
        let mut jobs: Vec<GenJob> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        for j in &mut jobs {
            if j.status == "running" || j.status == "waiting" {
                j.status = "queued".into();
            }
        }
        Self {
            jobs: std::sync::Mutex::new(jobs),
            cancels: std::sync::Mutex::new(HashMap::new()),
            notify: tokio::sync::Notify::new(),
            path,
        }
    }

    fn save(&self, jobs: &mut Vec<GenJob>) {
        let now = crate::commands::now();
        jobs.retain(|j| {
            !matches!(j.status.as_str(), "done" | "error" | "cancelled")
                || now - j.updated_at < PRUNE_MS
        });
        if let Ok(json) = serde_json::to_string_pretty(jobs) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    pub fn enqueue(&self, job: GenJob) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(job);
        self.save(&mut jobs);
        drop(jobs);
        self.notify.notify_one();
    }

    pub fn list(&self) -> Vec<GenJob> {
        self.jobs.lock().unwrap().clone()
    }

    /// Cancel by note id (the id the UI holds). A running job's token
    /// fires; a queued/waiting job just flips. Returns the job if one
    /// was actually cancelled.
    pub fn cancel_by_note(&self, note_id: &str) -> Option<(GenJob, bool)> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs.iter_mut().find(|j| {
            j.note_id == note_id && matches!(j.status.as_str(), "queued" | "waiting" | "running")
        })?;
        let was_running = job.status == "running";
        job.status = "cancelled".into();
        job.updated_at = crate::commands::now();
        let out = job.clone();
        self.save(&mut jobs);
        drop(jobs);
        if was_running {
            if let Some(t) = self.cancels.lock().unwrap().get(&out.id) {
                t.cancel();
            }
        }
        self.notify.notify_one();
        Some((out, was_running))
    }

    fn set_status(&self, id: &str, status: &str, error: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
            // A cancel that raced the finish stays a cancel.
            if j.status == "cancelled" && status != "cancelled" {
                return;
            }
            j.status = status.into();
            j.error = error.into();
            j.updated_at = crate::commands::now();
        }
        self.save(&mut jobs);
    }

    fn was_cancelled(&self, id: &str) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .iter()
            .any(|j| j.id == id && j.status == "cancelled")
    }
}

/// An error that means "the engine isn't there", as opposed to one the
/// prompt or model earned: these park the job instead of failing it.
pub fn is_engine_down(err: &str) -> bool {
    let l = err.to_lowercase();
    l.contains("connection refused") || l.contains("tcp connect error") || l.contains("dns error")
}

pub(crate) fn emit_status(app: &tauri::AppHandle, job: &GenJob, title: &str, detail: &str) {
    let _ = app.emit(
        "generation://status",
        StatusEvent {
            job_id: job.id.clone(),
            note_id: job.note_id.clone(),
            notebook_id: job.notebook_id.clone(),
            status: job.status.clone(),
            detail: detail.to_string(),
            title: title.to_string(),
        },
    );
}

/// The drain loop: dispatch whatever can run, then sleep until something
/// changes (a new job, a cancel) or the 30s tick re-probes waiting jobs.
pub fn spawn_worker(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            dispatch(&app).await;
            let state = app.state::<crate::commands::AppState>();
            let queue = &state.gen_queue;
            tokio::select! {
                _ = queue.notify.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            }
        }
    });
}

async fn dispatch(app: &tauri::AppHandle) {
    let state = app.state::<crate::commands::AppState>();
    // The default engine key is read outside the jobs lock (it awaits).
    let default_key = {
        let ai = state.ai.read().await;
        ai.chat_engine_id(crate::inference::Role::Generate)
            .to_string()
    };
    let to_run: Vec<GenJob> = {
        let mut jobs = state.gen_queue.jobs.lock().unwrap();
        let mut busy: HashSet<String> = jobs
            .iter()
            .filter(|j| j.status == "running")
            .map(|j| j.engine_key.clone())
            .collect();
        let mut out = Vec::new();
        let now = crate::commands::now();
        for j in jobs.iter_mut() {
            if !matches!(j.status.as_str(), "queued" | "waiting") {
                continue;
            }
            let key = j.provider.clone().unwrap_or_else(|| default_key.clone());
            if busy.contains(&key) {
                continue;
            }
            busy.insert(key.clone());
            j.status = "running".into();
            // A fresh attempt owes nothing to the last one's failure text.
            j.error = String::new();
            j.engine_key = key;
            j.updated_at = now;
            out.push(j.clone());
        }
        if !out.is_empty() {
            state.gen_queue.save(&mut jobs);
        }
        out
    };
    for job in to_run {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { run_job(app, job).await });
    }
}

/// What one queued job is called on the activity indicator: the document
/// kind, which is the word the Studio itself uses for it.
pub(crate) fn job_label(job: &GenJob) -> String {
    let kind = job.kind.trim();
    if kind.is_empty() {
        "a document".to_string()
    } else {
        kind.replace('_', " ")
    }
}

async fn run_job(app: tauri::AppHandle, job: GenJob) {
    let state = app.state::<crate::commands::AppState>();
    let queue = &state.gen_queue;
    let token = CancellationToken::new();
    queue
        .cancels
        .lock()
        .unwrap()
        .insert(job.id.clone(), token.clone());
    emit_status(&app, &job, "", "");

    // The indicator names the job, not just the engine: a queue running for
    // minutes should say what it is making.
    let label = format!("Generating {}", crate::genqueue::job_label(&job));
    let produced = tokio::select! {
        r = tokio::time::timeout(
            RUN_DEADLINE,
            crate::inference::labeled(
                label,
                crate::commands::generate_content_for_job(&state, &app, &job, &token),
            ),
        ) => Some(match r {
            Ok(inner) => inner,
            Err(_) => Err(anyhow::anyhow!(
                "generation exceeded {} minutes — the model provider may be \
                 overloaded; try again or switch providers",
                RUN_DEADLINE.as_secs() / 60
            )),
        }),
        _ = token.cancelled() => None,
    };
    queue.cancels.lock().unwrap().remove(&job.id);

    let ts = crate::commands::now();
    match produced {
        // Cancelled mid-run: the pending note held nothing — remove it.
        None => {
            let _ = state.db.delete_note(&job.note_id).await;
            let mut j = job.clone();
            j.status = "cancelled".into();
            emit_status(&app, &j, "", "");
        }
        Some(Ok((title, content))) => {
            if let Err(err) = state
                .db
                .update_note(&job.note_id, &title, &content, ts)
                .await
            {
                crate::note!("genqueue: persisting result failed: {err:#}");
                queue.set_status(&job.id, "error", "persist failed");
                return;
            }
            let _ = state.db.set_note_status(&job.note_id, "").await;
            queue.set_status(&job.id, "done", "");
            if let Ok(Some(done)) = state.db.get_note(&job.note_id).await {
                crate::commands::index_note(&state, &done).await;
                let _ = app.emit("generate://done", &done);
            }
            let mut j = job.clone();
            j.status = "done".into();
            emit_status(&app, &j, &title, "");
        }
        Some(Err(err)) => {
            let raw = format!("{err:#}");
            // A user cancel surfaces as an engine error on some providers
            // (the stream drops) — honor the recorded cancel over the error.
            if queue.was_cancelled(&job.id) {
                let _ = state.db.delete_note(&job.note_id).await;
                let mut j = job.clone();
                j.status = "cancelled".into();
                emit_status(&app, &j, "", "");
            } else if is_engine_down(&raw) {
                // Parked, not failed: the pending note says so and the
                // worker's tick re-tries until the engine answers.
                queue.set_status(&job.id, "waiting", &raw);
                let detail = crate::commands::classify_model_error(&raw)
                    .unwrap_or_else(|| "The model engine isn't answering.".into());
                let mut j = job.clone();
                j.status = "waiting".into();
                emit_status(&app, &j, "", &detail);
            } else {
                let msg = crate::commands::classify_model_error(&raw)
                    .unwrap_or_else(|| format!("Generation failed: {raw}"));
                let _ = state
                    .db
                    .update_note(&job.note_id, &placeholder_stripped(&job), &msg, ts)
                    .await;
                let _ = state.db.set_note_status(&job.note_id, "error").await;
                queue.set_status(&job.id, "error", &msg);
                let mut j = job.clone();
                j.status = "error".into();
                emit_status(&app, &j, "", &msg);
            }
        }
    }
    let _ = app.emit(
        "mcp://changed",
        serde_json::json!({ "scope": "notes", "notebookId": job.notebook_id }),
    );
    // A finished engine frees its slot — someone may be queued behind it.
    queue.notify.notify_one();
}

/// The pending title without its "(generating…)" tail, so an error note
/// reads as the artifact it wanted to be.
fn placeholder_stripped(job: &GenJob) -> String {
    match crate::rag::artifact_spec(&job.kind) {
        Some((t, _)) => t.to_string(),
        None => "Report".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, status: &str) -> GenJob {
        GenJob {
            id: id.into(),
            notebook_id: "nb".into(),
            kind: "briefing".into(),
            prompt: String::new(),
            source_ids: None,
            note_id: format!("note-{id}"),
            status: status.into(),
            error: String::new(),
            provider: None,
            engine_key: String::new(),
            created_at: crate::commands::now(),
            updated_at: crate::commands::now(),
        }
    }

    #[test]
    fn interrupted_jobs_requeue_on_load() {
        let dir = std::env::temp_dir().join(format!("alchemy-genq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let q = GenQueue::load(&dir);
        let mut running = job("a", "running");
        running.engine_key = "ollama".into();
        q.enqueue(running);
        q.enqueue(job("b", "waiting"));
        q.enqueue(job("c", "done"));
        drop(q);
        let q = GenQueue::load(&dir);
        let jobs = q.list();
        assert_eq!(jobs.iter().filter(|j| j.status == "queued").count(), 2);
        assert_eq!(jobs.iter().filter(|j| j.status == "done").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_flips_queued_jobs_and_survives_status_races() {
        let dir = std::env::temp_dir().join(format!("alchemy-genq2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let q = GenQueue::load(&dir);
        q.enqueue(job("a", "queued"));
        let (cancelled, was_running) = q.cancel_by_note("note-a").expect("job found");
        assert_eq!(cancelled.status, "cancelled");
        assert!(!was_running);
        // A late "done" from a racing finish must not resurrect it.
        q.set_status("a", "done", "");
        assert!(q.was_cancelled("a"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_down_classifier_matches_field_errors() {
        assert!(is_engine_down(
            "ollama chat request failed: error sending request for url \
             (http://localhost:11434/api/chat): client error (Connect): \
             tcp connect error: Connection refused (os error 61)"
        ));
        assert!(!is_engine_down(
            "model \"x\" not found, try pulling it first"
        ));
        assert!(!is_engine_down("invalid api key"));
    }
}
