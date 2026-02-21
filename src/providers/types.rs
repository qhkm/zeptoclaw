//! Provider types for ZeptoClaw
//!
//! This module defines the core types and traits for LLM providers,
//! including the `LLMProvider` trait, chat options, and response types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Result, ZeptoError};
use crate::providers::structured::OutputFormat;
use crate::session::Message;

/// Events emitted during streaming LLM responses.
#[derive(Debug)]
pub enum StreamEvent {
    /// A chunk of text content from the LLM.
    Delta(String),
    /// Tool calls detected mid-stream (triggers fallback to non-streaming tool loop).
    ToolCalls(Vec<LLMToolCall>),
    /// Stream complete — carries the full assembled content and usage stats.
    Done {
        content: String,
        usage: Option<Usage>,
    },
    /// Provider error mid-stream.
    Error(ZeptoError),
}

/// Definition of a tool that can be called by the LLM.
///
/// Tool definitions describe the available tools, their parameters,
/// and how the LLM should invoke them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name of the tool (must be unique)
    pub name: String,
    /// Human-readable description of what the tool does
    pub description: String,
    /// JSON Schema describing the tool's parameters
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition.
    ///
    /// # Arguments
    /// * `name` - Unique identifier for the tool
    /// * `description` - Human-readable description
    /// * `parameters` - JSON Schema for the tool's parameters
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ToolDefinition;
    /// use serde_json::json;
    ///
    /// let tool = ToolDefinition::new(
    ///     "web_search",
    ///     "Search the web for information",
    ///     json!({
    ///         "type": "object",
    ///         "properties": {
    ///             "query": { "type": "string", "description": "Search query" }
    ///         },
    ///         "required": ["query"]
    ///     }),
    /// );
    /// ```
    pub fn new(name: &str, description: &str, parameters: serde_json::Value) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        }
    }
}

/// Trait for LLM providers (OpenAI, Anthropic, etc.).
///
/// Implement this trait to add support for a new LLM provider.
/// The provider is responsible for translating between ZeptoClaw's
/// message format and the provider's API format.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Send a chat completion request to the LLM.
    ///
    /// # Arguments
    /// * `messages` - The conversation history
    /// * `tools` - Available tools the LLM can call
    /// * `model` - Optional model override (uses default if None)
    /// * `options` - Additional options like temperature, max_tokens, etc.
    ///
    /// # Returns
    /// The LLM's response, which may include text content and/or tool calls.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse>;

    /// Get the default model for this provider.
    ///
    /// # Returns
    /// The model identifier string (e.g., "gpt-4", "claude-3-opus")
    fn default_model(&self) -> &str;

    /// Get the provider name.
    ///
    /// # Returns
    /// The provider name (e.g., "openai", "anthropic")
    fn name(&self) -> &str;

    /// Send a streaming chat completion request.
    ///
    /// Returns an `mpsc::Receiver` that yields `StreamEvent`s.
    /// The default implementation wraps `chat()` and emits a single `Done` event.
    /// Providers that support SSE streaming should override this.
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let response = self.chat(messages, tools, model, options).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let _ = tx
            .send(StreamEvent::Done {
                content: response.content,
                usage: response.usage,
            })
            .await;
        Ok(rx)
    }

    /// Embed texts into vector representations.
    ///
    /// Returns one embedding vector per input text. The dimensionality depends on
    /// the underlying model (e.g., 1536 for `text-embedding-3-small`).
    ///
    /// The default implementation returns an error because not all providers
    /// support embeddings. Override this on providers that do (e.g., OpenAI).
    async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Err(ZeptoError::Provider(
            "Embedding not supported by this provider".into(),
        ))
    }
}

/// Options for chat completion requests.
///
/// Use the builder pattern to construct options.
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    /// Maximum number of tokens to generate
    pub max_tokens: Option<u32>,
    /// Temperature for sampling (0.0 = deterministic, 1.0 = creative)
    pub temperature: Option<f32>,
    /// Nucleus sampling parameter
    pub top_p: Option<f32>,
    /// Stop sequences that halt generation
    pub stop: Option<Vec<String>>,
    /// Output format (text, JSON, or JSON schema)
    pub output_format: OutputFormat,
}

