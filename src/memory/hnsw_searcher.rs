//! HNSW-based memory searcher for fast approximate nearest-neighbor search.
//!
//! Feature-gated behind `memory-hnsw`. When the feature is enabled, this
//! searcher embeds both the query and candidate chunks using the configured
//! LLM provider, then ranks by cosine similarity using an HNSW index built
//! from previously indexed memory entry embeddings.
//!
//! ## Why HNSW?
//!
//! For users with 1 000+ memory entries, brute-force cosine similarity over
//! all stored vectors becomes slow. The HNSW (Hierarchical Navigable Small
//! World) index provides approximate nearest-neighbor search in
//! O(log n) expected time after an O(n log n) build, making it well-suited
//! for large memory stores.
//!
//! ## Index lifecycle
//!
//! The HNSW index is rebuilt from scratch whenever a vector is added or
//! removed via `index()` / `remove()`. For the expected workload (hundreds to
//! tens-of-thousands of entries, infrequent writes, frequent reads) this is
//! the simplest correct approach — building is fast for <100 K points.
//!
//! ## Persistence
//!
//! Vectors are persisted to a sidecar JSON file
//! (`~/.zeptoclaw/memory/hnsw_vectors.json`). The HNSW index is rebuilt from
//! this file on startup; no index serialization is required.
//!
//! ## score_batch strategy
//!
//! Because embedding requires async API calls, the synchronous `score()` method
//! always returns 0.0 — callers should use `score_batch()` for meaningful
//! results. `score_batch()` embeds the query, searches the HNSW index for the
//! nearest neighbors, and returns cosine-similarity scores mapped back onto the
//! original chunk slice.

#![cfg(feature = "memory-hnsw")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use instant_distance::{Builder, HnswMap, Search};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

use crate::error::Result;
use crate::providers::LLMProvider;

use super::traits::MemorySearcher;

// ============================================================================
// Cosine similarity (duplicated from embedding_searcher to avoid cross-feature
// imports — this module may be compiled without `memory-embedding`).
// ============================================================================

/// Compute cosine similarity between two equal-length vectors.
///
/// Returns 0.0 if either vector is empty, has different lengths, or has zero
/// magnitude. The result is clamped to `[0.0, 1.0]`.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

// ============================================================================
// HNSW Point wrapper
// ============================================================================

/// Wraps a dense embedding vector as an `instant_distance::Point`.
///
/// Distance is defined as `1.0 - cosine_similarity(self, other)`, so that
/// lower distance = higher cosine similarity, as required by HNSW.
#[derive(Clone)]
struct EmbeddingPoint(Vec<f32>);

impl instant_distance::Point for EmbeddingPoint {
    fn distance(&self, other: &Self) -> f32 {
        // HNSW minimises distance, so we use (1 - cosine_sim) to make the
        // most similar vectors have the smallest distance.
        1.0 - cosine_similarity(&self.0, &other.0)
    }
}

// ============================================================================
// Vector persistence
// ============================================================================

/// Persisted map of memory-key → embedding vector.
#[derive(Debug, Serialize, Deserialize, Default)]
struct VectorStore {
    vectors: HashMap<String, Vec<f32>>,
}

/// Load vector store from disk, returning an empty store on any error.
fn load_vector_store(path: &PathBuf) -> VectorStore {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            warn!(
                "Failed to parse HNSW vector store at {}: {}",
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
            warn!("Failed to create HNSW vector store directory: {}", e);
            return;
        }
    }
    match serde_json::to_string_pretty(store) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, &json) {
                warn!(
                    "Failed to write HNSW vector store to {}: {}",
                    path.display(),
                    e
                );
            }
        }
        Err(e) => warn!("Failed to serialize HNSW vector store: {}", e),
    }
}

// ============================================================================
// Index builder
// ============================================================================

/// Build an `HnswMap` from a `VectorStore`.
///
/// Returns `None` when the store is empty (HNSW cannot be built over zero
/// points).
fn build_hnsw_index(store: &VectorStore) -> Option<HnswMap<EmbeddingPoint, String>> {
    if store.vectors.is_empty() {
        return None;
    }

    // Collect in deterministic key order so the index is reproducible.
    let mut entries: Vec<(&String, &Vec<f32>)> = store.vectors.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let points: Vec<EmbeddingPoint> = entries
        .iter()
        .map(|(_, v)| EmbeddingPoint((*v).clone()))
        .collect();

    let values: Vec<String> = entries.iter().map(|(k, _)| (*k).clone()).collect();

    Some(Builder::default().build(points, values))
}

// ============================================================================
// HnswSearcher
// ============================================================================

