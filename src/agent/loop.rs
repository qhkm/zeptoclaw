//! Agent loop implementation
//!
//! This module provides the core agent loop that processes messages,
//! calls LLM providers, and executes tools.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{watch, Mutex, RwLock};
use tracing::{debug, error, info, info_span, Instrument};

use crate::agent::context_monitor::ContextMonitor;
use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::Config;
use crate::error::{Result, ZeptoError};
use crate::health::UsageMetrics;
use crate::providers::{ChatOptions, LLMProvider};
use crate::safety::SafetyLayer;
use crate::session::{Message, Role, SessionManager, ToolCall};
use crate::tools::approval::ApprovalGate;
use crate::tools::{Tool, ToolContext, ToolRegistry};
use crate::utils::metrics::MetricsCollector;

use super::budget::TokenBudget;
use super::context::ContextBuilder;

/// The main agent loop that processes messages and coordinates with LLM providers.
///
/// The `AgentLoop` is responsible for:
/// - Receiving messages from the message bus
/// - Building conversation context with session history
/// - Calling the LLM provider for responses
/// - Executing tool calls and feeding results back to the LLM
/// - Publishing responses back to the message bus
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use zeptoclaw::agent::AgentLoop;
/// use zeptoclaw::bus::MessageBus;
/// use zeptoclaw::config::Config;
/// use zeptoclaw::session::SessionManager;
///
/// let config = Config::default();
/// let session_manager = SessionManager::new_memory();
/// let bus = Arc::new(MessageBus::new());
/// let agent = AgentLoop::new(config, session_manager, bus);
///
/// // Configure provider and tools
/// agent.set_provider(Box::new(my_provider)).await;
/// agent.register_tool(Box::new(my_tool)).await;
///
/// // Start processing messages
/// agent.start().await?;
/// ```
pub struct AgentLoop {
    /// Agent configuration
    config: Config,
    /// Session manager for conversation state
    session_manager: Arc<SessionManager>,
    /// Message bus for input/output
    bus: Arc<MessageBus>,
    /// The LLM provider to use (Arc<dyn ..> allows cheap cloning without holding the lock)
    provider: Arc<RwLock<Option<Arc<dyn LLMProvider>>>>,
    /// Registered tools
    tools: Arc<RwLock<ToolRegistry>>,
    /// Whether the loop is currently running
    running: AtomicBool,
    /// Context builder for constructing LLM messages
    context_builder: ContextBuilder,
    /// Optional usage metrics sink for gateway observability
    usage_metrics: Arc<RwLock<Option<Arc<UsageMetrics>>>>,
    /// Per-agent metrics collector for tool and token tracking.
    metrics_collector: Arc<MetricsCollector>,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Per-session locks to serialize concurrent messages for the same session
    session_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// Pending messages for sessions with active runs (for queue modes).
    pending_messages: Arc<Mutex<HashMap<String, Vec<InboundMessage>>>>,
    /// Whether to stream the final LLM response in CLI mode.
    streaming: AtomicBool,
    /// Per-session token budget tracker.
    token_budget: Arc<TokenBudget>,
    /// Tool approval gate for policy-based tool gating.
    approval_gate: Arc<ApprovalGate>,
    /// Optional safety layer for tool output sanitization.
    safety_layer: Option<Arc<SafetyLayer>>,
    /// Optional context monitor for compaction.
    context_monitor: Option<ContextMonitor>,
}

impl AgentLoop {
    /// Create a new agent loop.
    ///
    /// # Arguments
    /// * `config` - The agent configuration
    /// * `session_manager` - Session manager for conversation state
    /// * `bus` - Message bus for receiving and sending messages
    ///
    /// # Example
    /// ```rust
    /// use std::sync::Arc;
    /// use zeptoclaw::agent::AgentLoop;
    /// use zeptoclaw::bus::MessageBus;
    /// use zeptoclaw::config::Config;
    /// use zeptoclaw::session::SessionManager;
    ///
    /// let config = Config::default();
    /// let session_manager = SessionManager::new_memory();
    /// let bus = Arc::new(MessageBus::new());
    /// let agent = AgentLoop::new(config, session_manager, bus);
    /// assert!(!agent.is_running());
    /// ```
    pub fn new(config: Config, session_manager: SessionManager, bus: Arc<MessageBus>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        let token_budget = Arc::new(TokenBudget::new(config.agents.defaults.token_budget));
        let approval_gate = Arc::new(ApprovalGate::new(config.approval.clone()));
        let safety_layer = if config.safety.enabled {
            Some(Arc::new(SafetyLayer::new(config.safety.clone())))
        } else {
            None
        };
        let context_monitor = if config.compaction.enabled {
            Some(ContextMonitor::new(
                config.compaction.context_limit,
                config.compaction.threshold,
            ))
        } else {
            None
        };
        Self {
            config,
            session_manager: Arc::new(session_manager),
            bus,
            provider: Arc::new(RwLock::new(None)),
            tools: Arc::new(RwLock::new(ToolRegistry::new())),
            running: AtomicBool::new(false),
            context_builder: ContextBuilder::new(),
            usage_metrics: Arc::new(RwLock::new(None)),
            metrics_collector: Arc::new(MetricsCollector::new()),
            shutdown_tx,
            session_locks: Arc::new(Mutex::new(HashMap::new())),
            pending_messages: Arc::new(Mutex::new(HashMap::new())),
            streaming: AtomicBool::new(false),
            token_budget,
            approval_gate,
            safety_layer,
            context_monitor,
        }
    }

