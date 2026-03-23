//! High-level library facade for embedding ZeptoClaw as a crate.
//!
//! `ZeptoAgent` provides a simple `chat()` method with persistent conversation
//! history, suitable for embedding in GUI apps (Tauri, Electron) or other Rust
//! programs that want agent capabilities without wiring up the full
//! `AgentLoop` / `MessageBus` pipeline.
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::agent::ZeptoAgent;
//! use zeptoclaw::{ClaudeProvider, EchoTool};
//!
//! let agent = ZeptoAgent::builder()
//!     .provider(ClaudeProvider::new("sk-..."))
//!     .tool(EchoTool)
//!     .system_prompt("You are a helpful assistant.")
//!     .build()
//!     .unwrap();
//!
//! let response = agent.chat("Hello!").await.unwrap();
//! println!("{}", response);
//!
//! // History is maintained across calls
//! let response2 = agent.chat("What did I just say?").await.unwrap();
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::FutureExt;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::error::{Result, ZeptoError};
use crate::providers::{ChatOptions, LLMProvider};
use crate::safety::taint::TaintEngine;
use crate::safety::{SafetyConfig, SafetyLayer};
use crate::session::{Message, ToolCall};
use crate::tools::approval::{ApprovalConfig, ApprovalGate, ApprovalRequest, ApprovalResponse};
use crate::tools::{Tool, ToolContext, ToolRegistry};
use crate::utils::metrics::MetricsCollector;

const DEFAULT_MAX_ITERATIONS: usize = 10;
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(60);

type ApprovalFuture = Pin<Box<dyn Future<Output = ApprovalResponse> + Send>>;
type ApprovalHandler = Arc<dyn Fn(ApprovalRequest) -> ApprovalFuture + Send + Sync>;

