//! Agent loop implementation
//!
//! This module provides the core agent loop that processes messages,
//! calls LLM providers, and executes tools.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{watch, Mutex, RwLock};
use tracing::{debug, error, info, info_span, Instrument};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::Config;
use crate::error::{Result, ZeptoError};
use crate::health::UsageMetrics;
use crate::providers::{ChatOptions, LLMProvider};
use crate::session::{Message, Role, SessionManager, ToolCall};
use crate::tools::{Tool, ToolContext, ToolRegistry};

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
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Per-session locks to serialize concurrent messages for the same session
    session_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
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
        Self {
            config,
            session_manager: Arc::new(session_manager),
            bus,
            provider: Arc::new(RwLock::new(None)),
            tools: Arc::new(RwLock::new(ToolRegistry::new())),
            running: AtomicBool::new(false),
            context_builder: ContextBuilder::new(),
            usage_metrics: Arc::new(RwLock::new(None)),
            shutdown_tx,
            session_locks: Arc::new(Mutex::new(HashMap::new())),
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
        Self {
            config,
            session_manager: Arc::new(session_manager),
            bus,
            provider: Arc::new(RwLock::new(None)),
            tools: Arc::new(RwLock::new(ToolRegistry::new())),
            running: AtomicBool::new(false),
            context_builder,
            usage_metrics: Arc::new(RwLock::new(None)),
            shutdown_tx,
            session_locks: Arc::new(Mutex::new(HashMap::new())),
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

        // Get or create session
        let mut session = self.session_manager.get_or_create(&msg.session_key).await?;

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

        // Call LLM -- provider lock is NOT held during this await
        let mut response = provider
            .chat(messages, tool_definitions.clone(), model, options.clone())
            .await?;
        if let (Some(metrics), Some(usage)) = (usage_metrics.as_ref(), response.usage.as_ref()) {
            metrics.record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
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

                    async move {
                        let args: serde_json::Value = match serde_json::from_str(&raw_args) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(tool = %name, error = %e, "Invalid JSON in tool arguments");
                                serde_json::json!({"_parse_error": format!("Invalid arguments JSON: {}", e)})
                            }
                        };

                        let tool_start = std::time::Instant::now();
                        let result = {
                            let tools_guard = tools.read().await;
                            match tools_guard.execute_with_context(&name, args, &ctx).await {
                                Ok(r) => {
                                    let latency_ms = tool_start.elapsed().as_millis() as u64;
                                    debug!(tool = %name, latency_ms = latency_ms, "Tool executed successfully");
                                    r
                                }
                                Err(e) => {
                                    let latency_ms = tool_start.elapsed().as_millis() as u64;
                                    error!(tool = %name, latency_ms = latency_ms, error = %e, "Tool execution failed");
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }
                                    format!("Error: {}", e)
                                }
                            }
                        };

                        // Sanitize the result before feeding back to LLM
                        let sanitized = crate::utils::sanitize::sanitize_tool_result(
                            &result,
                            crate::utils::sanitize::DEFAULT_MAX_RESULT_BYTES,
                        );

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
}
