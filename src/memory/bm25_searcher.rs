//! BM25 keyword scoring searcher.
//!
//! Pure Rust implementation of Okapi BM25 scoring. No external dependencies.
//! Feature-gated behind `memory-bm25`.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::traits::MemorySearcher;
use crate::error::Result;

/// BM25 tuning parameters.
const K1: f32 = 1.2;
const B: f32 = 0.75;

/// BM25 normalization ceiling. Scores are divided by
/// `query_terms.len() * MAX_BM25_SCORE_PER_TERM` and clamped to 0.0..1.0.
/// Empirically, a single-term BM25 score rarely exceeds ~3.0 in practice
/// (IDF ≤ ~2.3 for typical corpus sizes, TF component ≤ K1+1 = 2.2).
const MAX_BM25_SCORE_PER_TERM: f32 = 3.0;

/// BM25 keyword scoring searcher.
///
/// Maintains an inverted index of term frequencies for indexed documents.
/// Scoring without a populated index still works but without IDF weighting.
///
/// Uses `std::sync::RwLock` (not `tokio::sync::RwLock`) because `score()`
/// is a synchronous trait method. All callers are expected to run in blocking
/// contexts (e.g., `spawn_blocking`), avoiding async runtime stalls.
pub struct Bm25Searcher {
    /// Inverted index: term -> { doc_key -> term_frequency }
    index: RwLock<Bm25Index>,
}

struct Bm25Index {
    /// term -> { key -> count }
    term_docs: HashMap<String, HashMap<String, u32>>,
    /// key -> total token count
    doc_lengths: HashMap<String, u32>,
    /// Total number of indexed documents.
    doc_count: u32,
}

impl Bm25Index {
    fn new() -> Self {
        Self {
            term_docs: HashMap::new(),
            doc_lengths: HashMap::new(),
            doc_count: 0,
        }
    }

    fn avg_doc_length(&self) -> f32 {
        if self.doc_count == 0 {
            return 1.0;
        }
        let total: u32 = self.doc_lengths.values().sum();
        total as f32 / self.doc_count as f32
    }
}

impl Default for Bm25Searcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Bm25Searcher {
    pub fn new() -> Self {
        Self {
            index: RwLock::new(Bm25Index::new()),
        }
    }

    /// Tokenize text into lowercase terms.
    fn tokenize(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_string())
            .collect()
    }
}

#[async_trait]
impl MemorySearcher for Bm25Searcher {
    fn name(&self) -> &str {
        "bm25"
    }

    fn score(&self, chunk: &str, query: &str) -> f32 {
        let query_terms = Self::tokenize(query);
        if query_terms.is_empty() {
            return 0.0;
        }

        let chunk_tokens = Self::tokenize(chunk);
        let doc_len = chunk_tokens.len() as f32;
        if doc_len == 0.0 {
            return 0.0;
        }

        // Count term frequencies in this chunk
        let mut tf_map: HashMap<&str, u32> = HashMap::new();
        for token in &chunk_tokens {
            *tf_map.entry(token.as_str()).or_insert(0) += 1;
        }

        let index = self.index.read().unwrap();
        let avg_dl = index.avg_doc_length();
        let n = index.doc_count.max(1) as f32;

        let mut score = 0.0f32;
        for term in &query_terms {
            let tf = *tf_map.get(term.as_str()).unwrap_or(&0) as f32;
            if tf == 0.0 {
                continue;
            }

            // IDF: log((N - df + 0.5) / (df + 0.5) + 1)
            let df = index
                .term_docs
                .get(term)
                .map(|docs| docs.len() as f32)
                .unwrap_or(0.0);
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

            // BM25 TF component
            let tf_norm = (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * doc_len / avg_dl));

            score += idf * tf_norm;
        }

        // Normalize to 0.0..1.0 range
        let max_possible = query_terms.len() as f32 * MAX_BM25_SCORE_PER_TERM;
        (score / max_possible).clamp(0.0, 1.0)
    }

    async fn index(&self, key: &str, text: &str) -> Result<()> {
        let tokens = Self::tokenize(text);
        let mut index = self.index.write().unwrap();

        // Remove old entry if exists
        if index.doc_lengths.contains_key(key) {
            // Clean up old term_docs
            for docs in index.term_docs.values_mut() {
                docs.remove(key);
            }
            index.term_docs.retain(|_, docs| !docs.is_empty());
            index.doc_count = index.doc_count.saturating_sub(1);
        }

        // Index new entry
        let mut tf_map: HashMap<String, u32> = HashMap::new();
        for token in &tokens {
            *tf_map.entry(token.clone()).or_insert(0) += 1;
        }

        for (term, count) in tf_map {
            index
                .term_docs
                .entry(term)
                .or_default()
                .insert(key.to_string(), count);
        }

        index
            .doc_lengths
            .insert(key.to_string(), tokens.len() as u32);
        index.doc_count += 1;

        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        let mut index = self.index.write().unwrap();

        if index.doc_lengths.remove(key).is_some() {
            for docs in index.term_docs.values_mut() {
                docs.remove(key);
            }
            // Clean up empty term entries
            index.term_docs.retain(|_, docs| !docs.is_empty());
            index.doc_count = index.doc_count.saturating_sub(1);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        assert_eq!(Bm25Searcher::new().name(), "bm25");
    }

    #[test]
    fn test_score_matching_terms() {
        let searcher = Bm25Searcher::new();
        let score = searcher.score("Rust programming language", "rust programming");
        assert!(score > 0.0, "Matching terms should score > 0: {}", score);
    }

    #[test]
    fn test_score_no_match() {
        let searcher = Bm25Searcher::new();
        let score = searcher.score("Hello World", "foobar baz");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_score_empty_query() {
        let searcher = Bm25Searcher::new();
        assert_eq!(searcher.score("some text", ""), 0.0);
    }

    #[test]
    fn test_score_empty_chunk() {
        let searcher = Bm25Searcher::new();
        assert_eq!(searcher.score("", "query"), 0.0);
    }

    #[tokio::test]
    async fn test_index_improves_idf() {
        let searcher = Bm25Searcher::new();

        // Score without index
        let before = searcher.score("rust is fast", "rust");

        // Index some documents
        searcher.index("doc1", "rust is fast").await.unwrap();
        searcher.index("doc2", "python is slow").await.unwrap();
        searcher.index("doc3", "java is verbose").await.unwrap();

        // Score with index — IDF should give "rust" more weight since it appears in fewer docs
        let after = searcher.score("rust is fast", "rust");

        // Both should be positive, and IDF weighting should change the score
        assert!(before > 0.0);
        assert!(after > 0.0);
        assert_ne!(before, after, "IDF weighting should change the score");
    }

    #[tokio::test]
    async fn test_remove_cleans_index() {
        let searcher = Bm25Searcher::new();
        searcher.index("doc1", "hello world").await.unwrap();

        {
            let index = searcher.index.read().unwrap();
            assert_eq!(index.doc_count, 1);
            assert!(index.doc_lengths.contains_key("doc1"));
        }

        searcher.remove("doc1").await.unwrap();

        {
            let index = searcher.index.read().unwrap();
            assert_eq!(index.doc_count, 0);
            assert!(!index.doc_lengths.contains_key("doc1"));
        }
    }

    #[tokio::test]
    async fn test_index_upsert() {
        let searcher = Bm25Searcher::new();
        searcher.index("doc1", "hello world").await.unwrap();
        searcher.index("doc1", "goodbye world").await.unwrap();

        let index = searcher.index.read().unwrap();
        assert_eq!(index.doc_count, 1, "Upsert should not increase doc count");
    }
}
