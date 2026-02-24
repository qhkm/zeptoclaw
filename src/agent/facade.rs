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

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{Result, ZeptoError};
use crate::providers::{ChatOptions, LLMProvider, ToolDefinition};
use crate::session::{Message, ToolCall};
use crate::tools::{Tool, ToolContext};

const DEFAULT_MAX_ITERATIONS: usize = 10;

/// High-level agent facade for library embedding.
///
/// Holds a provider, tools, system prompt, and persistent conversation
/// history behind a `Mutex` for thread-safe concurrent access.
pub struct ZeptoAgent {
    provider: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: String,
    max_iterations: usize,
    history: Mutex<Vec<Message>>,
    model: Option<String>,
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
        let mut history = self.history.lock().await;

        // Append user message to history
        history.push(Message::user(user_message));

        // Build messages: system prompt + full history
        let mut messages = vec![Message::system(&self.system_prompt)];
        messages.extend(history.iter().cloned());

        let tool_defs: Vec<ToolDefinition> = self
            .tools
            .iter()
            .map(|t| ToolDefinition::new(t.name(), t.description(), t.parameters()))
            .collect();

        let ctx = ToolContext::default();

        for _ in 0..self.max_iterations {
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
                history.push(Message::assistant(&response.content));
                return Ok(response.content);
            }

            // Build assistant message with tool calls
            let session_tool_calls: Vec<ToolCall> = response
                .tool_calls
                .iter()
                .map(|tc| ToolCall::new(&tc.id, &tc.name, &tc.arguments))
                .collect();

            let assistant_msg =
                Message::assistant_with_tools(&response.content, session_tool_calls);
            messages.push(assistant_msg.clone());
            history.push(assistant_msg);

            // Execute each tool call
            for tc in &response.tool_calls {
                let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);

                let result = if let Some(tool) = self.tools.iter().find(|t| t.name() == tc.name) {
                    match tool.execute(args, &ctx).await {
                        Ok(output) => output.for_llm,
                        Err(e) => format!("Tool error: {e}"),
                    }
                } else {
                    format!("Unknown tool: {}", tc.name)
                };

                let tool_msg = Message::tool_result(&tc.id, &result);
                messages.push(tool_msg.clone());
                history.push(tool_msg);
            }
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
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// Get the provider name.
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

/// Builder for `ZeptoAgent`.
pub struct ZeptoAgentBuilder {
    provider: Option<Box<dyn LLMProvider>>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: Option<String>,
    max_iterations: usize,
    model: Option<String>,
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
        }
    }

    /// Set the LLM provider (required).
    pub fn provider(mut self, provider: impl LLMProvider + 'static) -> Self {
        self.provider = Some(Box::new(provider));
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

        Ok(ZeptoAgent {
            provider: Arc::from(provider),
            tools: self.tools,
            system_prompt,
            max_iterations: self.max_iterations,
            history: Mutex::new(Vec::new()),
            model: self.model,
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
    use crate::providers::{LLMResponse, LLMToolCall, StreamEvent};
    use crate::EchoTool;
    use async_trait::async_trait;
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
}
