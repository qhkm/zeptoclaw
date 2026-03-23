//! Middleware framework for the agent pipeline.
//!
//! Each cross-cutting concern (injection scanning, token budgets, caching, etc.)
//! implements the [`Middleware`] trait. A [`Pipeline`](super::pipeline::Pipeline)
//! composes them into an ordered chain that wraps the core LLM + tool loop.

pub mod cache;
pub mod compaction;
pub mod context_build;
pub mod feedback;
pub mod injection_scan;
pub mod memory_injection;
pub mod metrics;
pub mod provider_resolution;
pub mod session;
pub mod session_save;
pub mod token_budget;

#[cfg(test)]
pub(crate) mod test_helpers;

use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};

use crate::agent::budget::TokenBudget;
use crate::agent::context::ContextBuilder;
use crate::agent::context_monitor::ContextMonitor;
use crate::agent::tool_call_limit::ToolCallLimitTracker;
use crate::agent::ToolFeedback;
use crate::bus::{InboundMessage, MessageBus};
use crate::cache::ResponseCache;
use crate::config::Config;
use crate::error::Result;
use crate::health::UsageMetrics;
use crate::memory::longterm::LongTermMemory;
use crate::providers::{ChatOptions, LLMProvider, StreamEvent, ToolDefinition, Usage};
use crate::safety::SafetyLayer;
use crate::security::AgentMode;
use crate::session::types::{Message, Session, ToolCall};
use crate::session::SessionManager;
use crate::tools::approval::{ApprovalGate, ApprovalRequest, ApprovalResponse};
use crate::tools::ToolRegistry;
use crate::tools::{ToolContext, ToolOutput};
use crate::utils::metrics::MetricsCollector;

#[cfg(feature = "panel")]
use crate::api::events::EventBus;

/// Type alias for the boxed future returned by an approval handler.
type ApprovalFuture = Pin<Box<dyn Future<Output = ApprovalResponse> + Send>>;

// ---------------------------------------------------------------------------
// Pipeline-level middleware
// ---------------------------------------------------------------------------

/// Trait for pipeline-level middleware that wraps the entire message flow.
///
/// Middlewares execute in insertion order: the first middleware added to the
/// builder is the outermost wrapper. Call `next.run(ctx)` to continue the
/// chain, or return early to short-circuit.
#[async_trait]
pub trait Middleware: Send + Sync + Debug {
    /// Human-readable name for logging and metrics.
    fn name(&self) -> &'static str;

    /// Execute this middleware's logic.
    async fn handle(
        &self,
        ctx: &mut PipelineContext,
        next: super::pipeline::Next<'_>,
    ) -> Result<PipelineOutput>;
}

// ---------------------------------------------------------------------------
// Tool-level middleware
// ---------------------------------------------------------------------------

/// Trait for per-tool-call middleware inside the CoreLoop.
///
/// Tool middlewares run for each individual tool call within a batch.
/// The innermost step calls `kernel::execute_tool()`.
#[async_trait]
pub trait ToolMiddleware: Send + Sync + Debug {
    /// Human-readable name for logging and metrics.
    fn name(&self) -> &'static str;

    /// Execute this tool middleware's logic.
    async fn handle_tool(
        &self,
        call: &ToolCall,
        ctx: &ToolExecutionContext,
        next: super::pipeline::ToolNext<'_>,
    ) -> Result<ToolOutput>;
}

// ---------------------------------------------------------------------------
// PipelineContext
// ---------------------------------------------------------------------------

/// Shared mutable context flowing through the middleware pipeline.
///
/// Fields are `Option` where they get populated by middleware execution order.
/// Each middleware reads/writes only the fields it needs.
pub struct PipelineContext {
    // --- Input (always set) ---
    pub inbound: InboundMessage,
    pub config: Arc<Config>,

    // --- Session (set by SessionMiddleware) ---
    /// Mutable session owned by PipelineContext. CoreLoop borrows it mutably
    /// during tool iterations. Wrap-middlewares must NOT hold references to
    /// session across `next.run()`.
    pub session: Option<Session>,
    pub session_key: String,

