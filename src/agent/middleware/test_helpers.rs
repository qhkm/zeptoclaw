//! Test utilities for middleware unit tests.
//!
//! Provides mock terminals, test contexts, and reusable middleware
//! implementations for verifying chain behavior.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{
    Middleware, OutputMode, PipelineContext, PipelineOutput, Subsystems, ToolExecutionContext,
};
use crate::agent::budget::TokenBudget;
use crate::agent::context::ContextBuilder;
use crate::agent::pipeline::{Next, Terminal, ToolExecutor};
use crate::agent::tool_call_limit::ToolCallLimitTracker;
use crate::bus::{InboundMessage, MessageBus};
use crate::config::Config;
use crate::error::{Result, ZeptoError};
use crate::session::types::ToolCall;
use crate::tools::ToolOutput;
use crate::tools::ToolRegistry;
use crate::utils::metrics::MetricsCollector;

// ---------------------------------------------------------------------------
// MockTerminal
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct MockTerminal {
    response: Option<String>,
}

impl MockTerminal {
    pub fn with_response(s: &str) -> Self {
        Self {
            response: Some(s.to_string()),
        }
    }

    /// Terminal that panics if called (for short-circuit tests).
    pub fn panicking() -> Self {
        Self { response: None }
    }
}

#[async_trait]
impl Terminal for MockTerminal {
    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<PipelineOutput> {
        match &self.response {
            Some(r) => Ok(PipelineOutput::Sync {
                response: r.clone(),
                usage: None,
                cached: false,
            }),
            None => panic!("MockTerminal::panicking() was called"),
        }
    }
}

// ---------------------------------------------------------------------------
// MockToolExecutor
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct MockToolExecutor;

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute_tool(
        &self,
        call: &ToolCall,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolOutput> {
        Ok(ToolOutput {
            for_llm: format!("executed: {}", call.name),
            for_user: None,
            is_error: false,
            is_async: false,
            pause_for_input: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Test context factories
// ---------------------------------------------------------------------------

/// Build minimal Subsystems for unit tests.
pub fn test_subsystems() -> Arc<Subsystems> {
    Arc::new(Subsystems {
        session_manager: crate::session::SessionManager::new_memory(),
        tools: Arc::new(RwLock::new(ToolRegistry::new())),
        context_builder: ContextBuilder::new(),
        context_monitor: None,
        ltm: None,
        safety_layer: None,
        taint: None,
        approval_gate: None,
        approval_handler: None,
        metrics_collector: MetricsCollector::new(),
        usage_metrics: Arc::new(RwLock::new(crate::health::UsageMetrics::default())),
        token_budget: Arc::new(TokenBudget::new(0)),
        tool_call_limit: ToolCallLimitTracker::new(None),
        cache: None,
        bus: MessageBus::new(),
        agent_mode: crate::security::AgentMode::default(),
        provider_registry: Arc::new(RwLock::new(HashMap::new())),
        tool_feedback_tx: Arc::new(RwLock::new(None)),
        #[cfg(feature = "panel")]
        event_bus: None,
    })
}

/// Build a minimal PipelineContext for unit tests.
pub fn test_context(subsystems: Arc<Subsystems>) -> PipelineContext {
    PipelineContext {
        inbound: InboundMessage {
            content: "test message".to_string(),
            session_key: "test-session".to_string(),
            channel: "test".to_string(),
            sender_id: "test-user".to_string(),
            chat_id: "test-chat".to_string(),
            media: vec![],
            metadata: HashMap::new(),
        },
        config: Arc::new(Config::default()),
        session: None,
        session_key: "test-session".to_string(),
        provider: None,
        model: None,
        chat_options: None,
        messages: None,
        tool_definitions: None,
        memory_override: None,
        output_mode: OutputMode::Sync,
        dry_run: false,
        subsystems,
    }
}

/// Build a PipelineContext with a specific channel name.
pub fn test_context_with_channel(subsystems: Arc<Subsystems>, channel: &str) -> PipelineContext {
    let mut ctx = test_context(subsystems);
    ctx.inbound.channel = channel.to_string();
    ctx
}

// ---------------------------------------------------------------------------
// Reusable test middlewares
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PassthroughMiddleware;

#[async_trait]
impl Middleware for PassthroughMiddleware {
    fn name(&self) -> &'static str {
        "passthrough"
    }
    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        next.run(ctx).await
    }
}

#[derive(Debug)]
pub struct ShortCircuitMiddleware {
    response: String,
}

impl ShortCircuitMiddleware {
    pub fn with_response(s: &str) -> Self {
        Self {
            response: s.to_string(),
        }
    }
}

#[async_trait]
impl Middleware for ShortCircuitMiddleware {
    fn name(&self) -> &'static str {
        "short_circuit"
    }
    async fn handle(&self, _ctx: &mut PipelineContext, _next: Next<'_>) -> Result<PipelineOutput> {
        Ok(PipelineOutput::Sync {
            response: self.response.clone(),
            usage: None,
            cached: false,
        })
    }
}

#[derive(Debug)]
pub struct PrefixMiddleware(pub &'static str);

#[async_trait]
impl Middleware for PrefixMiddleware {
    fn name(&self) -> &'static str {
        "prefix"
    }
    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        let output = next.run(ctx).await?;
        match output {
            PipelineOutput::Sync {
                response,
                usage,
                cached,
            } => Ok(PipelineOutput::Sync {
                response: format!("{}{}", self.0, response),
                usage,
                cached,
            }),
            other => Ok(other),
        }
    }
}

#[derive(Debug)]
pub struct ErrorMiddleware;

#[async_trait]
impl Middleware for ErrorMiddleware {
    fn name(&self) -> &'static str {
        "error"
    }
    async fn handle(&self, _ctx: &mut PipelineContext, _next: Next<'_>) -> Result<PipelineOutput> {
        Err(ZeptoError::Tool("middleware error".to_string()))
    }
}

#[derive(Debug)]
pub struct WrapMiddleware;

#[async_trait]
impl Middleware for WrapMiddleware {
    fn name(&self) -> &'static str {
        "wrap"
    }
    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        ctx.session_key = "before".to_string();
        let output = next.run(ctx).await?;
        ctx.session_key = "wrapped".to_string();
        Ok(output)
    }
}
