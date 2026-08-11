//! Local cross-encoder reranker (ONNX via fastembed, same `ort` runtime
//! kokoro already ships). A cross-encoder reads (query, passage) PAIRS
//! through one transformer — categorically stronger at ordering than the
//! bi-encoder cosine scores that built the candidate pool, and the only
//! rerank shape that has ever beaten fusion order in our harness: every
//! prompted LLM (bonsai, Apple FM, listwise/pointwise/lite) scored BELOW
//! the fused baseline; the oracle says the pool itself holds 0.62–0.96
//! nDCG@10 if ranked perfectly (beir_eval.rs).
//!
//! Eval-first: this module is exercised by `beir_xenc_*` tests before any
//! app path adopts it. Models download once into the app data dir beside
//! the built-in embedder.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
use tokio::sync::OnceCell;

/// Truncate passages before scoring: the small rerankers cap at 512
/// tokens, and BEIR passages are paragraphs, not books. Char-based is
/// fine — the tokenizer re-truncates precisely.
const PASSAGE_CHARS: usize = 2_000;

/// Which cross-encoder to load. `Small` is the shipping candidate
/// (~150 MB, MiniLM-class, CPU-fast); `Large` is the quality ceiling
/// probe (bge-reranker-v2-m3, ~2 GB, multilingual, 8k context).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XencModel {
    Small,
    Large,
}

impl XencModel {
    fn to_fastembed(self) -> RerankerModel {
        match self {
            XencModel::Small => RerankerModel::JINARerankerV1TurboEn,
            XencModel::Large => RerankerModel::BGERerankerV2M3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            XencModel::Small => "jina-reranker-v1-turbo-en",
            XencModel::Large => "bge-reranker-v2-m3",
        }
    }
}

/// Lazy-loaded cross-encoder. Clone-cheap; the model loads once on first
/// score and lives for the process. `rerank` needs `&mut` (ort session
/// state), so the loaded model sits behind a Mutex — scoring is already
/// serialized per query, contention is theoretical.
#[derive(Clone)]
pub struct CrossEncoder {
    model: Arc<OnceCell<Arc<std::sync::Mutex<TextRerank>>>>,
    cache_dir: PathBuf,
    which: XencModel,
}

impl CrossEncoder {
    /// `data_dir` is the app data dir; models land in `data_dir/reranker`.
    pub fn new(data_dir: PathBuf, which: XencModel) -> Self {
        Self {
            model: Arc::new(OnceCell::new()),
            cache_dir: data_dir.join("reranker"),
            which,
        }
    }

    async fn model(&self) -> Result<Arc<std::sync::Mutex<TextRerank>>> {
        let cache = self.cache_dir.clone();
        let which = self.which;
        let out = self
            .model
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&cache).ok();
                    TextRerank::try_new(
                        RerankInitOptions::new(which.to_fastembed()).with_cache_dir(cache),
                    )
                    .map(|m| Arc::new(std::sync::Mutex::new(m)))
                    .context("failed to load the cross-encoder reranker")
                })
                .await
                .context("reranker load task failed")?
            })
            .await?;
        Ok(out.clone())
    }

    /// Score `passages` against `query`; returns indices into `passages`
    /// in best-first order. Blocking inference runs off the async runtime.
    pub async fn rank(&self, query: &str, passages: &[String]) -> Result<Vec<usize>> {
        if passages.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.model().await?;
        let query = query.to_string();
        let docs: Vec<String> = passages
            .iter()
            .map(|p| {
                let end = p
                    .char_indices()
                    .nth(PASSAGE_CHARS)
                    .map(|(i, _)| i)
                    .unwrap_or(p.len());
                p[..end].to_string()
            })
            .collect();
        tokio::task::spawn_blocking(move || {
            let mut model = model.lock().expect("reranker mutex poisoned");
            let mut results = model
                .rerank(query, &docs, false, None)
                .context("cross-encoder scoring failed")?;
            // fastembed returns best-first already, but sort defensively:
            // downstream order IS the product.
            results.sort_by(|a, b| b.score.total_cmp(&a.score));
            Ok(results.into_iter().map(|r| r.index).collect())
        })
        .await
        .context("reranker scoring task failed")?
    }
}