    // --- Provider (set by ProviderResolutionMiddleware) ---
    pub provider: Option<Arc<dyn LLMProvider>>,
    pub model: Option<String>,
    pub chat_options: Option<ChatOptions>,

    // --- Context (set by ContextBuildMiddleware) ---
    pub messages: Option<Vec<Message>>,
    pub tool_definitions: Option<Vec<ToolDefinition>>,
    pub memory_override: Option<String>,

    // --- Output mode ---
    pub output_mode: OutputMode,

    // --- Per-run state ---
    pub dry_run: bool,

    // --- Subsystem refs (set once at pipeline build) ---
    pub subsystems: Arc<Subsystems>,
}

/// Controls whether the final LLM response is returned synchronously
/// or streamed via a channel.
pub enum OutputMode {
    /// Return the full response as a String.
    Sync,
    /// Stream response deltas through the sender.
    Streaming { tx: mpsc::Sender<StreamEvent> },
}

impl std::fmt::Debug for OutputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputMode::Sync => write!(f, "OutputMode::Sync"),
            OutputMode::Streaming { .. } => write!(f, "OutputMode::Streaming"),
        }
    }
}

impl std::fmt::Debug for PipelineContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineContext")
            .field("session_key", &self.session_key)
            .field("output_mode", &self.output_mode)
            .field("dry_run", &self.dry_run)
            .field("has_session", &self.session.is_some())
            .field("has_provider", &self.provider.is_some())
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Subsystems
// ---------------------------------------------------------------------------

/// Immutable references to shared subsystems.
///
/// Built once at `AgentLoop` construction, `Arc`-shared across all pipeline
/// executions. Interior mutability is used where the subsystem needs writes.
pub struct Subsystems {
    pub session_manager: SessionManager,
    pub tools: Arc<RwLock<ToolRegistry>>,
    pub context_builder: ContextBuilder,
    pub context_monitor: Option<ContextMonitor>,
    pub ltm: Option<Arc<tokio::sync::Mutex<LongTermMemory>>>,
    /// Arc matches AgentLoop field type.
    pub safety_layer: Option<Arc<SafetyLayer>>,
    /// std::sync::RwLock matches AgentLoop field type.
    pub taint: Option<Arc<std::sync::RwLock<crate::safety::taint::TaintEngine>>>,
    pub approval_gate: Option<ApprovalGate>,
    pub approval_handler: Option<Arc<dyn Fn(ApprovalRequest) -> ApprovalFuture + Send + Sync>>,
    pub metrics_collector: MetricsCollector,
    pub usage_metrics: Arc<RwLock<UsageMetrics>>,
    /// Arc matches AgentLoop field type.
    pub token_budget: Arc<TokenBudget>,
    pub tool_call_limit: ToolCallLimitTracker,
    pub cache: Option<Arc<std::sync::Mutex<ResponseCache>>>,
    pub bus: MessageBus,
    pub agent_mode: AgentMode,
    pub provider_registry: Arc<RwLock<HashMap<String, Arc<dyn LLMProvider>>>>,
    pub tool_feedback_tx: Arc<RwLock<Option<mpsc::UnboundedSender<ToolFeedback>>>>,
    #[cfg(feature = "panel")]
    pub event_bus: Option<EventBus>,
}