fn preview_text(text: &str, max_chars: usize) -> String {
    let mut preview = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

async fn resolve_tool_approval(
    gate: &ApprovalGate,
    approval_handler: Option<&ApprovalHandler>,
    tool_name: &str,
    args: &Value,
) -> Option<String> {
    if !gate.requires_approval(tool_name) {
        return None;
    }

    if let Some(handler) = approval_handler {
        match handler(gate.create_request(tool_name, args)).await {
            ApprovalResponse::Approved => None,
            ApprovalResponse::Denied(reason) => Some(format!(
                "Tool '{}' was denied by user approval. {}",
                tool_name, reason
            )),
            ApprovalResponse::TimedOut => Some(format!(
                "Tool '{}' approval timed out and was not executed.",
                tool_name
            )),
        }
    } else {
        let prompt = gate.format_approval_request(tool_name, args);
        Some(format!(
            "Tool '{}' requires user approval and was not executed. {}",
            tool_name, prompt
        ))
    }
}

/// High-level agent facade for library embedding.
///
/// Holds a provider, tools, system prompt, and persistent conversation
/// history behind a `Mutex` for thread-safe concurrent access.
pub struct ZeptoAgent {
    provider: Arc<dyn LLMProvider>,
    tools: ToolRegistry,
    system_prompt: String,
    max_iterations: usize,
    history: Mutex<Vec<Message>>,
    model: Option<String>,
    tool_context: ToolContext,
    metrics: MetricsCollector,
    safety: Option<SafetyLayer>,
    taint: Option<RwLock<TaintEngine>>,
    tool_timeout: Duration,
    approval_gate: ApprovalGate,
    approval_handler: Option<ApprovalHandler>,
}

impl ZeptoAgent {
    /// Create a new builder.
    pub fn builder() -> ZeptoAgentBuilder {
        ZeptoAgentBuilder::new()
    }

    /// Send a user message and get the assistant's response.
    ///
    /// The conversation history is maintained across calls. The agent loop
    /// executes tool calls until the LLM returns a plain text response (or
    /// the iteration cap is reached).
    pub async fn chat(&self, user_message: &str) -> Result<String> {
        self.chat_with_callback(user_message, |_, _| {}).await
    }

    /// Like `chat()` but calls `on_step(tool_name, tool_result)` after each
    /// tool execution, enabling live progress updates in UIs.
    pub async fn chat_with_callback<F>(&self, user_message: &str, on_step: F) -> Result<String>
    where
        F: Fn(&str, &str),
    {
        let mut history = self.history.lock().await;

        // Append user message to history
        history.push(Message::user(user_message));

        // Build messages: system prompt + full history
        let mut messages = vec![Message::system(&self.system_prompt)];
        messages.extend(history.iter().cloned());

        let tool_defs = self.tools.definitions();
        let ctx = self.tool_context.clone();

        for iteration in 0..self.max_iterations {
            info!(
                "[ZeptoAgent] Iteration {}/{} — sending {} messages to LLM",
                iteration + 1,
                self.max_iterations,
                messages.len()
            );
            let response = self
                .provider
                .chat(
                    messages.clone(),
                    tool_defs.clone(),
                    self.model.as_deref(),
                    ChatOptions::new(),
                )
                .await?;

            if !response.has_tool_calls() {
                // Store assistant response in history and return
                info!(
                    "[ZeptoAgent] LLM returned text response: {:?}",
                    preview_text(&response.content, 200)
                );
                history.push(Message::assistant(&response.content));
                return Ok(response.content);
            }

            // Only process the FIRST tool call per LLM turn.
            // This ensures sequential execution: the LLM sees each result
            // before deciding the next action (critical for desktop automation).
            let tc = &response.tool_calls[0];
            info!(
                "[ZeptoAgent] LLM returned {} tool call(s), executing first: '{}'",
                response.tool_calls.len(),
                tc.name
            );

            let session_tool_calls = vec![ToolCall::new(&tc.id, &tc.name, &tc.arguments)];
            let assistant_msg =
                Message::assistant_with_tools(&response.content, session_tool_calls);
            messages.push(assistant_msg.clone());
            history.push(assistant_msg);

            let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);
            info!(
                "[ZeptoAgent] Executing tool '{}' with args: {}",
                tc.name, args
            );

            // Notify UI that a tool is being executed
            on_step(&tc.name, &format!("Executing: {} {}", tc.name, args));

            let result = if self.tools.has(&tc.name) {
                if let Some(denial) = resolve_tool_approval(
                    &self.approval_gate,
                    self.approval_handler.as_ref(),
                    &tc.name,
                    &args,
                )
                .await
                {
                    warn!("[ZeptoAgent] Tool '{}' blocked: {}", tc.name, denial);
                    denial
                } else {
                    let execution = std::panic::AssertUnwindSafe(async {
                        crate::kernel::execute_tool(
                            &self.tools,
                            &tc.name,
                            args,
                            &ctx,
                            self.safety.as_ref(),
                            &self.metrics,
                            self.taint.as_ref(),
                        )
                        .await
                    })
                    .catch_unwind();

                    match tokio::time::timeout(self.tool_timeout, execution).await {
                        Ok(Ok(Ok(output))) => {
                            let output_preview = preview_text(&output.for_llm, 200);
                            debug!(
                                "[ZeptoAgent] Tool '{}' succeeded: {}",
                                tc.name, output_preview
                            );
                            output.for_llm
                        }
                        Ok(Ok(Err(e))) => {
                            warn!("[ZeptoAgent] Tool '{}' failed: {}", tc.name, e);
                            format!("Tool error: {e}")
                        }
                        Ok(Err(_panic)) => {
                            warn!("[ZeptoAgent] Tool '{}' panicked during execution", tc.name);
                            format!("Tool error: Tool '{}' panicked during execution", tc.name)
                        }
                        Err(_) => {
                            warn!(
                                "[ZeptoAgent] Tool '{}' timed out after {:?}",
                                tc.name, self.tool_timeout
                            );
                            format!(
                                "Tool error: Tool '{}' timed out after {}s",
                                tc.name,
                                self.tool_timeout.as_secs_f64()
                            )
                        }
                    }
                }
            } else {
                warn!("[ZeptoAgent] Unknown tool: {}", tc.name);
                format!("Unknown tool: {}", tc.name)
            };

            // Notify UI with the result
            let result_preview = preview_text(&result, 147);
            on_step(&tc.name, &format!("Done: {}", result_preview));

            let tool_msg = Message::tool_result(&tc.id, &result);
            messages.push(tool_msg.clone());
            history.push(tool_msg);
        }

        // Safety cap reached
        let cap_msg = "I've completed the requested actions.".to_string();
        history.push(Message::assistant(&cap_msg));
        Ok(cap_msg)
    }

    /// Clear all conversation history.
    pub async fn clear_history(&self) {
        let mut history = self.history.lock().await;
        history.clear();
    }

    /// Repair history after a cancelled generation.
    ///
    /// If the last assistant message has tool_calls with no matching tool
    /// response, this removes the dangling messages to keep history valid.
    /// OpenAI requires every `tool_call_id` to have a corresponding tool
    /// response — this prevents the "tool_call_ids did not have response
    /// messages" error.
    pub async fn repair_history(&self) {
        use crate::session::Role;
        let mut history = self.history.lock().await;
        // Walk backwards: if the last message is an assistant with tool_calls
        // (i.e. not followed by a tool result), remove it.
        while let Some(last) = history.last() {
            let has_tool_calls = last.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());
            if matches!(last.role, Role::Assistant) && has_tool_calls {
                info!("[ZeptoAgent] Removing dangling assistant tool_call from history");
                history.pop();
            } else {
                break;
            }
        }
    }

    /// Get a snapshot of the current conversation history.
    pub async fn history(&self) -> Vec<Message> {
        let history = self.history.lock().await;
        history.clone()
    }

    /// Get the number of messages in the conversation history.
    pub async fn history_len(&self) -> usize {
        let history = self.history.lock().await;
        history.len()
    }

    /// Get the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Get the names of all registered tools.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.names()
    }

    /// Get the provider name.
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