impl ChatOptions {
    /// Create new default chat options.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ChatOptions;
    ///
    /// let options = ChatOptions::new();
    /// assert!(options.max_tokens.is_none());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of tokens to generate.
    ///
    /// # Arguments
    /// * `max_tokens` - Maximum tokens in the response
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ChatOptions;
    ///
    /// let options = ChatOptions::new().with_max_tokens(1000);
    /// assert_eq!(options.max_tokens, Some(1000));
    /// ```
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the temperature for sampling.
    ///
    /// Lower values (e.g., 0.2) make output more focused and deterministic.
    /// Higher values (e.g., 0.8) make output more creative and diverse.
    ///
    /// # Arguments
    /// * `temperature` - Temperature value (typically 0.0 to 1.0)
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ChatOptions;
    ///
    /// let options = ChatOptions::new().with_temperature(0.7);
    /// assert_eq!(options.temperature, Some(0.7));
    /// ```
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set the top_p (nucleus sampling) parameter.
    ///
    /// # Arguments
    /// * `top_p` - Nucleus sampling threshold (0.0 to 1.0)
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ChatOptions;
    ///
    /// let options = ChatOptions::new().with_top_p(0.9);
    /// assert_eq!(options.top_p, Some(0.9));
    /// ```
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set stop sequences that will halt generation.
    ///
    /// # Arguments
    /// * `stop` - List of stop sequences
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ChatOptions;
    ///
    /// let options = ChatOptions::new().with_stop(vec!["END".to_string()]);
    /// assert!(options.stop.is_some());
    /// ```
    pub fn with_stop(mut self, stop: Vec<String>) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Set the output format for structured responses.
    ///
    /// # Arguments
    /// * `output_format` - The desired output format (text, JSON, or JSON schema)
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::ChatOptions;
    /// use zeptoclaw::providers::structured::OutputFormat;
    ///
    /// let options = ChatOptions::new().with_output_format(OutputFormat::json());
    /// assert!(options.output_format.is_json());
    /// ```
    pub fn with_output_format(mut self, output_format: OutputFormat) -> Self {
        self.output_format = output_format;
        self
    }
}

/// Response from an LLM chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Text content of the response
    pub content: String,
    /// Tool calls made by the LLM (if any)
    pub tool_calls: Vec<LLMToolCall>,
    /// Token usage information (if available)
    pub usage: Option<Usage>,
}

impl LLMResponse {
    /// Create a simple text response with no tool calls.
    ///
    /// # Arguments
    /// * `content` - The text content
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::LLMResponse;
    ///
    /// let response = LLMResponse::text("Hello, world!");
    /// assert_eq!(response.content, "Hello, world!");
    /// assert!(!response.has_tool_calls());
    /// ```
    pub fn text(content: &str) -> Self {
        Self {
            content: content.to_string(),
            tool_calls: vec![],
            usage: None,
        }
    }

    /// Create a response with tool calls.
    ///
    /// # Arguments
    /// * `content` - Optional text content
    /// * `tool_calls` - The tool calls
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::{LLMResponse, LLMToolCall};
    ///
    /// let tool_call = LLMToolCall::new("call_1", "search", r#"{"query": "rust"}"#);
    /// let response = LLMResponse::with_tools("Searching...", vec![tool_call]);
    /// assert!(response.has_tool_calls());
    /// ```
    pub fn with_tools(content: &str, tool_calls: Vec<LLMToolCall>) -> Self {
        Self {
            content: content.to_string(),
            tool_calls,
            usage: None,
        }
    }

    /// Check if this response contains any tool calls.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::LLMResponse;
    ///
    /// let response = LLMResponse::text("No tools here");
    /// assert!(!response.has_tool_calls());
    /// ```
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Set usage information for this response.
    ///
    /// # Arguments
    /// * `usage` - Token usage information
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

/// A tool call made by the LLM.
///
/// This represents the LLM's request to execute a specific tool
/// with given arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMToolCall {
    /// Unique identifier for this tool call
    pub id: String,
    /// Name of the tool to execute
    pub name: String,
    /// JSON-encoded arguments for the tool
    pub arguments: String,
}

impl LLMToolCall {
    /// Create a new tool call.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this call
    /// * `name` - Name of the tool to execute
    /// * `arguments` - JSON-encoded arguments
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::LLMToolCall;
    ///
    /// let call = LLMToolCall::new("call_123", "web_search", r#"{"query": "rust"}"#);
    /// assert_eq!(call.name, "web_search");
    /// ```
    pub fn new(id: &str, name: &str, arguments: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    /// Parse the arguments as a specific type.
    ///
    /// # Returns
    /// The parsed arguments, or an error if parsing fails.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::LLMToolCall;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct SearchArgs {
    ///     query: String,
    /// }
    ///
    /// let call = LLMToolCall::new("call_1", "search", r#"{"query": "rust"}"#);
    /// let args: SearchArgs = call.parse_arguments().unwrap();
    /// assert_eq!(args.query, "rust");
    /// ```
    pub fn parse_arguments<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(&self.arguments)
    }
}

