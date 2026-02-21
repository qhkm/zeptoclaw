//! Factory for creating the configured MemorySearcher.

use std::sync::Arc;

use tracing::warn;

use crate::config::{MemoryBackend, MemoryConfig};
use crate::providers::LLMProvider;

use super::builtin_searcher::BuiltinSearcher;
use super::traits::MemorySearcher;

/// Create the configured MemorySearcher based on config.
///
/// If the requested backend requires a cargo feature that was not compiled in,
/// logs a warning and falls back to `BuiltinSearcher`.
///
/// For the `Embedding` backend, this function cannot supply a provider and will
/// always fall back to `BuiltinSearcher`. Use [`create_searcher_with_provider`]
/// when a provider is available.
pub fn create_searcher(config: &MemoryConfig) -> Arc<dyn MemorySearcher> {
    create_searcher_with_provider(config, None)
}

/// Create the configured MemorySearcher, optionally supplying an LLM provider.
///
/// The `provider` argument is only used when `config.backend` is
/// [`MemoryBackend::Embedding`] and the `memory-embedding` cargo feature is
/// compiled in. All other backends ignore the provider.
///
/// If the `Embedding` backend is requested but no provider is given (or the
/// feature is not compiled), logs a warning and returns `BuiltinSearcher`.
pub fn create_searcher_with_provider(
    config: &MemoryConfig,
    provider: Option<Arc<dyn LLMProvider>>,
) -> Arc<dyn MemorySearcher> {
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
            #[cfg(feature = "memory-embedding")]
            {
                if let Some(p) = provider {
                    let path = crate::config::Config::dir()
                        .join("memory")
                        .join("embeddings.json");
                    Arc::new(super::embedding_searcher::EmbeddingSearcher::new(p, path))
                } else {
                    warn!("memory-embedding backend requires a provider; falling back to builtin. Pass a provider via create_searcher_with_provider()");
                    Arc::new(BuiltinSearcher)
                }
            }
            #[cfg(not(feature = "memory-embedding"))]
            {
                let _ = provider; // suppress unused warning
                warn!("memory-embedding feature not compiled; falling back to builtin. Rebuild with: cargo build --features memory-embedding");
                Arc::new(BuiltinSearcher)
            }
        }
        MemoryBackend::Hnsw => {
            #[cfg(feature = "memory-hnsw")]
            {
                if let Some(p) = provider {
                    let path = crate::config::Config::dir()
                        .join("memory")
                        .join("hnsw_vectors.json");
                    Arc::new(super::hnsw_searcher::HnswSearcher::new(p, path))
                } else {
                    warn!("memory-hnsw backend requires a provider; falling back to builtin. Pass a provider via create_searcher_with_provider()");
                    Arc::new(BuiltinSearcher)
                }
            }
            #[cfg(not(feature = "memory-hnsw"))]
            {
                let _ = provider; // suppress unused warning
                warn!("memory-hnsw feature not compiled; falling back to builtin. Rebuild with: cargo build --features memory-hnsw");
                Arc::new(BuiltinSearcher)
            }
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

    // -----------------------------------------------------------------------
    // create_searcher_with_provider tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_searcher_with_provider_none_embedding_falls_back() {
        // Without a provider, embedding backend must fall back to builtin
        // regardless of the feature flag.
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Embedding;
        let searcher = create_searcher_with_provider(&config, None);
        // Either "embedding" (if feature compiled + provider given) or "builtin"
        // Here we pass None so it MUST be "builtin".
        assert_eq!(searcher.name(), "builtin");
    }

    #[test]
    fn test_create_searcher_with_provider_builtin_ignores_provider() {
        use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
        use crate::session::Message;
        use async_trait::async_trait;
        use std::sync::Arc;

        struct NoopProvider;
        #[async_trait]
        impl LLMProvider for NoopProvider {
            fn name(&self) -> &str {
                "noop"
            }
            fn default_model(&self) -> &str {
                "noop"
            }
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> crate::error::Result<LLMResponse> {
                Ok(LLMResponse::text("ok"))
            }
        }

        let config = MemoryConfig::default(); // Builtin
        let provider: Option<Arc<dyn LLMProvider>> = Some(Arc::new(NoopProvider));
        let searcher = create_searcher_with_provider(&config, provider);
        assert_eq!(searcher.name(), "builtin");
    }

    #[cfg(feature = "memory-embedding")]
    #[test]
    fn test_create_searcher_with_provider_embedding_with_provider() {
        use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
        use crate::session::Message;
        use async_trait::async_trait;
        use std::sync::Arc;

        struct FakeProvider;
        #[async_trait]
        impl LLMProvider for FakeProvider {
            fn name(&self) -> &str {
                "fake"
            }
            fn default_model(&self) -> &str {
                "fake"
            }
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> crate::error::Result<LLMResponse> {
                Ok(LLMResponse::text("ok"))
            }
            async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
                Ok(texts.iter().map(|_| vec![1.0f32, 0.0]).collect())
            }
        }

        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Embedding;
        let provider: Option<Arc<dyn LLMProvider>> = Some(Arc::new(FakeProvider));
        let searcher = create_searcher_with_provider(&config, provider);
        assert_eq!(searcher.name(), "embedding");
    }

    #[cfg(feature = "memory-hnsw")]
    #[test]
    fn test_create_searcher_with_provider_hnsw_with_provider() {
        use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
        use crate::session::Message;
        use async_trait::async_trait;
        use std::sync::Arc;

        struct FakeProvider;
        #[async_trait]
        impl LLMProvider for FakeProvider {
            fn name(&self) -> &str {
                "fake"
            }
            fn default_model(&self) -> &str {
                "fake"
            }
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> crate::error::Result<LLMResponse> {
                Ok(LLMResponse::text("ok"))
            }
            async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
                Ok(texts.iter().map(|_| vec![1.0f32, 0.0]).collect())
            }
        }

        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Hnsw;
        let provider: Option<Arc<dyn LLMProvider>> = Some(Arc::new(FakeProvider));
        let searcher = create_searcher_with_provider(&config, provider);
        assert_eq!(searcher.name(), "hnsw");
    }

    #[test]
    fn test_create_searcher_hnsw_without_provider_falls_back() {
        // Without a provider, HNSW backend must fall back to builtin
        // regardless of whether the feature flag is enabled.
        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Hnsw;
        let searcher = create_searcher_with_provider(&config, None);
        assert_eq!(searcher.name(), "builtin");
    }
}
