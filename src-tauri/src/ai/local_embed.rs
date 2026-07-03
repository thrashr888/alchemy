//! Built-in CPU embedder (Model2Vec potion-base-8M): ~30 MB download on first
//! use, then instant static embeddings — no Ollama, no GPU, sources stay
//! on-device. 256-dim vectors.

use std::sync::Arc;

use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use tokio::sync::OnceCell;

pub const BUILTIN_EMBED_MODEL: &str = "minishlab/potion-base-8M";

#[derive(Clone, Default)]
pub struct LocalEmbedder {
    model: Arc<OnceCell<Arc<StaticModel>>>,
}

impl LocalEmbedder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load (downloading to the HF cache on first use). Blocking work runs on
    /// a blocking thread; subsequent calls are instant.
    async fn model(&self) -> Result<Arc<StaticModel>> {
        let cell = self.model.clone();
        let out = cell
            .get_or_try_init(|| async {
                tokio::task::spawn_blocking(|| {
                    StaticModel::from_pretrained(BUILTIN_EMBED_MODEL, None, None, None)
                        .map(Arc::new)
                        .context(
                            "failed to load the built-in embedder (first use downloads ~30 MB \
                             from huggingface.co — check your network/proxy)",
                        )
                })
                .await
                .context("embedder load task failed")?
            })
            .await?;
        Ok(out.clone())
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