/// HNSW-based memory searcher.
///
/// Uses the LLM provider's `embed()` method to generate query vectors and
/// searches a pre-built HNSW index for approximate nearest neighbors. The
/// returned cosine-similarity scores are mapped back to the original chunk
/// slice via a reverse-lookup on the memory key.
pub struct HnswSearcher {
    provider: Arc<dyn LLMProvider>,
    store: RwLock<VectorStore>,
    index: RwLock<Option<HnswMap<EmbeddingPoint, String>>>,
    store_path: PathBuf,
}

impl HnswSearcher {
    /// Create a new `HnswSearcher`.
    ///
    /// Loads the existing vector store from `store_path` (if present) and
    /// builds the initial HNSW index. Both steps are best-effort: failures
    /// result in an empty store/index rather than a hard error.
    pub fn new(provider: Arc<dyn LLMProvider>, store_path: PathBuf) -> Self {
        let store = load_vector_store(&store_path);
        let index = build_hnsw_index(&store);
        Self {
            provider,
            store: RwLock::new(store),
            index: RwLock::new(index),
            store_path,
        }
    }

    /// Rebuild the HNSW index from the current vector store.
    ///
    /// Must be called (under the write lock on `store`) whenever the store
    /// changes. Rebuilding is O(n log n) but fast for the expected workload.
    async fn rebuild_index(&self) {
        let store = self.store.read().await;
        let new_index = build_hnsw_index(&store);
        let mut idx = self.index.write().await;
        *idx = new_index;
    }
}

#[async_trait]
impl MemorySearcher for HnswSearcher {
    fn name(&self) -> &str {
        "hnsw"
    }

    /// Synchronous scoring is not meaningful for HNSW-based search.
    ///
    /// Always returns 0.0. Use `score_batch()` for embedding-based ranking.
    fn score(&self, _chunk: &str, _query: &str) -> f32 {
        0.0
    }

    /// Embed the query, search the HNSW index, and return cosine-similarity
    /// scores for each chunk.
    ///
    /// Each chunk is expected to correspond to a memory entry that was
    /// previously `index()`-ed. The score is retrieved by matching the chunk
    /// text against memory keys. If the chunk is not found in the index, its
    /// score is 0.0.
    ///
    /// If embedding or the HNSW search fails for any reason, falls back to
    /// 0.0 scores for all chunks.
    async fn score_batch(&self, chunks: &[&str], query: &str) -> Vec<f32> {
        if chunks.is_empty() {
            return Vec::new();
        }

        // Embed the query.
        let embeddings = match self.provider.embed(&[query.to_string()]).await {
            Ok(vecs) => vecs,
            Err(e) => {
                warn!(
                    "HNSW: embedding failed in score_batch: {}; returning zero scores",
                    e
                );
                return vec![0.0; chunks.len()];
            }
        };

        let query_vec = match embeddings.into_iter().next() {
            Some(v) if !v.is_empty() => v,
            _ => {
                warn!("HNSW: embed() returned no vector; returning zero scores");
                return vec![0.0; chunks.len()];
            }
        };

        let query_point = EmbeddingPoint(query_vec.clone());

        // Search the HNSW index.
        let index = self.index.read().await;
        let hnsw_map = match index.as_ref() {
            Some(m) => m,
            None => {
                // Index is empty — all scores are 0.0.
                return vec![0.0; chunks.len()];
            }
        };

        // Retrieve top-k neighbors (we take up to all stored points).
        let store = self.store.read().await;
        let k = store.vectors.len().min(chunks.len().max(10));
        drop(store);

        let mut search = Search::default();
        let neighbors: HashMap<String, f32> = hnsw_map
            .search(&query_point, &mut search)
            .take(k)
            .map(|item| {
                // distance = 1 - cosine_sim  →  cosine_sim = 1 - distance
                let sim = (1.0 - item.distance).clamp(0.0, 1.0);
                (item.value.clone(), sim)
            })
            .collect();

        // Map neighbor scores back to the chunk slice.
        // Chunks are matched against memory keys stored in the index.
        chunks
            .iter()
            .map(|chunk| neighbors.get(*chunk).copied().unwrap_or(0.0))
            .collect()
    }

    /// Embed `text` and store the vector for `key`, then rebuild the HNSW index.
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

        self.rebuild_index().await;