impl Debug for Subsystems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subsystems")
            .field("session_manager", &"<SessionManager>")
            .field("tools", &"<RwLock<ToolRegistry>>")
            .field("context_builder", &"<ContextBuilder>")
            .field(
                "context_monitor",
                &self.context_monitor.as_ref().map(|_| "<ContextMonitor>"),
            )
            .field("ltm", &self.ltm.as_ref().map(|_| "<LongTermMemory>"))
            .field(
                "safety_layer",
                &self.safety_layer.as_ref().map(|_| "<SafetyLayer>"),
            )
            .field("taint", &self.taint.as_ref().map(|_| "<TaintEngine>"))
            .field(
                "approval_gate",
                &self.approval_gate.as_ref().map(|_| "<ApprovalGate>"),
            )
            .field(
                "approval_handler",
                &self.approval_handler.as_ref().map(|_| "<ApprovalHandler>"),
            )
            .field("metrics_collector", &"<MetricsCollector>")
            .field("usage_metrics", &"<RwLock<UsageMetrics>>")
            .field("token_budget", &self.token_budget)
            .field("tool_call_limit", &"<ToolCallLimitTracker>")
            .field("cache", &self.cache.as_ref().map(|_| "<ResponseCache>"))
            .field("bus", &"<MessageBus>")
            .field("agent_mode", &self.agent_mode)
            .field("provider_registry", &"<RwLock<HashMap<..>>>")
            .field("tool_feedback_tx", &"<RwLock<..>>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ToolExecutionContext
// ---------------------------------------------------------------------------

/// Context passed to tool-level middlewares for each tool call.
pub struct ToolExecutionContext {
    pub tool_context: ToolContext,
    pub subsystems: Arc<Subsystems>,
    pub dry_run: bool,
    pub tool_result_budget: usize,
}

impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("dry_run", &self.dry_run)
            .field("tool_result_budget", &self.tool_result_budget)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// PipelineOutput
// ---------------------------------------------------------------------------

/// What the pipeline produces.
#[derive(Debug)]
pub enum PipelineOutput {
    /// Synchronous response with full content.
    Sync {
        response: String,
        usage: Option<Usage>,
        cached: bool,
    },
    /// Streaming response — deltas sent via OutputMode::Streaming channel.
    Streaming,
}

impl PipelineOutput {
    /// Returns the response text, or `None` for streaming.
    pub fn response(&self) -> Option<&str> {
        match self {
            Self::Sync { response, .. } => Some(response),
            Self::Streaming => None,
        }
    }

    /// Returns token usage if available.
    pub fn usage(&self) -> Option<&Usage> {
        match self {
            Self::Sync { usage, .. } => usage.as_ref(),
            Self::Streaming => None,
        }
    }

    /// Returns `true` if this was a cache hit.
    pub fn is_cached(&self) -> bool {
        matches!(self, Self::Sync { cached: true, .. })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_output_sync_variant() {
        let output = PipelineOutput::Sync {
            response: "hello".to_string(),
            usage: None,
            cached: false,
        };
        match output {
            PipelineOutput::Sync {
                response, cached, ..
            } => {
                assert_eq!(response, "hello");
                assert!(!cached);
            }
            PipelineOutput::Streaming => panic!("expected Sync"),
        }
    }

    #[test]
    fn pipeline_output_streaming_variant() {
        let output = PipelineOutput::Streaming;
        assert!(matches!(output, PipelineOutput::Streaming));
    }

    #[test]
    fn output_mode_sync() {
        let mode = OutputMode::Sync;
        assert!(matches!(mode, OutputMode::Sync));
    }

    #[test]
    fn output_mode_streaming_with_channel() {
        let (tx, _rx) = mpsc::channel::<StreamEvent>(1);
        let mode = OutputMode::Streaming { tx };
        assert!(matches!(mode, OutputMode::Streaming { .. }));
    }

    #[test]
    fn pipeline_output_sync_accessors() {
        let output = PipelineOutput::Sync {
            response: "hello".into(),
            usage: Some(Usage::new(10, 5)),
            cached: false,
        };
        assert_eq!(output.response(), Some("hello"));
        assert!(output.usage().is_some());
        assert!(!output.is_cached());
    }

    #[test]
    fn pipeline_output_cached_accessor() {
        let output = PipelineOutput::Sync {
            response: "cached".into(),
            usage: None,
            cached: true,
        };
        assert!(output.is_cached());
        assert!(output.usage().is_none());
    }

    #[test]
    fn pipeline_output_streaming_accessors() {
        let output = PipelineOutput::Streaming;
        assert!(output.response().is_none());
        assert!(output.usage().is_none());
        assert!(!output.is_cached());
    }

    #[test]
    fn subsystems_debug_impl() {
        // Verify Debug doesn't panic — exact output not important
        let s = format!("{:?}", *test_helpers::test_subsystems());
        assert!(s.contains("Subsystems"));
    }
}