/// Builder for `ZeptoAgent`.
pub struct ZeptoAgentBuilder {
    provider: Option<Arc<dyn LLMProvider>>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: Option<String>,
    max_iterations: usize,
    model: Option<String>,
    history: Vec<Message>,
    tool_context: ToolContext,
    safety: Option<SafetyLayer>,
    taint: Option<RwLock<TaintEngine>>,
    tool_timeout: Duration,
    approval_gate: ApprovalGate,
    approval_handler: Option<ApprovalHandler>,
}

impl ZeptoAgentBuilder {
    /// Create a new builder with defaults.
    pub fn new() -> Self {
        Self {
            provider: None,
            tools: Vec::new(),
            system_prompt: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            model: None,
            history: Vec::new(),
            tool_context: ToolContext::default(),
            safety: Some(SafetyLayer::new(SafetyConfig::default())),
            taint: Some(RwLock::new(TaintEngine::new(SafetyConfig::default().taint))),
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            approval_gate: ApprovalGate::new(ApprovalConfig::default()),
            approval_handler: None,
        }
    }

    /// Set the LLM provider (required).
    pub fn provider(mut self, provider: impl LLMProvider + 'static) -> Self {
        self.provider = Some(Arc::new(provider));
        self
    }

    /// Set the LLM provider from a pre-existing `Arc` (for shared providers).
    pub fn provider_arc(mut self, provider: Arc<dyn LLMProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Add a single tool.
    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.push(Box::new(tool));
        self
    }