    /// Create a new agent loop with a custom context builder.
    ///
    /// # Arguments
    /// * `config` - The agent configuration
    /// * `session_manager` - Session manager for conversation state
    /// * `bus` - Message bus for receiving and sending messages
    /// * `context_builder` - Custom context builder
    pub fn with_context_builder(
        config: Config,
        session_manager: SessionManager,
        bus: Arc<MessageBus>,
        context_builder: ContextBuilder,
    ) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        let token_budget = Arc::new(TokenBudget::new(config.agents.defaults.token_budget));
        let approval_gate = Arc::new(ApprovalGate::new(config.approval.clone()));
        let safety_layer = if config.safety.enabled {
            Some(Arc::new(SafetyLayer::new(config.safety.clone())))
        } else {
            None
        };
        let context_monitor = if config.compaction.enabled {
            Some(ContextMonitor::new(
                config.compaction.context_limit,
                config.compaction.threshold,
            ))
        } else {
            None
        };
        Self {
            config,
            session_manager: Arc::new(session_manager),
            bus,
            provider: Arc::new(RwLock::new(None)),
            tools: Arc::new(RwLock::new(ToolRegistry::new())),
            running: AtomicBool::new(false),
            context_builder,
            usage_metrics: Arc::new(RwLock::new(None)),
            metrics_collector: Arc::new(MetricsCollector::new()),
            shutdown_tx,
            session_locks: Arc::new(Mutex::new(HashMap::new())),
            pending_messages: Arc::new(Mutex::new(HashMap::new())),
            streaming: AtomicBool::new(false),
            token_budget,
            approval_gate,
            safety_layer,
            context_monitor,
        }
    }

    /// Check if the agent loop is currently running.
    ///
    /// # Returns
    /// `true` if the loop is running, `false` otherwise.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Set the LLM provider to use.
    ///
    /// # Arguments
    /// * `provider` - The LLM provider implementation
    ///
    /// # Example
    /// ```rust,ignore
    /// use zeptoclaw::providers::ClaudeProvider;
    ///
    /// let provider = ClaudeProvider::new("api-key");
    /// agent.set_provider(Box::new(provider)).await;
    /// ```
    pub async fn set_provider(&self, provider: Box<dyn LLMProvider>) {
        let mut p = self.provider.write().await;
        *p = Some(Arc::from(provider));
    }

    /// Enable usage metrics collection for this agent loop.
    pub async fn set_usage_metrics(&self, metrics: Arc<UsageMetrics>) {
        let mut usage_metrics = self.usage_metrics.write().await;
        *usage_metrics = Some(metrics);
    }

    /// Get the per-agent metrics collector.
    pub fn metrics_collector(&self) -> Arc<MetricsCollector> {
        Arc::clone(&self.metrics_collector)
    }

    /// Register a tool with the agent.
    ///
    /// # Arguments
    /// * `tool` - The tool to register
    ///
    /// # Example
    /// ```rust,ignore
    /// use zeptoclaw::tools::EchoTool;
    ///
    /// agent.register_tool(Box::new(EchoTool)).await;
    /// ```
    pub async fn register_tool(&self, tool: Box<dyn Tool>) {
        let mut tools = self.tools.write().await;
        tools.register(tool);
    }

    /// Get the number of registered tools.
    pub async fn tool_count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.len()
    }

    /// Check if a tool is registered.
    pub async fn has_tool(&self, name: &str) -> bool {
        let tools = self.tools.read().await;
        tools.has(name)
    }

    /// Process a single inbound message.
    ///
    /// This method:
    /// 1. Gets or creates a session for the message
    /// 2. Builds the conversation context
    /// 3. Calls the LLM provider
    /// 4. Executes any tool calls
    /// 5. Continues the tool loop until no more tool calls
    /// 6. Returns the final response
    ///
    /// # Arguments
    /// * `msg` - The inbound message to process
    ///
    /// # Returns
    /// The assistant's final response text.
    ///
    /// # Errors
    /// Returns an error if:
    /// - No provider is configured
    /// - The LLM call fails
    /// - Session management fails
    pub async fn process_message(&self, msg: &InboundMessage) -> Result<String> {
        // Acquire a per-session lock to serialize concurrent messages for the
        // same session key. Different sessions can still proceed concurrently.
        let session_lock = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(msg.session_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _session_guard = session_lock.lock().await;

        // Clone the provider Arc early and release the RwLock immediately.
        // This avoids holding the provider read lock across multi-second LLM
        // calls and tool executions, which would block set_provider() writes.
        let provider = {
            let guard = self.provider.read().await;
            Arc::clone(
                guard
                    .as_ref()
                    .ok_or_else(|| ZeptoError::Provider("No provider configured".into()))?,
            )
        };
        let usage_metrics = {
            let metrics = self.usage_metrics.read().await;
            metrics.clone()
        };
        let metrics_collector = Arc::clone(&self.metrics_collector);

        // Get or create session
        let mut session = self.session_manager.get_or_create(&msg.session_key).await?;

        // Apply three-tier context overflow recovery if needed
        if let Some(ref monitor) = self.context_monitor {
            if monitor.needs_compaction(&session.messages) {
                let context_limit = self.config.compaction.context_limit;
                let (recovered, tier) = crate::agent::compaction::try_recover_context(
                    session.messages,
                    context_limit,
                    8,    // keep_recent for tier 1
                    5120, // 5KB tool result budget for tier 2
                );
                if tier > 0 {
                    debug!(
                        tier = tier,
                        "Context recovered via tier {} compaction", tier
                    );
                }
                session.messages = recovered;
            }
        }

        // Build messages with history
        let messages = self
            .context_builder
            .build_messages(session.messages.clone(), &msg.content);

        // Get tool definitions (short-lived read lock)
        let tool_definitions = {
            let tools = self.tools.read().await;
            tools.definitions()
        };

        // Build chat options
        let options = ChatOptions::new()
            .with_max_tokens(self.config.agents.defaults.max_tokens)
            .with_temperature(self.config.agents.defaults.temperature);

        let model = Some(self.config.agents.defaults.model.as_str());

        // Check token budget before first LLM call
        if self.token_budget.is_exceeded() {
            return Err(ZeptoError::Provider(format!(
                "Token budget exceeded: {}",
                self.token_budget.summary()
            )));
        }

        // Call LLM -- provider lock is NOT held during this await
        let mut response = provider
            .chat(messages, tool_definitions.clone(), model, options.clone())
            .await?;
        if let (Some(metrics), Some(usage)) = (usage_metrics.as_ref(), response.usage.as_ref()) {
            metrics.record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
        }
        if let Some(usage) = response.usage.as_ref() {
            metrics_collector
                .record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
            self.token_budget
                .record(usage.prompt_tokens as u64, usage.completion_tokens as u64);
        }

        // Add user message to session
        session.add_message(Message::user(&msg.content));

        // Tool loop
        let max_iterations = self.config.agents.defaults.max_tool_iterations;
        let mut iteration = 0;

        while response.has_tool_calls() && iteration < max_iterations {
            iteration += 1;
            debug!("Tool iteration {} of {}", iteration, max_iterations);
            if let Some(metrics) = usage_metrics.as_ref() {
                metrics.record_tool_calls(response.tool_calls.len() as u64);
            }

            // Add assistant message with tool calls
            let mut assistant_msg = Message::assistant(&response.content);
            assistant_msg.tool_calls = Some(
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .collect(),
            );
            session.add_message(assistant_msg);

            // Execute tool calls in parallel
            let workspace = self.config.workspace_path();
            let workspace_str = workspace.to_string_lossy();
            let tool_ctx = ToolContext::new()
                .with_channel(&msg.channel, &msg.chat_id)
                .with_workspace(&workspace_str);

            let approval_gate = Arc::clone(&self.approval_gate);
            let safety_layer = self.safety_layer.clone();
            let hook_engine = Arc::new(
                crate::hooks::HookEngine::new(self.config.hooks.clone())
                    .with_bus(Arc::clone(&self.bus)),
            );

            // Compute dynamic tool result budget based on remaining context space
            let current_tokens = ContextMonitor::estimate_tokens(&session.messages);
            let context_limit = self.config.compaction.context_limit;
            let result_budget = crate::utils::sanitize::compute_tool_result_budget(
                context_limit,
                current_tokens,
                response.tool_calls.len(),
            );

            let tool_futures: Vec<_> = response
                .tool_calls
                .iter()
                .map(|tool_call| {
                    let tools = Arc::clone(&self.tools);
                    let ctx = tool_ctx.clone();
                    let name = tool_call.name.clone();
                    let id = tool_call.id.clone();
                    let raw_args = tool_call.arguments.clone();
                    let usage_metrics = usage_metrics.clone();
                    let metrics_collector = Arc::clone(&metrics_collector);
                    let gate = Arc::clone(&approval_gate);
                    let hooks = Arc::clone(&hook_engine);
                    let safety = safety_layer.clone();
                    let budget = result_budget;

                    async move {
                        let args: serde_json::Value = match serde_json::from_str(&raw_args) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(tool = %name, error = %e, "Invalid JSON in tool arguments");
                                serde_json::json!({"_parse_error": format!("Invalid arguments JSON: {}", e)})
                            }
                        };

                        // Check hooks before executing
                        let channel_name = ctx.channel.as_deref().unwrap_or("cli");
                        let chat_id = ctx.chat_id.as_deref().unwrap_or(channel_name);
                        if let crate::hooks::HookResult::Block(msg) =
                            hooks.before_tool(&name, &args, channel_name, chat_id)
                        {
                            return (id, format!("Tool '{}' blocked by hook: {}", name, msg));
                        }

                        // Check approval gate before executing
                        if gate.requires_approval(&name) {
                            let prompt = gate.format_approval_request(&name, &args);
                            info!(tool = %name, "Tool requires approval, blocking execution");
                            return (id, format!("Tool '{}' requires user approval and was not executed. {}", name, prompt));
                        }

                        let tool_start = std::time::Instant::now();
                        let (result, success) = {
                            let tools_guard = tools.read().await;
                            match tools_guard.execute_with_context(&name, args, &ctx).await {
                                Ok(r) => {
                                    let elapsed = tool_start.elapsed();
                                    let latency_ms = elapsed.as_millis() as u64;
                                    debug!(tool = %name, latency_ms = latency_ms, "Tool executed successfully");
                                    hooks.after_tool(&name, &r, elapsed, channel_name, chat_id);
                                    (r, true)
                                }
                                Err(e) => {
                                    let elapsed = tool_start.elapsed();
                                    let latency_ms = elapsed.as_millis() as u64;
                                    error!(tool = %name, latency_ms = latency_ms, error = %e, "Tool execution failed");
                                    hooks.on_error(&name, &e.to_string(), channel_name, chat_id);
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }
                                    (format!("Error: {}", e), false)
                                }
                            }
                        };
                        metrics_collector.record_tool_call(&name, tool_start.elapsed(), success);

                        // Sanitize the result with dynamic budget
                        let sanitized = crate::utils::sanitize::sanitize_tool_result(
                            &result,
                            budget,
                        );

                        // Apply safety layer if enabled
                        let sanitized = if let Some(ref safety) = safety {
                            let safety_result = safety.check_tool_output(&sanitized);
                            if safety_result.blocked {
                                format!(
                                    "[Safety blocked]: {}",
                                    safety_result.block_reason.unwrap_or_default()
                                )
                            } else {
                                safety_result.content
                            }
                        } else {
                            sanitized
                        };

                        (id, sanitized)
                    }
                })
                .collect();

            let results = futures::future::join_all(tool_futures).await;

            for (id, result) in results {
                session.add_message(Message::tool_result(&id, &result));
            }

            // Get fresh tool definitions for the next LLM call
            let tool_definitions = {
                let tools = self.tools.read().await;
                tools.definitions()
            };

            // Check token budget before next LLM call
            if self.token_budget.is_exceeded() {
                info!(budget = %self.token_budget.summary(), "Token budget exceeded during tool loop");
                break;
            }

            // Call LLM again with tool results -- provider lock NOT held
            let messages: Vec<_> = self
                .context_builder
                .build_messages(session.messages.clone(), "")
                .into_iter()
                .filter(|m| !(m.role == Role::User && m.content.is_empty()))
                .collect();

            response = provider
                .chat(messages, tool_definitions, model, options.clone())
                .await?;
            if let (Some(metrics), Some(usage)) = (usage_metrics.as_ref(), response.usage.as_ref())
            {
                metrics.record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
            }
            if let Some(usage) = response.usage.as_ref() {
                metrics_collector
                    .record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
                self.token_budget
                    .record(usage.prompt_tokens as u64, usage.completion_tokens as u64);
            }
        }

        if iteration >= max_iterations && response.has_tool_calls() {
            info!(
                iterations = iteration,
                "Tool loop reached maximum iterations, returning partial response"
            );
        }

        // Add final assistant response
        session.add_message(Message::assistant(&response.content));
        self.session_manager.save(&session).await?;

        Ok(response.content)
    }

    /// Process a message with streaming output for the final LLM response.
    ///
    /// This method works like `process_message()` but streams the final response
    /// token-by-token through the returned receiver. Tool loop iterations are
    /// still non-streaming. The assembled final response is returned via
    /// `StreamEvent::Done`.
    pub async fn process_message_streaming(
        &self,
        msg: &InboundMessage,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::providers::StreamEvent>> {
        use crate::providers::StreamEvent;

        // Acquire per-session lock
        let session_lock = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(msg.session_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _session_guard = session_lock.lock().await;

        let provider = {
            let guard = self.provider.read().await;
            Arc::clone(
                guard
                    .as_ref()
                    .ok_or_else(|| ZeptoError::Provider("No provider configured".into()))?,
            )
        };
        let metrics_collector = Arc::clone(&self.metrics_collector);

        let mut session = self.session_manager.get_or_create(&msg.session_key).await?;

        // Apply three-tier context overflow recovery if needed (streaming)
        if let Some(ref monitor) = self.context_monitor {
            if monitor.needs_compaction(&session.messages) {
                let context_limit = self.config.compaction.context_limit;
                let (recovered, tier) = crate::agent::compaction::try_recover_context(
                    session.messages,
                    context_limit,
                    8,    // keep_recent for tier 1
                    5120, // 5KB tool result budget for tier 2
                );
                if tier > 0 {
                    debug!(
                        tier = tier,
                        "Context recovered via tier {} compaction (streaming)", tier
                    );
                }
                session.messages = recovered;
            }
        }

        let messages = self
            .context_builder
            .build_messages(session.messages.clone(), &msg.content);

        let tool_definitions = {
            let tools = self.tools.read().await;
            tools.definitions()
        };

        let options = ChatOptions::new()
            .with_max_tokens(self.config.agents.defaults.max_tokens)
            .with_temperature(self.config.agents.defaults.temperature);
        let model = Some(self.config.agents.defaults.model.as_str());

        // Check token budget before first LLM call
        if self.token_budget.is_exceeded() {
            return Err(ZeptoError::Provider(format!(
                "Token budget exceeded: {}",
                self.token_budget.summary()
            )));
        }

        // First call: non-streaming to see if there are tool calls
        let mut response = provider
            .chat(messages, tool_definitions.clone(), model, options.clone())
            .await?;
        if let Some(usage) = response.usage.as_ref() {
            self.token_budget
                .record(usage.prompt_tokens as u64, usage.completion_tokens as u64);
        }

        session.add_message(Message::user(&msg.content));

        // Tool loop (non-streaming)
        let max_iterations = self.config.agents.defaults.max_tool_iterations;
        let mut iteration = 0;

        while response.has_tool_calls() && iteration < max_iterations {
            iteration += 1;

            let mut assistant_msg = Message::assistant(&response.content);
            assistant_msg.tool_calls = Some(
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .collect(),
            );
            session.add_message(assistant_msg);

            let workspace = self.config.workspace_path();
            let workspace_str = workspace.to_string_lossy();
            let tool_ctx = ToolContext::new()
                .with_channel(&msg.channel, &msg.chat_id)
                .with_workspace(&workspace_str);

            let approval_gate = Arc::clone(&self.approval_gate);
            let safety_layer_stream = self.safety_layer.clone();

            // Compute dynamic tool result budget based on remaining context space
            let current_tokens_stream = ContextMonitor::estimate_tokens(&session.messages);
            let context_limit_stream = self.config.compaction.context_limit;
            let result_budget_stream = crate::utils::sanitize::compute_tool_result_budget(
                context_limit_stream,
                current_tokens_stream,
                response.tool_calls.len(),
            );

            let tool_futures: Vec<_> = response
                .tool_calls
                .iter()
                .map(|tool_call| {
                    let tools = Arc::clone(&self.tools);
                    let ctx = tool_ctx.clone();
                    let name = tool_call.name.clone();
                    let id = tool_call.id.clone();
                    let raw_args = tool_call.arguments.clone();
                    let metrics_collector = Arc::clone(&metrics_collector);
                    let gate = Arc::clone(&approval_gate);
                    let safety = safety_layer_stream.clone();
                    let budget = result_budget_stream;

                    async move {
                        let args: serde_json::Value = serde_json::from_str(&raw_args)
                            .unwrap_or_else(|_| serde_json::json!({}));

                        // Check approval gate before executing
                        if gate.requires_approval(&name) {
                            let prompt = gate.format_approval_request(&name, &args);
                            info!(tool = %name, "Tool requires approval, blocking execution");
                            return (
                                id,
                                format!(
                                    "Tool '{}' requires user approval and was not executed. {}",
                                    name, prompt
                                ),
                            );
                        }

                        let tool_start = std::time::Instant::now();
                        let (result, success) = {
                            let tools_guard = tools.read().await;
                            match tools_guard.execute_with_context(&name, args, &ctx).await {
                                Ok(r) => (r, true),
                                Err(e) => (format!("Error: {}", e), false),
                            }
                        };
                        metrics_collector.record_tool_call(&name, tool_start.elapsed(), success);
                        let sanitized =
                            crate::utils::sanitize::sanitize_tool_result(&result, budget);

                        // Apply safety layer if enabled
                        let sanitized = if let Some(ref safety) = safety {
                            let safety_result = safety.check_tool_output(&sanitized);
                            if safety_result.blocked {
                                format!(
                                    "[Safety blocked]: {}",
                                    safety_result.block_reason.unwrap_or_default()
                                )
                            } else {
                                safety_result.content
                            }
                        } else {
                            sanitized
                        };

                        (id, sanitized)
                    }
                })
                .collect();

            let results = futures::future::join_all(tool_futures).await;
            for (id, result) in results {
                session.add_message(Message::tool_result(&id, &result));
            }

            let tool_definitions = {
                let tools = self.tools.read().await;
                tools.definitions()
            };

            // Check token budget before next LLM call
            if self.token_budget.is_exceeded() {
                info!(budget = %self.token_budget.summary(), "Token budget exceeded during streaming tool loop");
                break;
            }

            let messages: Vec<_> = self
                .context_builder
                .build_messages(session.messages.clone(), "")
                .into_iter()
                .filter(|m| !(m.role == Role::User && m.content.is_empty()))
                .collect();

            response = provider
                .chat(messages, tool_definitions, model, options.clone())
                .await?;
            if let Some(usage) = response.usage.as_ref() {
                metrics_collector
                    .record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
                self.token_budget
                    .record(usage.prompt_tokens as u64, usage.completion_tokens as u64);
            }
        }

        // Final call: if no more tool calls, use streaming
        if !response.has_tool_calls() {
            // Re-issue the final call via chat_stream
            let messages: Vec<_> = self
                .context_builder
                .build_messages(session.messages.clone(), "")
                .into_iter()
                .filter(|m| !(m.role == Role::User && m.content.is_empty()))
                .collect();

            let tool_definitions = {
                let tools = self.tools.read().await;
                tools.definitions()
            };

            let stream_rx = provider
                .chat_stream(messages, tool_definitions, model, options)
                .await?;

            // Wrap in a forwarding task that also saves the session
            let (out_tx, out_rx) = tokio::sync::mpsc::channel::<StreamEvent>(32);
            let session_manager = Arc::clone(&self.session_manager);
            let session_clone = session.clone();
            let metrics_collector = Arc::clone(&metrics_collector);

            tokio::spawn(async move {
                let mut session = session_clone;
                let mut stream_rx = stream_rx;

                while let Some(event) = stream_rx.recv().await {
                    match &event {
                        StreamEvent::Done { content, usage } => {
                            if let Some(usage) = usage.as_ref() {
                                metrics_collector.record_tokens(
                                    usage.prompt_tokens as u64,
                                    usage.completion_tokens as u64,
                                );
                            }
                            session.add_message(Message::assistant(content));
                            let _ = session_manager.save(&session).await;
                            let _ = out_tx.send(event).await;
                            return;
                        }
                        StreamEvent::ToolCalls(_) => {
                            // Unexpected tool calls during streaming — emit and let caller handle
                            let _ = out_tx.send(event).await;
                            return;
                        }
                        _ => {
                            if out_tx.send(event).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });

            Ok(out_rx)
        } else {
            // Still has tool calls after max iterations — return non-streaming result
            session.add_message(Message::assistant(&response.content));
            self.session_manager.save(&session).await?;

            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let _ = tx
                .send(StreamEvent::Done {
                    content: response.content,
                    usage: response.usage,
                })
                .await;
            Ok(rx)
        }
    }

    /// Try to queue a message if the session is busy, or return false if lock is free.
    /// Returns `true` if the message was queued (caller should not wait for response).
    pub async fn try_queue_or_process(&self, msg: &InboundMessage) -> bool {
        let session_lock = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(msg.session_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Try to acquire the lock without blocking
        let is_busy = session_lock.try_lock().is_err();

        if is_busy {
            // Session is busy, queue the message
            let mut pending = self.pending_messages.lock().await;
            pending
                .entry(msg.session_key.clone())
                .or_default()
                .push(msg.clone());
            debug!(session = %msg.session_key, "Message queued (session busy)");
            true
        } else {
            // Lock acquired and immediately dropped — caller should process normally
            // The real lock is acquired in process_message
            false
        }
    }

    /// Start the agent loop (consuming from message bus).
    ///
    /// This method runs in a loop, consuming messages from the inbound
    /// channel and publishing responses to the outbound channel.
    ///
    /// The loop continues until `stop()` is called.
    ///
    /// # Errors
    /// Returns an error if the loop is already running.
    ///
    /// # Example
    /// ```rust,ignore
    /// // Start in a separate task
    /// let agent_clone = agent.clone();
    /// tokio::spawn(async move {
    ///     agent_clone.start().await.unwrap();
    /// });
    ///
    /// // Later, stop the loop
    /// agent.stop();
    /// ```
    pub async fn start(&self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(ZeptoError::Config("Agent loop already running".into()));
        }
        info!("Starting agent loop");

        // Subscribe fresh and consume any stale stop signal from a previous run.
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let _ = *shutdown_rx.borrow_and_update();

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Received shutdown signal");
                        break;
                    }
                }
                // Wait for inbound messages
                msg = self.bus.consume_inbound() => {
                    if let Some(msg) = msg {
                        let tenant_id = msg
                            .metadata
                            .get("tenant_id")
                            .filter(|v| !v.is_empty())
                            .map(String::as_str)
                            .unwrap_or(&msg.chat_id);
                        let request_id = uuid::Uuid::new_v4();
                        let request_span = info_span!(
                            "request",
                            request_id = %request_id,
                            tenant_id = %tenant_id,
                            chat_id = %msg.chat_id,
                            session_id = %msg.session_key,
                            channel = %msg.channel,
                            sender = %msg.sender_id,
                        );
                        let msg_ref = &msg;
                        let bus_ref = &self.bus;
                        let usage_metrics = {
                            let metrics = self.usage_metrics.read().await;
                            metrics.clone()
                        };
                        async {
                            info!("Processing message");
                            let start = std::time::Instant::now();
                            let tokens_before = usage_metrics.as_ref().map(|m| {
                                (
                                    m.input_tokens.load(std::sync::atomic::Ordering::Relaxed),
                                    m.output_tokens.load(std::sync::atomic::Ordering::Relaxed),
                                )
                            });
                            if let Some(metrics) = usage_metrics.as_ref() {
                                metrics.record_request();
                            }

                            let timeout_duration = std::time::Duration::from_secs(
                                self.config.agents.defaults.agent_timeout_secs,
                            );
                            let process_result = tokio::time::timeout(
                                timeout_duration,
                                self.process_message(msg_ref),
                            )
                            .await;

                            match process_result {
                                Ok(Ok(response)) => {
                                    let latency_ms = start.elapsed().as_millis() as u64;
                                    let (input_tokens, output_tokens) = tokens_before
                                        .and_then(|(ib, ob)| {
                                            usage_metrics.as_ref().map(|m| {
                                                let ia = m.input_tokens.load(std::sync::atomic::Ordering::Relaxed);
                                                let oa = m.output_tokens.load(std::sync::atomic::Ordering::Relaxed);
                                                (ia.saturating_sub(ib), oa.saturating_sub(ob))
                                            })
                                        })
                                        .unwrap_or((0, 0));
                                    info!(
                                        latency_ms = latency_ms,
                                        response_len = response.len(),
                                        input_tokens = input_tokens,
                                        output_tokens = output_tokens,
                                        "Request completed"
                                    );

                                    let outbound = OutboundMessage::new(&msg_ref.channel, &msg_ref.chat_id, &response);
                                    if let Err(e) = bus_ref.publish_outbound(outbound).await {
                                        error!("Failed to publish outbound message: {}", e);
                                        if let Some(metrics) = usage_metrics.as_ref() {
                                            metrics.record_error();
                                        }
                                    }
                                }
                                Ok(Err(e)) => {
                                    let latency_ms = start.elapsed().as_millis() as u64;
                                    error!(latency_ms = latency_ms, error = %e, "Request failed");
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }

                                    let error_msg = OutboundMessage::new(
                                        &msg_ref.channel,
                                        &msg_ref.chat_id,
                                        &format!("Error: {}", e),
                                    );
                                    bus_ref.publish_outbound(error_msg).await.ok();
                                }
                                Err(_elapsed) => {
                                    let timeout_secs = self.config.agents.defaults.agent_timeout_secs;
                                    error!(timeout_secs = timeout_secs, "Agent run timed out");
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }

                                    let timeout_msg = OutboundMessage::new(
                                        &msg_ref.channel,
                                        &msg_ref.chat_id,
                                        &format!("Agent run timed out after {}s. Try a simpler request.", timeout_secs),
                                    );
                                    bus_ref.publish_outbound(timeout_msg).await.ok();
                                }
                            }

                            // After processing, drain any pending messages for this session
                            let pending = {
                                let mut map = self.pending_messages.lock().await;
                                map.remove(&msg_ref.session_key).unwrap_or_default()
                            };

                            if !pending.is_empty() {
                                match self.config.agents.defaults.message_queue_mode {
                                    crate::config::MessageQueueMode::Collect => {
                                        // Concatenate all pending messages into one
                                        let combined: Vec<String> = pending
                                            .iter()
                                            .enumerate()
                                            .map(|(i, m)| format!("{}. {}", i + 1, m.content))
                                            .collect();
                                        let combined_content = format!(
                                            "[Queued messages while I was busy]\n\n{}",
                                            combined.join("\n")
                                        );
                                        let synthetic = InboundMessage::new(
                                            &msg_ref.channel,
                                            &msg_ref.sender_id,
                                            &msg_ref.chat_id,
                                            &combined_content,
                                        );
                                        if let Err(e) = bus_ref.publish_inbound(synthetic).await {
                                            error!("Failed to re-queue collected messages: {}", e);
                                        }
                                    }
                                    crate::config::MessageQueueMode::Followup => {
                                        // Replay each pending message as a separate inbound
                                        for pending_msg in pending {
                                            if let Err(e) = bus_ref.publish_inbound(pending_msg).await {
                                                error!("Failed to re-queue followup message: {}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }.instrument(request_span).await;
                    } else {
                        // Channel closed, exit loop
                        info!("Inbound channel closed");
                        break;
                    }
                }
            }

            // Also check the running flag (belt and suspenders)
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
        }

        self.running.store(false, Ordering::SeqCst);
        info!("Agent loop stopped");
        Ok(())
    }

    /// Stop the agent loop.
    ///
    /// This signals the loop to stop immediately (after completing any
    /// in-progress message processing). The `start()` method will return
    /// after the loop stops.
    pub fn stop(&self) {
        info!("Stopping agent loop");
        self.running.store(false, Ordering::SeqCst);
        // Send shutdown signal to wake up the select! loop
        let _ = self.shutdown_tx.send(true);
    }

    /// Get a reference to the session manager.
    pub fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
    }

    /// Get a reference to the message bus.
    pub fn bus(&self) -> &Arc<MessageBus> {
        &self.bus
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get a clone of the current LLM provider Arc, if configured.
    pub async fn provider(&self) -> Option<Arc<dyn LLMProvider>> {
        let guard = self.provider.read().await;
        guard.clone()
    }

    /// Set whether to stream the final LLM response.
    pub fn set_streaming(&self, enabled: bool) {
        self.streaming.store(enabled, Ordering::SeqCst);
    }

    /// Check if streaming is enabled.
    pub fn is_streaming(&self) -> bool {
        self.streaming.load(Ordering::SeqCst)
    }

    /// Get a reference to the token budget tracker.
    pub fn token_budget(&self) -> &TokenBudget {
        &self.token_budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_loop_creation() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = AgentLoop::new(config, session_manager, bus);

        assert!(!agent.is_running());
    }

    #[tokio::test]
    async fn test_agent_loop_with_context_builder() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let context_builder = ContextBuilder::new().with_system_prompt("Custom prompt");

        let agent = AgentLoop::with_context_builder(config, session_manager, bus, context_builder);

        assert!(!agent.is_running());
    }

    #[tokio::test]
    async fn test_agent_loop_tool_registration() {
        use crate::tools::EchoTool;

        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = AgentLoop::new(config, session_manager, bus);

        assert_eq!(agent.tool_count().await, 0);
        assert!(!agent.has_tool("echo").await);

        agent.register_tool(Box::new(EchoTool)).await;

        assert_eq!(agent.tool_count().await, 1);
        assert!(agent.has_tool("echo").await);
    }

    #[tokio::test]
    async fn test_agent_loop_accessors() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = AgentLoop::new(config, session_manager, bus);

        // Test accessors don't panic
        let _ = agent.config();
        let _ = agent.bus();
        let _ = agent.session_manager();
    }

    #[tokio::test]
    async fn test_process_message_no_provider() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = AgentLoop::new(config, session_manager, bus);

        let msg = InboundMessage::new("test", "user123", "chat456", "Hello");
        let result = agent.process_message(&msg).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ZeptoError::Provider(_)));
        assert!(err.to_string().contains("No provider configured"));
    }

    #[tokio::test]
    async fn test_agent_loop_start_stop() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = Arc::new(AgentLoop::new(config, session_manager, bus.clone()));

        assert!(!agent.is_running());

        // Start in background task
        let agent_clone = Arc::clone(&agent);
        let handle = tokio::spawn(async move { agent_clone.start().await });

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert!(agent.is_running());

        // Stop it
        agent.stop();

        // Send a dummy message to unblock the consume_inbound call
        let dummy_msg = InboundMessage::new("test", "user", "chat", "dummy");
        bus.publish_inbound(dummy_msg).await.ok();

        // Wait for the task to complete
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(200), handle).await;

        assert!(result.is_ok());
        assert!(!agent.is_running());
    }

    #[tokio::test]
    async fn test_agent_loop_double_start() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = Arc::new(AgentLoop::new(config, session_manager, bus.clone()));

        // Start first instance
        let agent_clone = Arc::clone(&agent);
        let handle = tokio::spawn(async move { agent_clone.start().await });

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Try to start again - should fail
        let result = agent.start().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already running"));

        // Cleanup
        agent.stop();
        // Send a dummy message to unblock the consume_inbound call
        let dummy_msg = InboundMessage::new("test", "user", "chat", "dummy");
        bus.publish_inbound(dummy_msg).await.ok();

        let _ = tokio::time::timeout(tokio::time::Duration::from_millis(200), handle).await;
    }

    #[tokio::test]
    async fn test_agent_loop_graceful_shutdown() {
        // Test that stop() works immediately without needing a dummy message
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = Arc::new(AgentLoop::new(config, session_manager, bus));

        // Start in background task
        let agent_clone = Arc::clone(&agent);
        let handle = tokio::spawn(async move { agent_clone.start().await });

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert!(agent.is_running());

        // Stop without sending any message - should work with graceful shutdown
        agent.stop();

        // Should complete within a reasonable time (no dummy message needed)
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(100), handle).await;

        assert!(
            result.is_ok(),
            "Agent loop should stop gracefully without needing a message"
        );
        assert!(!agent.is_running());
    }

    #[tokio::test]
    async fn test_agent_loop_can_restart_after_stop() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = Arc::new(AgentLoop::new(config, session_manager, bus));

        // First run
        let agent_clone = Arc::clone(&agent);
        let first = tokio::spawn(async move { agent_clone.start().await });
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        agent.stop();
        let first_result =
            tokio::time::timeout(tokio::time::Duration::from_millis(200), first).await;
        assert!(first_result.is_ok());
        assert!(!agent.is_running());

        // Restart same instance and ensure it keeps running until explicitly stopped.
        let agent_clone = Arc::clone(&agent);
        let second = tokio::spawn(async move { agent_clone.start().await });
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert!(agent.is_running());
        agent.stop();
        let second_result =
            tokio::time::timeout(tokio::time::Duration::from_millis(200), second).await;
        assert!(second_result.is_ok());
        assert!(!agent.is_running());
    }

    #[test]
    fn test_context_builder_standalone() {
        let builder = ContextBuilder::new();
        let system = builder.build_system_message();
        assert!(system.content.contains("ZeptoClaw"));
    }

    #[test]
    fn test_build_messages_standalone() {
        let builder = ContextBuilder::new();
        let messages = builder.build_messages(vec![], "Hello");
        assert_eq!(messages.len(), 2);
        assert!(messages[1].content == "Hello");
    }

    #[tokio::test]
    async fn test_agent_loop_streaming_flag_default() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = AgentLoop::new(config, session_manager, bus);
        assert!(!agent.is_streaming());
    }

    #[tokio::test]
    async fn test_agent_loop_set_streaming() {
        let config = Config::default();
        let session_manager = SessionManager::new_memory();
        let bus = Arc::new(MessageBus::new());
        let agent = AgentLoop::new(config, session_manager, bus);
        agent.set_streaming(true);
        assert!(agent.is_streaming());
    }
}