/// Token usage information from a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    /// Number of tokens in the prompt
    pub prompt_tokens: u32,
    /// Number of tokens in the completion
    pub completion_tokens: u32,
    /// Total tokens used (prompt + completion)
    pub total_tokens: u32,
}

impl Usage {
    /// Create new usage information.
    ///
    /// # Arguments
    /// * `prompt_tokens` - Tokens in the prompt
    /// * `completion_tokens` - Tokens in the completion
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::Usage;
    ///
    /// let usage = Usage::new(100, 50);
    /// assert_eq!(usage.total_tokens, 150);
    /// ```
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_response_creation() {
        let response = LLMResponse {
            content: "Hello".to_string(),
            tool_calls: vec![],
            usage: None,
        };
        assert_eq!(response.content, "Hello");
        assert!(!response.has_tool_calls());
    }

    #[test]
    fn test_llm_response_text() {
        let response = LLMResponse::text("Hello, world!");
        assert_eq!(response.content, "Hello, world!");
        assert!(!response.has_tool_calls());
        assert!(response.usage.is_none());
    }

    #[test]
    fn test_llm_response_with_tools() {
        let tool_call = LLMToolCall::new("call_1", "search", r#"{"query": "rust"}"#);
        let response = LLMResponse::with_tools("Searching...", vec![tool_call]);

        assert_eq!(response.content, "Searching...");
        assert!(response.has_tool_calls());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "search");
    }

