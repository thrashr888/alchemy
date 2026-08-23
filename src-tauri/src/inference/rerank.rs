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

    /// Raw relevance scores for `passages` against `query`, in passage
    /// order — the primitive under both ranking (chat rerank) and
    /// entailment-style verification (judged evals treat score-over-
    /// threshold as "this excerpt supports this claim"). Positive logits
    /// mean relevant; the judged harness calibrates the exact threshold.
    pub async fn scores(&self, query: &str, passages: &[String]) -> Result<Vec<f32>> {
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
            let results = model
                .rerank(query, &docs, false, None)
                .context("cross-encoder scoring failed")?;
            let mut scores = vec![0.0f32; docs.len()];
            for r in results {
                scores[r.index] = r.score;
            }
            Ok(scores)
        })
        .await
        .context("reranker scoring task failed")?
    }

    /// Score `passages` against `query`; returns indices into `passages`
    /// in best-first order. Blocking inference runs off the async runtime.
    pub async fn rank(&self, query: &str, passages: &[String]) -> Result<Vec<usize>> {
        let scores = self.scores(query, passages).await?;
        let mut order: Vec<usize> = (0..scores.len()).collect();
        order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-app-shaped latency: ~24 chunk-sized passages (the k*3 chat pool),
    /// not BEIR's 50 × 2000-char documents. Reuses the eval's downloaded
    /// model; skips silently when it isn't on disk. Measured 2026-08-11 on
    /// an idle M-series: 737 ms with the default options (batched all-core
    /// beats capped threads; a 320-token cap changed nothing because chunk
    /// passages already fit) — so the defaults ARE the tuned config.
    #[tokio::test]
    #[ignore = "needs the reranker model downloaded (any BEIR_XENC=small run)"]
    async fn xenc_latency_smoke() {
        let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/beir-cache");
        let xe = CrossEncoder::new(cache, XencModel::Small);
        let passage = "Espresso dialing starts with a 1:2 ratio and a 25 to 30 second shot. \
                       Grind finer when the shot runs fast and sour; coarser when it chokes \
                       and turns bitter. Change one variable at a time and taste each pull. "
            .repeat(4);
        let pool: Vec<String> = (0..24).map(|i| format!("{i} {passage}")).collect();
        // First call pays model load; the second is the steady state chat sees.
        if xe
            .rank("how do I dial in espresso grind size", &pool)
            .await
            .is_err()
        {
            crate::note!("SKIP: reranker model not downloaded");
            return;
        }
        let t0 = std::time::Instant::now();
        let order = xe
            .rank("how do I dial in espresso grind size", &pool)
            .await
            .expect("rank");
        crate::note!(
            "xenc small: 24 chunk-sized passages in {:.0} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(order.len(), 24);
    }
}
