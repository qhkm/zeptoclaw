//! Factory for creating the configured MemorySearcher.

use std::sync::Arc;

use tracing::warn;

use crate::config::{MemoryBackend, MemoryConfig};

use super::builtin_searcher::BuiltinSearcher;
use super::traits::MemorySearcher;

/// Create the configured MemorySearcher based on config.
///
/// If the requested backend requires a cargo feature that was not compiled in,
/// logs a warning and falls back to `BuiltinSearcher`.
pub fn create_searcher(config: &MemoryConfig) -> Arc<dyn MemorySearcher> {
    match &config.backend {
        MemoryBackend::Disabled => Arc::new(BuiltinSearcher),
        MemoryBackend::Builtin => Arc::new(BuiltinSearcher),
        MemoryBackend::Qmd => {
            warn!("Memory backend 'qmd' not implemented; using builtin");
            Arc::new(BuiltinSearcher)
        }
        MemoryBackend::Bm25 => {
            #[cfg(feature = "memory-bm25")]
            {
                Arc::new(super::bm25_searcher::Bm25Searcher::new())
            }
            #[cfg(not(feature = "memory-bm25"))]
            {
                warn!("memory-bm25 feature not compiled; falling back to builtin. Rebuild with: cargo build --features memory-bm25");
                Arc::new(BuiltinSearcher)
            }
        }
        MemoryBackend::Embedding => {
            warn!("memory-embedding feature not yet implemented; falling back to builtin");
            Arc::new(BuiltinSearcher)
        }
        MemoryBackend::Hnsw => {
            warn!("memory-hnsw feature not yet implemented; falling back to builtin");
            Arc::new(BuiltinSearcher)
        }
        MemoryBackend::Tantivy => {
            warn!("memory-tantivy feature not yet implemented; falling back to builtin");
            Arc::new(BuiltinSearcher)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_searcher_builtin() {
        let config = MemoryConfig::default();
        let searcher = create_searcher(&config);
        assert_eq!(searcher.name(), "builtin");
    }

    #[test]
    fn test_create_searcher_disabled_returns_builtin() {
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Disabled;
        let searcher = create_searcher(&config);
        assert_eq!(searcher.name(), "builtin");
    }

    #[test]
    fn test_create_searcher_qmd_falls_back() {
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Qmd;
        let searcher = create_searcher(&config);
        assert_eq!(searcher.name(), "builtin");
    }

    #[test]
    fn test_create_searcher_embedding_falls_back() {
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Embedding;
        let searcher = create_searcher(&config);
        assert_eq!(searcher.name(), "builtin");
    }

    #[cfg(feature = "memory-bm25")]
    #[test]
    fn test_create_searcher_bm25() {
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Bm25;
        let searcher = create_searcher(&config);
        assert_eq!(searcher.name(), "bm25");
    }

    #[test]
    fn test_create_searcher_hnsw_falls_back() {
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Hnsw;
        let searcher = create_searcher(&config);
        assert_eq!(searcher.name(), "builtin");
    }
}