    #[test]
    fn test_llm_response_with_usage() {
        let usage = Usage::new(100, 50);
        let response = LLMResponse::text("Hello").with_usage(usage);

        assert!(response.usage.is_some());
        let usage = response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_chat_options_builder() {
        let options = ChatOptions::new()
            .with_max_tokens(1000)
            .with_temperature(0.7);
        assert_eq!(options.max_tokens, Some(1000));
        assert_eq!(options.temperature, Some(0.7));
    }

    #[test]
    fn test_chat_options_all_fields() {
        let options = ChatOptions::new()
            .with_max_tokens(2000)
            .with_temperature(0.5)
            .with_top_p(0.9)
            .with_stop(vec!["END".to_string(), "STOP".to_string()]);

        assert_eq!(options.max_tokens, Some(2000));
        assert_eq!(options.temperature, Some(0.5));
        assert_eq!(options.top_p, Some(0.9));
        assert!(options.stop.is_some());
        let stop = options.stop.unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0], "END");
    }

    #[test]
    fn test_chat_options_default() {
        let options = ChatOptions::default();
        assert!(options.max_tokens.is_none());
        assert!(options.temperature.is_none());
        assert!(options.top_p.is_none());
        assert!(options.stop.is_none());
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        assert_eq!(tool.name, "search");
    }

    #[test]
    fn test_tool_definition_new() {
        let tool = ToolDefinition::new(
            "web_search",
            "Search the web for information",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        );

        assert_eq!(tool.name, "web_search");
        assert_eq!(tool.description, "Search the web for information");
        assert!(tool.parameters.is_object());
    }

    #[test]
    fn test_llm_tool_call_new() {
        let call = LLMToolCall::new("call_123", "web_search", r#"{"query": "rust"}"#);
        assert_eq!(call.id, "call_123");
        assert_eq!(call.name, "web_search");
        assert_eq!(call.arguments, r#"{"query": "rust"}"#);
    }

    #[test]
    fn test_llm_tool_call_parse_arguments() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct SearchArgs {
            query: String,
        }

        let call = LLMToolCall::new("call_1", "search", r#"{"query": "rust programming"}"#);
        let args: SearchArgs = call.parse_arguments().unwrap();
        assert_eq!(args.query, "rust programming");
    }

    #[test]
    fn test_usage_new() {
        let usage = Usage::new(100, 50);
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_llm_response_serialization() {
        let response = LLMResponse::text("Hello");
        let json = serde_json::to_string(&response).unwrap();
        let parsed: LLMResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.content, "Hello");
        assert!(!parsed.has_tool_calls());
    }

    #[test]
    fn test_tool_definition_serialization() {
        let tool = ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({"type": "object"}),
        );

        let json = serde_json::to_string(&tool).unwrap();
        let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, "search");
        assert_eq!(parsed.description, "Search the web");
    }

    #[tokio::test]
    async fn test_stream_event_done_carries_content() {
        let event = StreamEvent::Done {
            content: "hello".to_string(),
            usage: Some(Usage::new(10, 5)),
        };
        match event {
            StreamEvent::Done { content, usage } => {
                assert_eq!(content, "hello");
                assert!(usage.is_some());
            }
            _ => panic!("Expected Done event"),
        }
    }

    #[tokio::test]
    async fn test_stream_event_delta() {
        let event = StreamEvent::Delta("chunk".to_string());
        match event {
            StreamEvent::Delta(text) => assert_eq!(text, "chunk"),
            _ => panic!("Expected Delta event"),
        }
    }

    #[tokio::test]
    async fn test_stream_event_tool_calls() {
        let tc = LLMToolCall::new("call_1", "search", r#"{"q":"rust"}"#);
        let event = StreamEvent::ToolCalls(vec![tc]);
        match event {
            StreamEvent::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search");
            }
            _ => panic!("Expected ToolCalls event"),
        }
    }

    #[tokio::test]
    async fn test_stream_event_error() {
        let event = StreamEvent::Error(ZeptoError::Provider("fail".into()));
        assert!(matches!(event, StreamEvent::Error(_)));
    }

    #[tokio::test]
    async fn test_chat_stream_default_impl() {
        struct FakeProvider;

        #[async_trait]
        impl LLMProvider for FakeProvider {
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> Result<LLMResponse> {
                Ok(LLMResponse::text("hello from fake"))
            }
            fn default_model(&self) -> &str {
                "fake"
            }
            fn name(&self) -> &str {
                "fake"
            }
        }

        let provider = FakeProvider;
        let mut rx = provider
            .chat_stream(vec![], vec![], None, ChatOptions::default())
            .await
            .unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            StreamEvent::Done { content, .. } => {
                assert_eq!(content, "hello from fake");
            }
            _ => panic!("Expected Done event from default chat_stream"),
        }
    }

    // ====================================================================
    // embed() tests
    // ====================================================================

    /// Compile-time check: if this test compiles, embed() exists on the trait.
    #[tokio::test]
    async fn test_embed_method_exists_on_trait() {
        struct MinimalProvider;

        #[async_trait]
        impl LLMProvider for MinimalProvider {
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> Result<LLMResponse> {
                Ok(LLMResponse::text("ok"))
            }
            fn default_model(&self) -> &str {
                "minimal"
            }
            fn name(&self) -> &str {
                "minimal"
            }
            // Note: embed() is NOT overridden here — default impl is used.
        }

        // The test succeeds if it compiles: embed() is on the trait.
        let provider = MinimalProvider;
        let result = provider.embed(&["hello".to_string()]).await;
        // Default impl returns an error.
        assert!(result.is_err());
    }

    /// Default impl of embed() returns a Provider error.
    #[tokio::test]
    async fn test_embed_default_returns_error() {
        struct DefaultProvider;

        #[async_trait]
        impl LLMProvider for DefaultProvider {
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> Result<LLMResponse> {
                Ok(LLMResponse::text("ok"))
            }
            fn default_model(&self) -> &str {
                "default"
            }
            fn name(&self) -> &str {
                "default"
            }
        }

        let provider = DefaultProvider;
        let result = provider.embed(&["text".to_string()]).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Embedding not supported by this provider"),
            "Expected 'Embedding not supported' error, got: {}",
            err
        );
    }

    /// Default impl of embed() with an empty input also returns an error.
    #[tokio::test]
    async fn test_embed_default_empty_input_returns_error() {
        struct DefaultProvider;

        #[async_trait]
        impl LLMProvider for DefaultProvider {
            async fn chat(
                &self,
                _messages: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _model: Option<&str>,
                _options: ChatOptions,
            ) -> Result<LLMResponse> {
                Ok(LLMResponse::text("ok"))
            }
            fn default_model(&self) -> &str {
                "default"
            }
            fn name(&self) -> &str {
                "default"
            }
        }

        let provider = DefaultProvider;
        let result = provider.embed(&[]).await;
        assert!(result.is_err());
    }
}
