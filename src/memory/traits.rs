//! Trait definitions for pluggable memory backends.

use async_trait::async_trait;

use crate::error::Result;

/// Pluggable search/ranking backend for memory entries.
///
/// Implementations score text chunks against queries. Used by both
/// `LongTermMemory::search()` and workspace memory chunk scoring.
#[async_trait]
pub trait MemorySearcher: Send + Sync {
    /// Backend name (e.g., "builtin", "bm25", "embedding").
    fn name(&self) -> &str;

    /// Score a text chunk against a query. Returns 0.0..=1.0.
    fn score(&self, chunk: &str, query: &str) -> f32;

    /// Batch-score for backends that benefit from batching (e.g., embedding).
    /// Default implementation calls `score()` in a loop.
    async fn score_batch(&self, chunks: &[&str], query: &str) -> Vec<f32> {
        chunks.iter().map(|c| self.score(c, query)).collect()
    }

    /// Index a new entry. No-op for stateless scorers (e.g., builtin).
    /// Stateful scorers (e.g., bm25) should override to maintain their index.
    ///
    /// Callers should invoke this when storing or updating a memory entry.
    async fn index(&self, _key: &str, _text: &str) -> Result<()> {
        Ok(())
    }

    /// Remove an entry from the index. No-op for stateless scorers.
    /// Stateful scorers (e.g., bm25) should override to keep their index in sync.
    ///
    /// Callers should invoke this when deleting a memory entry.
    async fn remove(&self, _key: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test searcher that always returns a fixed score.
    struct FixedScorer(f32);

    #[async_trait]
    impl MemorySearcher for FixedScorer {
        fn name(&self) -> &str {
            "fixed"
        }
        fn score(&self, _chunk: &str, _query: &str) -> f32 {
            self.0
        }
    }

    #[test]
    fn test_trait_object_construction() {
        let searcher: Box<dyn MemorySearcher> = Box::new(FixedScorer(0.5));
        assert_eq!(searcher.name(), "fixed");
        assert_eq!(searcher.score("hello", "world"), 0.5);
    }

    #[tokio::test]
    async fn test_default_score_batch() {
        let searcher = FixedScorer(0.7);
        let chunks = vec!["a", "b", "c"];
        let scores = searcher.score_batch(&chunks, "query").await;
        assert_eq!(scores, vec![0.7, 0.7, 0.7]);
    }

    #[tokio::test]
    async fn test_default_index_and_remove_are_noop() {
        let searcher = FixedScorer(0.0);
        assert!(searcher.index("key", "text").await.is_ok());
        assert!(searcher.remove("key").await.is_ok());
    }
}
