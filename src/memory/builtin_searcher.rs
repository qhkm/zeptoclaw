//! Built-in memory searcher using substring + term matching.
//!
//! This is the default searcher — zero cost, always compiled.
//! Extracts the scoring logic that was previously inline in `score_chunk()`.

use async_trait::async_trait;

use super::traits::MemorySearcher;

/// Built-in substring + term-frequency searcher.
///
/// Scoring algorithm:
/// - Tokenize query into terms (2+ char alphanumeric tokens)
/// - Count term hits in chunk (case-insensitive)
/// - Score = coverage * 0.7 + density * 0.3 + phrase_bonus (0.25 if full phrase matches)
pub struct BuiltinSearcher;

impl BuiltinSearcher {
    /// Tokenize text into lowercase terms of 2+ alphanumeric characters.
    fn tokenize(query: &str) -> Vec<String> {
        let terms: Vec<String> = query
            .to_lowercase()
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|term| term.len() >= 2)
            .map(|term| term.to_string())
            .collect();

        if terms.is_empty() {
            vec![query.to_lowercase()]
        } else {
            terms
        }
    }
}

#[async_trait]
impl MemorySearcher for BuiltinSearcher {
    fn name(&self) -> &str {
        "builtin"
    }

    fn score(&self, chunk: &str, query: &str) -> f32 {
        let query_lower = query.to_lowercase();
        let query_terms = Self::tokenize(query);
        let chunk_lower = chunk.to_lowercase();

        let mut matched_terms = 0usize;
        let mut term_hits = 0usize;

        for term in &query_terms {
            let hits = chunk_lower.match_indices(term).count();
            if hits > 0 {
                matched_terms += 1;
                term_hits += hits;
            }
        }

        if matched_terms == 0 {
            return 0.0;
        }

        let coverage = matched_terms as f32 / query_terms.len() as f32;
        let density = (term_hits as f32 / (query_terms.len().max(1) as f32 * 2.0)).min(1.0);
        let phrase_bonus = if chunk_lower.contains(&query_lower) {
            0.25
        } else {
            0.0
        };

        (coverage * 0.7 + density * 0.3 + phrase_bonus).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        assert_eq!(BuiltinSearcher.name(), "builtin");
    }

    #[test]
    fn test_score_exact_phrase_match() {
        let score = BuiltinSearcher.score("Hello World", "hello world");
        assert!(
            score > 0.9,
            "Exact phrase match should score high: {}",
            score
        );
    }

    #[test]
    fn test_score_partial_match() {
        let score = BuiltinSearcher.score("Rust programming language", "rust");
        assert!(
            score > 0.3,
            "Partial match should score above zero: {}",
            score
        );
    }

    #[test]
    fn test_score_no_match() {
        let score = BuiltinSearcher.score("Hello World", "foobar");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_score_case_insensitive() {
        let s1 = BuiltinSearcher.score("HELLO", "hello");
        let s2 = BuiltinSearcher.score("hello", "HELLO");
        assert_eq!(s1, s2);
        assert!(s1 > 0.0);
    }
}
