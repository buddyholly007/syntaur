//! Pure-Rust sentence embedding wrapper.
//!
//! Wraps the `fastembed` crate (which uses ONNX runtime via `ort`) to give
//! the voice tool router a thread-safe, async-friendly text → vector function
//! without adding any external service to the syntaur stack.
//!
//! ## Why fastembed instead of LlmChain::embed_text
//!
//! `LlmChain::embed_text` requires a provider in `models.providers` whose
//! model id contains "embed". As of 2026-04-09 there's no such provider —
//! TurboQuant and Nemotron are both chat models without `/v1/embeddings`
//! support, and adding a separate llama-server instance just to host an
//! embedding model is needless infrastructure when we can ship a 30 MB ONNX
//! file inside the syntaur binary's runtime cache and run it CPU.
//!
//! BGE-small-en-v1.5 produces 384-dim float32 vectors, takes ~50 ms per
//! short query on a desktop Intel, and is the de-facto default embedding
//! model for English short-text routing in 2025-2026.
//!
//! ## Thread safety
//!
//! `fastembed::TextEmbedding` holds an ONNX session, which is `Send + Sync`
//! but not internally locked. We wrap it in `tokio::sync::Mutex` so concurrent
//! `find_tool` calls serialize cleanly. Each `embed()` call holds the lock
//! for ~50 ms which is fine for voice traffic (one user, sequential turns).

use std::sync::Arc;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use tokio::sync::Mutex;

pub struct Embedder {
    inner: Mutex<TextEmbedding>,
}

impl Embedder {
    /// Initialize the embedder with BGE-small-en-v1.5. The first call after
    /// a fresh install downloads the model (~30 MB) into fastembed's default
    /// cache dir (`~/.cache/fastembed/` or `$FASTEMBED_CACHE_PATH` if set).
    /// Subsequent calls hit the cache and return immediately.
    pub fn new() -> Result<Arc<Self>, String> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_show_download_progress(false),
        )
        .map_err(|e| format!("fastembed init: {}", e))?;
        Ok(Arc::new(Self {
            inner: Mutex::new(model),
        }))
    }

    /// Embed a single text. Returns a 384-dim float vector for BGE-small.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut model = self.inner.lock().await;
        let result = model
            .embed(vec![text.to_string()], None)
            .map_err(|e| format!("fastembed embed: {}", e))?;
        result
            .into_iter()
            .next()
            .ok_or_else(|| "fastembed returned empty result".to_string())
    }

    /// Embed many texts in one batch — used at startup when populating the
    /// router with all initial entries. Faster than calling embed() in a loop
    /// because fastembed batches through ONNX in one shot.
    pub async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self.inner.lock().await;
        model
            .embed(texts, None)
            .map_err(|e| format!("fastembed embed_batch: {}", e))
    }
}

/// Cosine similarity between two vectors. Both must be the same length;
/// returns 0.0 if either is all zeros (avoids NaN). Range: [-1, 1] but
/// BGE outputs are typically in [0, 1] for related text.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_vectors_is_one() {
        let v = vec![0.5_f32, 0.3, 0.7, 0.1];
        let c = cosine(&v, &v);
        assert!((c - 1.0).abs() < 1e-6, "expected ~1.0, got {}", c);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!((cosine(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_zero_vector() {
        let zero = vec![0.0_f32; 4];
        let v = vec![1.0_f32, 2.0, 3.0, 4.0];
        assert_eq!(cosine(&zero, &v), 0.0);
        assert_eq!(cosine(&v, &zero), 0.0);
    }

    #[test]
    fn cosine_mismatched_lengths_returns_zero() {
        let a = vec![1.0_f32, 2.0];
        let b = vec![1.0_f32, 2.0, 3.0];
        assert_eq!(cosine(&a, &b), 0.0);
    }
}