    /// Add multiple tools at once.
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum number of tool-call iterations per chat (default: 10).
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// Override the model for this agent (otherwise uses provider default).
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Pre-load conversation history (e.g. restored from a previous session).
    ///
    /// The agent will continue the conversation from where it left off.
    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }

    /// Set the workspace used by tool executions.
    pub fn workspace(mut self, workspace: impl Into<String>) -> Self {
        self.tool_context.workspace = Some(workspace.into());
        self
    }

    /// Set the full tool execution context used by facade tool calls.
    pub fn tool_context(mut self, tool_context: ToolContext) -> Self {
        self.tool_context = tool_context;
        self
    }

    /// Set the per-tool execution timeout for facade tool calls.
    pub fn tool_timeout(mut self, tool_timeout: Duration) -> Self {
        self.tool_timeout = tool_timeout.max(Duration::from_millis(1));
        self
    }

    /// Set the approval policy used before tool execution.
    pub fn approval_config(mut self, approval_config: ApprovalConfig) -> Self {
        self.approval_gate = ApprovalGate::new(approval_config);
        self
    }

    /// Install an async approval handler for embedded frontends.
    pub fn approval_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(ApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ApprovalResponse> + Send + 'static,
    {
        self.approval_handler = Some(Arc::new(move |request| handler(request).boxed()));
        self
    }

    /// Disable the facade safety layer and taint tracking.
    ///
    /// This keeps the facade routed through the kernel path, but without
    /// input/output scanning or taint enforcement.
    pub fn without_safety(mut self) -> Self {
        self.safety = None;
        self.taint = None;
        self
    }

    /// Build the `ZeptoAgent`.
    ///
    /// Returns `Err` if no provider was set.
    pub fn build(self) -> Result<ZeptoAgent> {
        let provider = self.provider.ok_or_else(|| {
            ZeptoError::Config(
                "ZeptoAgent requires a provider. Call .provider() on the builder.".into(),
            )
        })?;

        let system_prompt = self
            .system_prompt
            .unwrap_or_else(|| "You are a helpful AI assistant.".into());

        let mut registry = ToolRegistry::new();
        for tool in self.tools {
            registry.register(tool);
        }

        Ok(ZeptoAgent {
            provider,
            tools: registry,
            system_prompt,
            max_iterations: self.max_iterations,
            history: Mutex::new(self.history),
            model: self.model,
            tool_context: self.tool_context,
            metrics: MetricsCollector::new(),
            safety: self.safety,
            taint: self.taint,
            tool_timeout: self.tool_timeout,
            approval_gate: self.approval_gate,
            approval_handler: self.approval_handler,
        })
    }
}