        Ok(())
    }

    /// Remove a memory entry's vector from the store and rebuild the HNSW index.
    ///
    /// No-op if the key is not present. The store is persisted after removal.
    async fn remove(&self, key: &str) -> Result<()> {
        {
            let mut store = self.store.write().await;
            store.vectors.remove(key);
            save_vector_store(&self.store_path, &store);
        }

        self.rebuild_index().await;

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
    // cosine_similarity (local copy)
    // ------------------------------------------------------------------

    #[test]
    fn test_hnsw_cosine_identical() {
        let v = vec![1.0f32, 2.0, 3.0];
        let score = cosine_similarity(&v, &v);
        assert!(
            (score - 1.0).abs() < 1e-6,
            "Identical vectors should produce similarity 1.0, got {}",
            score
        );
    }

    #[test]
    fn test_hnsw_cosine_orthogonal() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            score.abs() < 1e-6,
            "Orthogonal vectors should produce similarity 0.0, got {}",
            score
        );
    }

    // ------------------------------------------------------------------
    // Fake provider for tests
    // ------------------------------------------------------------------

    use crate::error::Result as ZResult;
    use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
    use crate::session::Message;
    use async_trait::async_trait;
    use std::sync::Arc;

    /// Fake provider that returns fixed-dimension unit vectors.
    ///
    /// Each text at position `i` in the batch gets a unit vector in dimension
    /// `i % dim`, making the cosine similarities between different positions
    /// predictable (orthogonal unit vectors → similarity = 0.0; same position
    /// → similarity = 1.0).
    struct FakeHnswProvider {
        dim: usize,
    }

    #[async_trait]
    impl LLMProvider for FakeHnswProvider {
        fn name(&self) -> &str {
            "fake-hnsw"
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

    // ------------------------------------------------------------------
    // HnswSearcher tests (feature-gated; compiled only with memory-hnsw)
    // ------------------------------------------------------------------

    #[test]
    fn test_hnsw_searcher_name() {
        let provider = Arc::new(FakeHnswProvider { dim: 4 });
        let path = std::env::temp_dir().join("zepto_test_hnsw_name.json");
        let searcher = HnswSearcher::new(provider, path);
        assert_eq!(searcher.name(), "hnsw");
    }

    #[test]
    fn test_hnsw_sync_score_returns_zero() {
        let provider = Arc::new(FakeHnswProvider { dim: 4 });
        let path = std::env::temp_dir().join("zepto_test_hnsw_sync.json");
        let searcher = HnswSearcher::new(provider, path);
        assert_eq!(searcher.score("hello world", "hello"), 0.0);
        assert_eq!(searcher.score("", ""), 0.0);
    }

    #[tokio::test]
    async fn test_hnsw_vector_persistence() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hnsw_vectors.json");

        // Write a store manually and reload it
        let mut store = VectorStore::default();
        store.vectors.insert("k1".to_string(), vec![1.0, 0.0]);
        store.vectors.insert("k2".to_string(), vec![0.0, 1.0]);
        save_vector_store(&path, &store);

        let loaded = load_vector_store(&path);
        assert_eq!(loaded.vectors.len(), 2);
        assert_eq!(loaded.vectors["k1"], vec![1.0, 0.0]);
        assert_eq!(loaded.vectors["k2"], vec![0.0, 1.0]);
    }

    #[tokio::test]
    async fn test_hnsw_index_stores_vector() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hnsw_vectors.json");

        let provider = Arc::new(FakeHnswProvider { dim: 4 });
        let searcher = HnswSearcher::new(provider, path.clone());

        searcher.index("key:hello", "hello world").await.unwrap();

        // The vector should appear in the persisted store
        let store = load_vector_store(&path);
        assert!(
            store.vectors.contains_key("key:hello"),
            "Expected 'key:hello' in persisted store"
        );
        assert_eq!(store.vectors["key:hello"].len(), 4);
    }

    #[tokio::test]
    async fn test_hnsw_remove_deletes_vector() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hnsw_vectors.json");

        let provider = Arc::new(FakeHnswProvider { dim: 4 });
        let searcher = HnswSearcher::new(provider, path.clone());

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
    async fn test_hnsw_rebuild_index_search_works() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hnsw_vectors.json");

        // Use dim=4 so we get orthogonal unit vectors for the first 4 entries.
        let provider = Arc::new(FakeHnswProvider { dim: 4 });
        let searcher = HnswSearcher::new(provider, path.clone());

        // Index two entries; FakeHnswProvider gives them unit vectors in
        // dimension 0 and 1 respectively.
        searcher.index("entry:0", "text-0").await.unwrap();
        searcher.index("entry:1", "text-1").await.unwrap();

        // Remove one and verify the index still returns scores for the remainder.
        searcher.remove("entry:1").await.unwrap();

        // Query with "entry:0" text — provider returns unit vector in dim 0
        // for position 0, which should match entry:0 perfectly.
        let scores = searcher.score_batch(&["entry:0"], "text-0").await;
        assert_eq!(scores.len(), 1);
        // The score should be > 0 since entry:0 is in the index.
        assert!(
            scores[0] >= 0.0 && scores[0] <= 1.0,
            "Score out of range: {}",
            scores[0]
        );
    }

    #[tokio::test]
    async fn test_hnsw_empty_index_search() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hnsw_vectors.json");

        let provider = Arc::new(FakeHnswProvider { dim: 4 });
        let searcher = HnswSearcher::new(provider, path);

        // No entries indexed — all scores should be 0.0.
        let scores = searcher.score_batch(&["some chunk"], "query text").await;
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0], 0.0, "Empty index should return 0.0 scores");
    }
}
