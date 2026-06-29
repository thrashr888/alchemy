//! AI provider abstraction. Today this is Ollama-only, but `AiConfig` plus the
//! `Ollama` client are deliberately kept narrow so a cloud/MLX provider can be
//! swapped in behind the same `embed` / `chat` surface later.

mod ollama;

pub use ollama::{GenStats, Ollama};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub base_url: String,
    pub chat_model: String,
    pub embed_model: String,
    /// Vision model used to OCR image sources (empty disables OCR).
    #[serde(default)]
    pub vision_model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            chat_model: "gpt-oss:120b".to_string(),
            embed_model: "nomic-embed-text:latest".to_string(),
            // OCR is opt-in: pick a vision model in Settings to enable it.
            vision_model: String::new(),
        }
    }
}

/// A single chat turn handed to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

impl ChatTurn {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    #[allow(dead_code)]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}
