//! Middleware for provider and model resolution.
//!
//! Resolves the LLM provider and model for the current message based on
//! metadata overrides (e.g. `provider_override`, `model_override`) or
//! config defaults.  Sets `ctx.provider` and `ctx.model` for downstream
//! middleware and the terminal executor.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::error::{Result, ZeptoError};
use crate::providers::LLMProvider;

/// Resolves the provider and model for the current pipeline execution.
///
/// Resolution order:
/// 1. `metadata["provider_override"]` → look up in `provider_registry`
/// 2. Fall back to the default provider (first registered)
///
/// For models:
/// 1. `metadata["model_override"]` if non-empty
/// 2. `config.agents.defaults.model`
#[derive(Debug)]
pub struct ProviderResolutionMiddleware;

impl ProviderResolutionMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProviderResolutionMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for ProviderResolutionMiddleware {
    fn name(&self) -> &'static str {
        "provider_resolution"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // Resolve provider: check metadata override, then fall back to registry default.
        let provider = resolve_provider(ctx).await?;
        ctx.provider = Some(provider);

        // Resolve model: check metadata override, then fall back to config default.
        let model = ctx
            .inbound
            .metadata
            .get("model_override")
            .filter(|m| !m.is_empty())
            .cloned()
            .unwrap_or_else(|| ctx.config.agents.defaults.model.clone());
        ctx.model = Some(model);

        next.run(ctx).await
    }
}

/// Look up the provider by metadata override or fall back to the first
/// registered provider in the registry.
async fn resolve_provider(ctx: &PipelineContext) -> Result<Arc<dyn LLMProvider>> {
    let registry = ctx.subsystems.provider_registry.read().await;

    // Check for explicit provider override in message metadata.
    if let Some(provider_name) = ctx
        .inbound
        .metadata
        .get("provider_override")
        .filter(|p| !p.is_empty())
    {
        if let Some(provider) = registry.get(provider_name) {
            return Ok(Arc::clone(provider));
        }
        warn!(
            provider = %provider_name,
            "Provider override not found in registry, falling back to default"
        );
    }

    // Fall back to the first registered provider (the "default").
    // In AgentLoop the default is registered under a well-known key,
    // but we accept any single entry as default.
    registry
        .values()
        .next()
        .cloned()
        .ok_or_else(|| ZeptoError::Provider("No provider configured".into()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::providers::Usage;

    /// Minimal mock provider for tests.
    #[derive(Debug)]
    struct StubProvider {
        name: String,
    }

    #[async_trait]
    impl crate::providers::LLMProvider for StubProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn default_model(&self) -> &str {
            "stub-model"
        }
        async fn chat(
            &self,
            _messages: Vec<crate::session::Message>,
            _tools: Vec<crate::providers::ToolDefinition>,
            _model: Option<&str>,
            _options: crate::providers::ChatOptions,
        ) -> crate::error::Result<crate::providers::LLMResponse> {
            Ok(crate::providers::LLMResponse {
                content: format!("from-{}", self.name),
                tool_calls: vec![],
                usage: Some(Usage::new(10, 5)),
            })
        }
    }

    fn register_provider(subsystems: &mut super::super::Subsystems, name: &str) {
        let provider: Arc<dyn LLMProvider> = Arc::new(StubProvider {
            name: name.to_string(),
        });
        // We need to set it synchronously for tests.  The RwLock in
        // test_subsystems_inner is tokio, but we can use `blocking_write`.
        subsystems
            .provider_registry
            .try_write()
            .unwrap()
            .insert(name.to_string(), provider);
    }

    #[tokio::test]
    async fn resolves_default_provider_and_model() {
        let mw = ProviderResolutionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut inner = test_subsystems_inner();
        register_provider(&mut inner, "default");
        let subsystems = Arc::new(inner);
        let mut ctx = test_context(subsystems);

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert!(ctx.provider.is_some());
        assert_eq!(
            ctx.model.as_deref(),
            Some(ctx.config.agents.defaults.model.as_str())
        );
    }

    #[tokio::test]
    async fn model_override_from_metadata() {
        let mw = ProviderResolutionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut inner = test_subsystems_inner();
        register_provider(&mut inner, "default");
        let subsystems = Arc::new(inner);
        let mut ctx = test_context(subsystems);
        ctx.inbound
            .metadata
            .insert("model_override".to_string(), "gpt-5".to_string());

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(ctx.model.as_deref(), Some("gpt-5"));
    }

    #[tokio::test]
    async fn provider_override_from_metadata() {
        let mw = ProviderResolutionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut inner = test_subsystems_inner();
        register_provider(&mut inner, "default");
        register_provider(&mut inner, "openai");
        let subsystems = Arc::new(inner);
        let mut ctx = test_context(subsystems);
        ctx.inbound
            .metadata
            .insert("provider_override".to_string(), "openai".to_string());

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        let provider = ctx.provider.as_ref().unwrap();
        assert_eq!(provider.name(), "openai");
    }

    #[tokio::test]
    async fn missing_provider_override_falls_back() {
        let mw = ProviderResolutionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut inner = test_subsystems_inner();
        register_provider(&mut inner, "default");
        let subsystems = Arc::new(inner);
        let mut ctx = test_context(subsystems);
        ctx.inbound
            .metadata
            .insert("provider_override".to_string(), "nonexistent".to_string());

        // Should fall back to the default provider instead of erroring.
        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert!(ctx.provider.is_some());
    }

    #[tokio::test]
    async fn no_provider_returns_error() {
        let mw = ProviderResolutionMiddleware::new();
        let terminal = MockTerminal::panicking();
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems(); // empty registry
        let mut ctx = test_context(subsystems);

        let result = pipeline.execute(&mut ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No provider"),
            "Expected provider error, got: {err}"
        );
    }
}
