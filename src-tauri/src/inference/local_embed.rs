//! Built-in CPU embedder (Model2Vec potion-base-8M): ~30 MB one-time download
//! into the app data dir with byte-level progress reporting, then instant
//! static embeddings — no Ollama, no GPU, sources stay on-device (256-dim).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;

pub const BUILTIN_EMBED_MODEL: &str = "minishlab/potion-base-8M";
const FILES: &[&str] = &["config.json", "tokenizer.json", "model.safetensors"];

/// Progress callback: (file label, bytes done, bytes total).
pub type EmbedderProgress = Arc<dyn Fn(&str, u64, u64) + Send + Sync>;

#[derive(Clone)]
pub struct LocalEmbedder {
    model: Arc<OnceCell<Arc<StaticModel>>>,
    model_dir: PathBuf,
    progress: Option<EmbedderProgress>,
}

impl LocalEmbedder {
    pub fn new(data_dir: PathBuf, progress: Option<EmbedderProgress>) -> Self {
        Self {
            model: Arc::new(OnceCell::new()),
            model_dir: data_dir.join("embedder").join("potion-base-8M"),
            progress,
        }
    }

    /// Download any missing model files with progress, then load.
    async fn model(&self) -> Result<Arc<StaticModel>> {
        let this = self.clone();
        let out = self
            .model
            .get_or_try_init(|| async move {
                this.ensure_files().await?;
                let dir = this.model_dir.to_string_lossy().into_owned();
                tokio::task::spawn_blocking(move || {
                    StaticModel::from_pretrained(&dir, None, None, None)
                        .map(Arc::new)
                        .context("failed to load the built-in embedder from disk")
                })
                .await
                .context("embedder load task failed")?
            })
            .await?;
        Ok(out.clone())
    }

    async fn ensure_files(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.model_dir).await.ok();
        let http = reqwest::Client::new();
        for file in FILES {
            let dest = self.model_dir.join(file);
            if tokio::fs::metadata(&dest)
                .await
                .map(|m| m.len() > 0)
                .unwrap_or(false)
            {
                continue;
            }
            let url = format!("https://huggingface.co/{BUILTIN_EMBED_MODEL}/resolve/main/{file}");
            let resp = http
                .get(&url)
                .timeout(std::time::Duration::from_secs(300))
                .send()
                .await
                .with_context(|| {
                    format!(
                        "downloading the built-in embedder failed ({file}) — check your \
                         network/proxy access to huggingface.co"
                    )
                })?;
            if !resp.status().is_success() {
                anyhow::bail!("embedder download {file}: HTTP {}", resp.status());
            }
            let total = resp.content_length().unwrap_or(0);
            let tmp = dest.with_extension("part");
            let mut out = tokio::fs::File::create(&tmp)
                .await
                .with_context(|| format!("cannot write {}", tmp.display()))?;
            let mut done: u64 = 0;
            let mut stream = resp.bytes_stream();
            use futures_util::StreamExt;
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.context("embedder download interrupted")?;
                out.write_all(&bytes).await?;
                done += bytes.len() as u64;
                if let Some(cb) = &self.progress {
                    cb(file, done, total);
                }
            }
            out.flush().await?;
            drop(out);
            tokio::fs::rename(&tmp, &dest).await?;
            if let Some(cb) = &self.progress {
                cb(file, total.max(done), total.max(done));
            }
        }
        Ok(())
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let model = self.model().await?;
        let texts = texts.to_vec();
        tokio::task::spawn_blocking(move || Ok(model.encode(&texts)))
            .await
            .context("embedding task failed")?
    }

    pub async fn test_embed(&self) -> Result<usize> {
        let v = self.embed(&["ok".to_string()]).await?;
        Ok(v.first().map(|x| x.len()).unwrap_or(0))
    }
}
