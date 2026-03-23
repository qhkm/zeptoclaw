//! Middleware for response caching.
//!
//! Computes a cache key from `(model, system_prompt, user_content)` before
//! the LLM call.  On a cache hit the pipeline short-circuits with the
//! cached response.  On a miss the pipeline proceeds normally and the
//! result is stored (when it is a pure text reply, not a tool-call response).

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::cache::ResponseCache;
use crate::error::Result;
use crate::session::types::Role;

/// Caches LLM responses keyed by `(model, system_prompt, user_content)`.
///
/// Only the first LLM call for a given input is cacheable.  Tool-loop
/// follow-up calls are never cached because their results depend on
/// mutable external state.
#[derive(Debug)]
pub struct CacheMiddleware;

impl CacheMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CacheMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for CacheMiddleware {
    fn name(&self) -> &'static str {
        "cache"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // If no cache is configured, skip entirely.
        // Clone the Arc so we don't hold an immutable borrow on ctx.subsystems
        // across the `next.run(ctx).await` call that takes `&mut ctx`.
        let cache_mutex = match ctx.subsystems.cache {
            Some(ref c) => Arc::clone(c),
            None => return next.run(ctx).await,
        };

        // Build cache key.  We need the model name and the system prompt
        // from the context.  If `messages` haven't been built yet (earlier
        // middlewares haven't populated them), fall back to defaults.
        let model_name = ctx
            .model
            .as_deref()
            .unwrap_or(&ctx.config.agents.defaults.model);

        let system_prompt = ctx
            .messages
            .as_ref()
            .and_then(|msgs| msgs.first())
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .unwrap_or("");

        let user_content = &ctx.inbound.content;
        let key = ResponseCache::cache_key(model_name, system_prompt, user_content);

        // Check for a cache hit.  The MutexGuard must be dropped before
        // any .await point to remain Send.
        let cached_hit = cache_mutex.lock().ok().and_then(|mut c| c.get(&key));

        if let Some(cached_response) = cached_hit {
            debug!("Cache hit for initial prompt");
            return Ok(PipelineOutput::Sync {
                response: cached_response,
                usage: None,
                cached: true,
            });
        }

        // Cache miss — run the rest of the pipeline.
        let output = next.run(ctx).await?;

        // Store non-cached Sync responses in the cache.
        // Streaming responses or responses that are already cached are not stored.
        if let PipelineOutput::Sync {
            ref response,
            ref usage,
            cached: false,
        } = output
        {
            let token_count = usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0);
            if let Ok(mut cache) = cache_mutex.lock() {
                cache.put(key, response.clone(), token_count);
                debug!("Cached initial LLM response");
            }
        }

        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::cache::ResponseCache;
    use std::sync::{Arc, Mutex};

    /// Build subsystems with a real (in-memory) response cache.
    fn subsystems_with_cache() -> Arc<super::super::Subsystems> {
        let mut inner = test_subsystems_inner();
        inner.cache = Some(Arc::new(Mutex::new(ResponseCache::new(60, 100))));
        Arc::new(inner)
    }

    #[tokio::test]
    async fn no_cache_configured_passes_through() {
        let mw = CacheMiddleware::new();
        let terminal = MockTerminal::with_response("no-cache");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        // Default subsystems have cache = None
        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("no-cache"));
        assert!(!output.is_cached());
    }

    #[tokio::test]
    async fn cache_miss_calls_next_and_stores() {
        let mw = CacheMiddleware::new();
        let terminal = MockTerminal::with_response("fresh");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = subsystems_with_cache();
        let mut ctx = test_context(Arc::clone(&subsystems));
        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("fresh"));
        assert!(!output.is_cached());

        // Verify the response was stored in the cache.
        let cache_mutex = subsystems.cache.as_ref().unwrap();
        let model = &ctx.config.agents.defaults.model;
        let key = ResponseCache::cache_key(model, "", &ctx.inbound.content);
        let cached = cache_mutex.lock().unwrap().get(&key);
        assert_eq!(cached, Some("fresh".to_string()));
    }

    #[tokio::test]
    async fn cache_hit_returns_cached_response() {
        let subsystems = subsystems_with_cache();

        // Pre-populate the cache.
        {
            let cache_mutex = subsystems.cache.as_ref().unwrap();
            let model = "test-model";
            let key = ResponseCache::cache_key(model, "", "test message");
            cache_mutex
                .lock()
                .unwrap()
                .put(key, "cached-reply".to_string(), 10);
        }

        let mw = CacheMiddleware::new();
        let terminal = MockTerminal::panicking(); // should NOT be reached
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut ctx = test_context(Arc::clone(&subsystems));
        // Set model to match what we pre-populated.
        ctx.model = Some("test-model".to_string());

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("cached-reply"));
        assert!(output.is_cached());
    }

    #[tokio::test]
    async fn streaming_output_is_not_cached() {
        // This verifies the store-side guard: if the terminal somehow returns
        // Streaming, we do not attempt to cache it.  We use a custom terminal
        // that returns Streaming.
        #[derive(Debug)]
        struct StreamTerminal;
        #[async_trait]
        impl crate::agent::pipeline::Terminal for StreamTerminal {
            async fn execute(
                &self,
                _ctx: &mut PipelineContext,
            ) -> crate::error::Result<PipelineOutput> {
                Ok(PipelineOutput::Streaming)
            }
        }

        let mw = CacheMiddleware::new();
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(StreamTerminal);

        let subsystems = subsystems_with_cache();
        let mut ctx = test_context(Arc::clone(&subsystems));
        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert!(matches!(output, PipelineOutput::Streaming));

        // Cache should remain empty.
        let cache_mutex = subsystems.cache.as_ref().unwrap();
        let model = &ctx.config.agents.defaults.model;
        let key = ResponseCache::cache_key(model, "", &ctx.inbound.content);
        let cached = cache_mutex.lock().unwrap().get(&key);
        assert!(cached.is_none());
    }
}
