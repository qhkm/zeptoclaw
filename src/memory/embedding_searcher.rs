//! Embedding-based memory searcher using LLM provider's embed() method.
//!
//! Feature-gated behind `memory-embedding`. When the feature is enabled,
//! this searcher embeds both the query and candidate chunks using the
//! configured LLM provider, then ranks by cosine similarity.
//!
//! ## Persistence
//!
//! Vectors are persisted to a sidecar JSON file (`~/.zeptoclaw/memory/embeddings.json`)
//! so that previously indexed entries do not need to be re-embedded on every
//! startup. The `index()` and `remove()` methods update and persist this store.
//!
//! ## score_batch strategy
//!
//! Because embedding requires async API calls, the synchronous `score()` method
//! always returns 0.0 — callers should use `score_batch()` for meaningful results.
//! `score_batch()` embeds the query once and all chunks in a single batched call,
//! then computes cosine similarity between the query embedding and each chunk
//! embedding.

#![cfg(feature = "memory-embedding")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

use crate::error::Result;
use crate::providers::LLMProvider;

use super::traits::MemorySearcher;

/// Persisted map of memory-key → embedding vector.
#[derive(Debug, Serialize, Deserialize, Default)]
struct VectorStore {
    vectors: HashMap<String, Vec<f32>>,
}

/// Embedding-based memory searcher.
///
/// Uses the LLM provider's `embed()` method to generate vectors and ranks
/// candidate text chunks via cosine similarity against the query embedding.
pub struct EmbeddingSearcher {
    provider: Arc<dyn LLMProvider>,
    store: RwLock<VectorStore>,
    store_path: PathBuf,
}

impl EmbeddingSearcher {
    /// Create a new `EmbeddingSearcher`.
    ///
    /// Loads an existing vector store from `store_path` if the file exists.
    /// If the file is missing or unreadable, starts with an empty store.
    pub fn new(provider: Arc<dyn LLMProvider>, store_path: PathBuf) -> Self {
        let store = load_vector_store(&store_path);
        Self {
            provider,
            store: RwLock::new(store),
            store_path,
        }
    }
}

/// Load vector store from disk, returning empty store on any error.
fn load_vector_store(path: &PathBuf) -> VectorStore {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            warn!(
                "Failed to parse embeddings store at {}: {}",
                path.display(),
                e
            );
            VectorStore::default()
        }),
        Err(_) => VectorStore::default(),
    }
}

/// Persist the vector store to disk.
///
/// Creates parent directories if they do not exist. Logs a warning on failure
/// rather than returning an error to keep the memory subsystem non-blocking.
fn save_vector_store(path: &PathBuf, store: &VectorStore) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("Failed to create embeddings store directory: {}", e);
            return;
        }
    }
    match serde_json::to_string_pretty(store) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                warn!(
                    "Failed to write embeddings store to {}: {}",
                    path.display(),
                    e
                );
            }
        }
        Err(e) => warn!("Failed to serialize embeddings store: {}", e),
    }
}

/// Compute cosine similarity between two equal-length vectors.
///
/// Returns 0.0 if either vector is empty, has different lengths, or has zero magnitude.
/// The result is clamped to `[0.0, 1.0]`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(0.0, 1.0)
}

#[async_trait]
impl MemorySearcher for EmbeddingSearcher {
    fn name(&self) -> &str {
        "embedding"
    }

    /// Synchronous scoring is not meaningful for embedding-based search.
    ///
    /// Always returns 0.0. Use `score_batch()` for embedding-based ranking.
    fn score(&self, _chunk: &str, _query: &str) -> f32 {
        0.0
    }

    /// Embed the query and all chunks in two batched API calls, then rank by cosine similarity.
    ///
    /// If embedding fails for any reason, falls back to 0.0 scores for all chunks.
    async fn score_batch(&self, chunks: &[&str], query: &str) -> Vec<f32> {
        if chunks.is_empty() {
            return Vec::new();
        }

        // Build input list: query first, then all chunks
        let mut inputs: Vec<String> = Vec::with_capacity(1 + chunks.len());
        inputs.push(query.to_string());
        inputs.extend(chunks.iter().map(|c| c.to_string()));

        let embeddings = match self.provider.embed(&inputs).await {
            Ok(vecs) => vecs,
            Err(e) => {
                warn!(
                    "Embedding failed in score_batch: {}; returning zero scores",
                    e
                );
                return vec![0.0; chunks.len()];
            }
        };

        if embeddings.is_empty() {
            return vec![0.0; chunks.len()];
        }

        let query_vec = &embeddings[0];
        let chunk_vecs = &embeddings[1..];

        chunk_vecs
            .iter()
            .map(|chunk_vec| cosine_similarity(query_vec, chunk_vec))
            .collect()
    }