impl Default for ZeptoAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{LLMResponse, LLMToolCall, StreamEvent, ToolDefinition};
    use crate::tools::approval::{ApprovalConfig, ApprovalPolicyConfig};
    use crate::tools::{ToolCategory, ToolOutput};
    use crate::EchoTool;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    // MockProvider that returns a fixed response
    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Ok(LLMResponse::text(&self.response))
        }
        fn default_model(&self) -> &str {
            "mock-model"
        }
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat_stream(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![])
        }
    }

    // MockToolCallProvider that returns a tool call first, then a text response
    struct MockToolCallProvider {
        call_count: Arc<tokio::sync::Mutex<usize>>,
    }

    #[async_trait]
    impl LLMProvider for MockToolCallProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            let mut count = self.call_count.lock().await;
            *count += 1;
            if *count == 1 {
                // First call: return a tool call for echo
                Ok(LLMResponse::with_tools(
                    "",
                    vec![LLMToolCall::new(
                        "call_1",
                        "echo",
                        r#"{"message":"hello from tool"}"#,
                    )],
                ))
            } else {
                // Second call: return text
                Ok(LLMResponse::text("Done! I used the echo tool."))
            }
        }
        fn default_model(&self) -> &str {
            "mock-model"
        }
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat_stream(
            &self,
            _m: Vec<Message>,
            _t: Vec<ToolDefinition>,
            _model: Option<&str>,
            _o: ChatOptions,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![])
        }
    }

    struct ToolThenTextProvider {
        tool_name: String,
        arguments: String,
        final_response: String,
        call_count: Arc<tokio::sync::Mutex<usize>>,
    }

    #[async_trait]
    impl LLMProvider for ToolThenTextProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            let mut count = self.call_count.lock().await;
            *count += 1;
            if *count == 1 {
                Ok(LLMResponse::with_tools(
                    "",
                    vec![LLMToolCall::new("call_1", &self.tool_name, &self.arguments)],
                ))
            } else {
                Ok(LLMResponse::text(&self.final_response))
            }
        }
        fn default_model(&self) -> &str {
            "mock-model"
        }
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat_stream(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![])
        }
    }

    struct CountingTool {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "Counts tool calls for facade tests"
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Shell
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {},
            })
        }

        async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::llm_only("tool ran".to_string()))
        }
    }

    struct SlowTool;

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &str {
            "slow_tool"
        }

        fn description(&self) -> &str {
            "Sleeps long enough to trigger facade timeout"
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Shell
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {},
            })
        }

        async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(ToolOutput::llm_only("slow".to_string()))
        }
    }

    struct PanicTool;

    #[async_trait]
    impl Tool for PanicTool {
        fn name(&self) -> &str {
            "panic_tool"
        }

        fn description(&self) -> &str {
            "Panics during facade tool execution"
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Shell
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {},
            })
        }

        async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
            panic!("boom");
        }
    }

    #[tokio::test]
    async fn test_builder_no_provider() {
        let result = ZeptoAgent::builder().build();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_builder_minimal() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "hi".into(),
            })
            .build()
            .unwrap();
        assert_eq!(agent.tool_count(), 0);
        assert_eq!(agent.provider_name(), "mock");
    }

    #[tokio::test]
    async fn test_builder_with_tools() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "hi".into(),
            })
            .tool(EchoTool)
            .system_prompt("Test prompt")
            .max_iterations(5)
            .build()
            .unwrap();
        assert_eq!(agent.tool_count(), 1);
        assert_eq!(agent.tool_names(), vec!["echo"]);
    }

    #[tokio::test]
    async fn test_chat_returns_response() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "Hello there!".into(),
            })
            .build()
            .unwrap();
        let response = agent.chat("Hi").await.unwrap();
        assert_eq!(response, "Hello there!");
    }

    #[tokio::test]
    async fn test_chat_maintains_history() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "response".into(),
            })
            .build()
            .unwrap();

        agent.chat("first").await.unwrap();
        assert_eq!(agent.history_len().await, 2); // user + assistant

        agent.chat("second").await.unwrap();
        assert_eq!(agent.history_len().await, 4); // 2 user + 2 assistant
    }

    #[tokio::test]
    async fn test_clear_history() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "ok".into(),
            })
            .build()
            .unwrap();

        agent.chat("hello").await.unwrap();
        assert_eq!(agent.history_len().await, 2);

        agent.clear_history().await;
        assert_eq!(agent.history_len().await, 0);
    }

    #[tokio::test]
    async fn test_history_snapshot() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "world".into(),
            })
            .build()
            .unwrap();

        agent.chat("hello").await.unwrap();
        let history = agent.history().await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "hello");
        assert_eq!(history[1].content, "world");
    }

    #[tokio::test]
    async fn test_tool_execution_loop() {
        let agent = ZeptoAgent::builder()
            .provider(MockToolCallProvider {
                call_count: Arc::new(tokio::sync::Mutex::new(0)),
            })
            .tool(EchoTool)
            .build()
            .unwrap();

        let response = agent.chat("use echo").await.unwrap();
        assert_eq!(response, "Done! I used the echo tool.");
        // History: user + assistant_with_tools + tool_result + assistant
        assert_eq!(agent.history_len().await, 4);
    }

    #[tokio::test]
    async fn test_model_override() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "ok".into(),
            })
            .model("gpt-4o")
            .build()
            .unwrap();
        assert!(agent.model.is_some());
        assert_eq!(agent.model.as_deref(), Some("gpt-4o"));
    }

    #[tokio::test]
    async fn test_default_system_prompt() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "ok".into(),
            })
            .build()
            .unwrap();
        assert_eq!(agent.system_prompt, "You are a helpful AI assistant.");
    }

    #[tokio::test]
    async fn test_custom_system_prompt() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "ok".into(),
            })
            .system_prompt("You are ZeptoBot.")
            .build()
            .unwrap();
        assert_eq!(agent.system_prompt, "You are ZeptoBot.");
    }

    #[tokio::test]
    async fn test_tools_builder_method() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool)];
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "ok".into(),
            })
            .tools(tools)
            .build()
            .unwrap();
        assert_eq!(agent.tool_count(), 1);
    }

    #[tokio::test]
    async fn test_builder_workspace_sets_tool_context() {
        let agent = ZeptoAgent::builder()
            .provider(MockProvider {
                response: "ok".into(),
            })
            .workspace("/tmp/project")
            .build()
            .unwrap();

        assert_eq!(
            agent.tool_context.workspace.as_deref(),
            Some("/tmp/project")
        );
    }

    #[tokio::test]
    async fn test_tool_execution_respects_approval_denial() {
        let calls = Arc::new(AtomicUsize::new(0));
        let agent = ZeptoAgent::builder()
            .provider(ToolThenTextProvider {
                tool_name: "dangerous_tool".into(),
                arguments: "{}".into(),
                final_response: "done".into(),
                call_count: Arc::new(tokio::sync::Mutex::new(0)),
            })
            .tool(CountingTool {
                name: "dangerous_tool",
                calls: Arc::clone(&calls),
            })
            .approval_config(ApprovalConfig {
                enabled: true,
                policy: ApprovalPolicyConfig::RequireForTools,
                require_for: vec!["dangerous_tool".into()],
                dangerous_tools: vec![],
                auto_approve_timeout_secs: 0,
            })
            .approval_handler(|_| async { ApprovalResponse::Denied("nope".into()) })
            .build()
            .unwrap();

        let response = agent.chat("use dangerous tool").await.unwrap();
        let history = agent.history().await;

        assert_eq!(response, "done");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(history[2].content.contains("denied by user approval"));
    }

    #[tokio::test]
    async fn test_tool_execution_approval_handler_allows_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let agent = ZeptoAgent::builder()
            .provider(ToolThenTextProvider {
                tool_name: "dangerous_tool".into(),
                arguments: "{}".into(),
                final_response: "done".into(),
                call_count: Arc::new(tokio::sync::Mutex::new(0)),
            })
            .tool(CountingTool {
                name: "dangerous_tool",
                calls: Arc::clone(&calls),
            })
            .approval_config(ApprovalConfig {
                enabled: true,
                policy: ApprovalPolicyConfig::RequireForTools,
                require_for: vec!["dangerous_tool".into()],
                dangerous_tools: vec![],
                auto_approve_timeout_secs: 0,
            })
            .approval_handler(|_| async { ApprovalResponse::Approved })
            .build()
            .unwrap();

        let response = agent.chat("use dangerous tool").await.unwrap();
        let history = agent.history().await;

        assert_eq!(response, "done");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(history[2].content, "tool ran");
    }

    #[tokio::test]
    async fn test_tool_execution_timeout_is_captured() {
        let agent = ZeptoAgent::builder()
            .provider(ToolThenTextProvider {
                tool_name: "slow_tool".into(),
                arguments: "{}".into(),
                final_response: "done".into(),
                call_count: Arc::new(tokio::sync::Mutex::new(0)),
            })
            .tool(SlowTool)
            .tool_timeout(Duration::from_millis(10))
            .approval_config(ApprovalConfig {
                enabled: false,
                ..Default::default()
            })
            .build()
            .unwrap();

        let response = agent.chat("use slow tool").await.unwrap();
        let history = agent.history().await;

        assert_eq!(response, "done");
        assert!(history[2].content.contains("timed out"));
    }

    #[tokio::test]
    async fn test_tool_execution_panic_is_captured() {
        let agent = ZeptoAgent::builder()
            .provider(ToolThenTextProvider {
                tool_name: "panic_tool".into(),
                arguments: "{}".into(),
                final_response: "done".into(),
                call_count: Arc::new(tokio::sync::Mutex::new(0)),
            })
            .tool(PanicTool)
            .approval_config(ApprovalConfig {
                enabled: false,
                ..Default::default()
            })
            .build()
            .unwrap();

        let response = agent.chat("use panic tool").await.unwrap();
        let history = agent.history().await;

        assert_eq!(response, "done");
        assert!(history[2].content.contains("panicked during execution"));
    }

    #[test]
    fn test_preview_text_handles_multibyte_without_panicking() {
        let text = "你".repeat(200);
        let preview = preview_text(&text, 147);

        assert_eq!(preview.chars().count(), 150);
        assert!(preview.ends_with("..."));
        assert!(std::str::from_utf8(preview.as_bytes()).is_ok());
    }

    #[test]
    fn test_preview_text_leaves_short_text_unchanged() {
        let text = "short preview";

        assert_eq!(preview_text(text, 147), text);
    }
}