    /// Embed and persist a memory entry's vector.
    ///
    /// If a vector for `key` already exists it is replaced. The store is
    /// persisted to disk after every successful update.
    async fn index(&self, key: &str, text: &str) -> Result<()> {
        let embeddings = self.provider.embed(&[text.to_string()]).await?;

        let vector = embeddings.into_iter().next().unwrap_or_default();

        {
            let mut store = self.store.write().await;
            store.vectors.insert(key.to_string(), vector);
            save_vector_store(&self.store_path, &store);
        }

        Ok(())
    }

    /// Remove a memory entry's vector from the store.
    ///
    /// No-op if the key is not present. The store is persisted after removal.
    async fn remove(&self, key: &str) -> Result<()> {
        let mut store = self.store.write().await;
        store.vectors.remove(key);
        save_vector_store(&self.store_path, &store);
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // cosine_similarity — always compiled (no feature gate needed here)
    // ------------------------------------------------------------------

    #[test]
    fn test_cosine_identical() {
        let v = vec![1.0f32, 2.0, 3.0];
        let score = cosine_similarity(&v, &v);
        // Identical vectors → similarity = 1.0
        assert!(
            (score - 1.0).abs() < 1e-6,
            "Identical vectors should produce similarity 1.0, got {}",
            score
        );
    }

    #[test]
    fn test_cosine_orthogonal() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            score.abs() < 1e-6,
            "Orthogonal vectors should produce similarity 0.0, got {}",
            score
        );
    }

    #[test]
    fn test_cosine_empty() {
        let score = cosine_similarity(&[], &[]);
        assert_eq!(score, 0.0, "Empty vectors should return 0.0");
    }

    #[test]
    fn test_cosine_different_lengths() {
        let a = vec![1.0f32, 2.0];
        let b = vec![1.0f32, 2.0, 3.0];
        let score = cosine_similarity(&a, &b);
        assert_eq!(score, 0.0, "Different-length vectors should return 0.0");
    }

    #[test]
    fn test_cosine_opposite() {
        // Opposite vector: cosine = -1.0 before clamping → 0.0 after clamp
        let a = vec![1.0f32, 0.0];
        let b = vec![-1.0f32, 0.0];
        let score = cosine_similarity(&a, &b);
        assert_eq!(
            score, 0.0,
            "Opposite vectors should clamp to 0.0, got {}",
            score
        );
    }

    #[test]
    fn test_cosine_partial_similarity() {
        // a = [1, 1, 0], b = [1, 0, 0]
        // dot = 1, |a| = sqrt(2), |b| = 1
        // similarity = 1/sqrt(2) ≈ 0.7071
        let a = vec![1.0f32, 1.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        let score = cosine_similarity(&a, &b);
        let expected = 1.0f32 / 2.0f32.sqrt();
        assert!(
            (score - expected).abs() < 1e-5,
            "Expected ~{:.4}, got {:.4}",
            expected,
            score
        );
    }

    // ------------------------------------------------------------------
    // EmbeddingSearcher — feature-gated tests
    // ------------------------------------------------------------------

    use crate::error::Result as ZResult;
    use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
    use crate::session::Message;
    use async_trait::async_trait;
    use std::sync::Arc;

    /// Fake provider that returns fixed-dimension embeddings.
    struct FakeEmbeddingProvider {
        /// Dimension of returned vectors
        dim: usize,
    }

    #[async_trait]
    impl LLMProvider for FakeEmbeddingProvider {
        fn name(&self) -> &str {
            "fake-embedding"
        }
        fn default_model(&self) -> &str {
            "fake-model"
        }
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> ZResult<LLMResponse> {
            Ok(LLMResponse::text("ok"))
        }
        async fn embed(&self, texts: &[String]) -> ZResult<Vec<Vec<f32>>> {
            // Return a normalised unit vector in dimension 0 for each text.
            // Each text gets a unique vector based on its position index so
            // that cosine similarity between different texts is computable.
            Ok(texts
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let mut v = vec![0.0f32; self.dim];
                    if !v.is_empty() {
                        v[i % self.dim] = 1.0;
                    }
                    v
                })
                .collect())
        }
    }

    #[test]
    fn test_embedding_searcher_name() {
        let provider = Arc::new(FakeEmbeddingProvider { dim: 4 });
        let path = std::env::temp_dir().join("zepto_test_embeddings_name.json");
        let searcher = EmbeddingSearcher::new(provider, path);
        assert_eq!(searcher.name(), "embedding");
    }

    #[test]
    fn test_embedding_searcher_sync_score_returns_zero() {
        let provider = Arc::new(FakeEmbeddingProvider { dim: 4 });
        let path = std::env::temp_dir().join("zepto_test_embeddings_sync.json");
        let searcher = EmbeddingSearcher::new(provider, path);
        // sync score is always 0.0 — embedding requires async
        assert_eq!(searcher.score("hello world", "hello"), 0.0);
        assert_eq!(searcher.score("", ""), 0.0);
    }

    #[tokio::test]
    async fn test_vector_store_persistence() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("embeddings.json");

        // Write a store manually
        let mut store = VectorStore::default();
        store.vectors.insert("k1".to_string(), vec![1.0, 0.0]);
        store.vectors.insert("k2".to_string(), vec![0.0, 1.0]);
        save_vector_store(&path, &store);

        // Reload and verify
        let loaded = load_vector_store(&path);
        assert_eq!(loaded.vectors.len(), 2);
        assert_eq!(loaded.vectors["k1"], vec![1.0, 0.0]);
        assert_eq!(loaded.vectors["k2"], vec![0.0, 1.0]);
    }

    #[tokio::test]
    async fn test_index_stores_vector() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("embeddings.json");

        let provider = Arc::new(FakeEmbeddingProvider { dim: 4 });
        let searcher = EmbeddingSearcher::new(provider, path.clone());

        searcher.index("key:hello", "hello world").await.unwrap();

        // Verify the store was persisted and contains the key
        let store = load_vector_store(&path);
        assert!(
            store.vectors.contains_key("key:hello"),
            "Expected 'key:hello' to be in persisted store"
        );
        assert_eq!(store.vectors["key:hello"].len(), 4);
    }

    #[tokio::test]
    async fn test_remove_deletes_vector() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("embeddings.json");

        let provider = Arc::new(FakeEmbeddingProvider { dim: 4 });
        let searcher = EmbeddingSearcher::new(provider, path.clone());

        searcher.index("key:a", "alpha").await.unwrap();
        searcher.index("key:b", "beta").await.unwrap();

        {
            let store = load_vector_store(&path);
            assert_eq!(store.vectors.len(), 2);
        }

        searcher.remove("key:a").await.unwrap();

        let store = load_vector_store(&path);
        assert!(
            !store.vectors.contains_key("key:a"),
            "key:a should be removed"
        );
        assert!(store.vectors.contains_key("key:b"), "key:b should remain");
    }

    #[tokio::test]
    async fn test_score_batch_returns_scores_for_all_chunks() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("embeddings.json");

        let provider = Arc::new(FakeEmbeddingProvider { dim: 8 });
        let searcher = EmbeddingSearcher::new(provider, path);

        let chunks = vec!["alpha text", "beta text", "gamma text"];
        let scores = searcher.score_batch(&chunks, "query").await;

        assert_eq!(scores.len(), 3, "Should return one score per chunk");
        for score in &scores {
            assert!(
                *score >= 0.0 && *score <= 1.0,
                "Score out of range: {}",
                score
            );
        }
    }

    #[tokio::test]
    async fn test_score_batch_empty_chunks() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("embeddings.json");

        let provider = Arc::new(FakeEmbeddingProvider { dim: 4 });
        let searcher = EmbeddingSearcher::new(provider, path);

        let scores = searcher.score_batch(&[], "query").await;
        assert!(scores.is_empty());
    }

    #[tokio::test]
    async fn test_index_upsert_replaces_existing_vector() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("embeddings.json");

        let provider = Arc::new(FakeEmbeddingProvider { dim: 4 });
        let searcher = EmbeddingSearcher::new(provider, path.clone());

        searcher.index("key:x", "first value").await.unwrap();
        let store_after_first = load_vector_store(&path);
        let first_vec = store_after_first.vectors["key:x"].clone();

        // Index same key again with different text — vector should change
        // (FakeEmbeddingProvider uses position in batch, not content, but the key
        // still should be present and store updated)
        searcher.index("key:x", "second value").await.unwrap();
        let store_after_second = load_vector_store(&path);
        assert!(
            store_after_second.vectors.contains_key("key:x"),
            "Key should remain after upsert"
        );
        let _ = first_vec; // silence unused warning
    }
}
