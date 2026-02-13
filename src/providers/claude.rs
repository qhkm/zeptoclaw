//! Claude (Anthropic) LLM provider implementation
//!
//! This module implements the `LLMProvider` trait for Claude/Anthropic's API,
//! handling message conversion, tool calls, and response parsing.
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::providers::{claude::ClaudeProvider, ChatOptions, LLMProvider};
//! use zeptoclaw::session::Message;
//!
//! async fn example() {
//!     let provider = ClaudeProvider::new("your-api-key");
//!
//!     let messages = vec![
//!         Message::system("You are a helpful assistant."),
//!         Message::user("Hello!"),
//!     ];
//!
//!     let response = provider
//!         .chat(messages, vec![], None, ChatOptions::default())
//!         .await
//!         .unwrap();
//!
//!     println!("Claude: {}", response.content);
//! }
//! ```

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Result, ZeptoError};
use crate::session::{Message, Role, ToolCall};

use super::{ChatOptions, LLMProvider, LLMResponse, LLMToolCall, ToolDefinition, Usage};

/// The Claude API endpoint URL.
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";

/// The default Claude model to use.
const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";

/// The Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Claude/Anthropic LLM provider.
///
/// Implements the `LLMProvider` trait for Anthropic's Claude API.
/// Handles message format conversion, tool calling, and response parsing.
pub struct ClaudeProvider {
    /// API key for authentication
    api_key: String,
    /// HTTP client for making requests
    client: Client,
}

impl ClaudeProvider {
    /// Create a new Claude provider with the given API key.
    ///
    /// # Arguments
    /// * `api_key` - Anthropic API key
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::providers::claude::ClaudeProvider;
    /// use zeptoclaw::providers::LLMProvider;
    ///
    /// let provider = ClaudeProvider::new("sk-ant-api03-xxx");
    /// assert_eq!(provider.name(), "claude");
    /// ```
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Create a new Claude provider with a custom HTTP client.
    ///
    /// This is useful for testing or when you need custom client configuration
    /// (e.g., custom timeouts, proxies).
    ///
    /// # Arguments
    /// * `api_key` - Anthropic API key
    /// * `client` - Custom reqwest client
    pub fn with_client(api_key: &str, client: Client) -> Self {
        Self {
            api_key: api_key.to_string(),
            client,
        }
    }
}

#[async_trait]
impl LLMProvider for ClaudeProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        let model = model.unwrap_or(DEFAULT_MODEL);

        // Convert messages to Claude format, extracting system message
        let (system, claude_messages) = convert_messages(messages)?;

        // Build request
        let request = ClaudeRequest {
            model: model.to_string(),
            max_tokens: options.max_tokens.unwrap_or(8192),
            messages: claude_messages,
            system,
            tools: if tools.is_empty() {
                None
            } else {
                Some(convert_tools(tools))
            },
            temperature: options.temperature,
            top_p: options.top_p,
            stop_sequences: options.stop,
        };

        // Send request
        let response = self
            .client
            .post(CLAUDE_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse as Claude error response
            if let Ok(error_response) = serde_json::from_str::<ClaudeErrorResponse>(&error_text) {
                return Err(ZeptoError::Provider(format!(
                    "Claude API error ({}): {} - {}",
                    status, error_response.error.r#type, error_response.error.message
                )));
            }

            return Err(ZeptoError::Provider(format!(
                "Claude API error ({}): {}",
                status, error_text
            )));
        }

        let claude_response: ClaudeResponse = response.json().await?;
        Ok(convert_response(claude_response))
    }

    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    fn name(&self) -> &str {
        "claude"
    }
}

// ============================================================================
// Claude API Request Types
// ============================================================================

/// Claude API request body.
#[derive(Debug, Serialize)]
struct ClaudeRequest {
    /// Model identifier
    model: String,
    /// Maximum tokens to generate
    max_tokens: u32,
    /// Conversation messages (excluding system)
    messages: Vec<ClaudeMessage>,
    /// System prompt (separate from messages in Claude API)
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    /// Available tools
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ClaudeTool>>,
    /// Temperature for sampling
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Top-p (nucleus) sampling
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    /// Stop sequences
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

/// A message in Claude's format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaudeMessage {
    /// Role: "user" or "assistant"
    role: String,
    /// Message content (string or array of blocks)
    content: ClaudeContent,
}

/// Claude message content - can be simple text or content blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ClaudeContent {
    /// Simple text content
    Text(String),
    /// Array of content blocks (for tool calls/results)
    Blocks(Vec<ClaudeContentBlock>),
}

/// A content block within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ClaudeContentBlock {
    /// Text content
    #[serde(rename = "text")]
    Text { text: String },
    /// Tool use (assistant requesting to call a tool)
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (user providing result of tool execution)
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Claude tool definition.
#[derive(Debug, Serialize)]
struct ClaudeTool {
    /// Tool name
    name: String,
    /// Tool description
    description: String,
    /// JSON Schema for tool parameters
    input_schema: serde_json::Value,
}

// ============================================================================
// Claude API Response Types
// ============================================================================

/// Claude API response body.
#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    /// Response content blocks
    content: Vec<ClaudeContentBlock>,
    /// Token usage
    usage: ClaudeUsage,
    /// Stop reason (e.g., "end_turn", "tool_use")
    #[allow(dead_code)]
    stop_reason: Option<String>,
}

/// Claude API error response.
#[derive(Debug, Deserialize)]
struct ClaudeErrorResponse {
    error: ClaudeError,
}

/// Claude API error details.
#[derive(Debug, Deserialize)]
struct ClaudeError {
    r#type: String,
    message: String,
}

/// Claude token usage.
#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    /// Tokens in the input
    input_tokens: u32,
    /// Tokens in the output
    output_tokens: u32,
}

// ============================================================================
// Conversion Functions
// ============================================================================

/// Convert ZeptoClaw messages to Claude API format.
///
/// Extracts the system message (if present) and converts all other messages
/// to Claude's message format. Handles tool calls and tool results.
///
/// # Arguments
/// * `messages` - ZeptoClaw messages
///
/// # Returns
/// A tuple of (optional system message, Claude messages)
fn convert_messages(messages: Vec<Message>) -> Result<(Option<String>, Vec<ClaudeMessage>)> {
    let mut system: Option<String> = None;
    let mut claude_messages: Vec<ClaudeMessage> = Vec::new();

    // Group consecutive tool results together
    let mut pending_tool_results: Vec<ClaudeContentBlock> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                // Claude uses a separate system field
                system = Some(msg.content);
            }
            Role::User => {
                // Flush any pending tool results first as a user message
                if !pending_tool_results.is_empty() {
                    claude_messages.push(ClaudeMessage {
                        role: "user".to_string(),
                        content: ClaudeContent::Blocks(std::mem::take(&mut pending_tool_results)),
                    });
                }

                // Add user message
                claude_messages.push(ClaudeMessage {
                    role: "user".to_string(),
                    content: ClaudeContent::Text(msg.content),
                });
            }
            Role::Assistant => {
                // Flush any pending tool results first
                if !pending_tool_results.is_empty() {
                    claude_messages.push(ClaudeMessage {
                        role: "user".to_string(),
                        content: ClaudeContent::Blocks(std::mem::take(&mut pending_tool_results)),
                    });
                }

                // Check if this message has tool calls
                if let Some(tool_calls) = msg.tool_calls {
                    let mut blocks: Vec<ClaudeContentBlock> = Vec::new();

                    // Add text content if present
                    if !msg.content.is_empty() {
                        blocks.push(ClaudeContentBlock::Text { text: msg.content });
                    }

                    // Add tool use blocks
                    for tc in tool_calls {
                        // Parse arguments from JSON string to Value
                        let input: serde_json::Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));

                        blocks.push(ClaudeContentBlock::ToolUse {
                            id: tc.id,
                            name: tc.name,
                            input,
                        });
                    }

                    claude_messages.push(ClaudeMessage {
                        role: "assistant".to_string(),
                        content: ClaudeContent::Blocks(blocks),
                    });
                } else {
                    // Simple text message
                    claude_messages.push(ClaudeMessage {
                        role: "assistant".to_string(),
                        content: ClaudeContent::Text(msg.content),
                    });
                }
            }
            Role::Tool => {
                // Tool results are sent as user messages with tool_result blocks
                if let Some(tool_call_id) = msg.tool_call_id {
                    pending_tool_results.push(ClaudeContentBlock::ToolResult {
                        tool_use_id: tool_call_id,
                        content: msg.content,
                        is_error: None,
                    });
                }
            }
        }
    }

    // Flush any remaining tool results
    if !pending_tool_results.is_empty() {
        claude_messages.push(ClaudeMessage {
            role: "user".to_string(),
            content: ClaudeContent::Blocks(pending_tool_results),
        });
    }

    Ok((system, claude_messages))
}

/// Convert ZeptoClaw tool definitions to Claude API format.
fn convert_tools(tools: Vec<ToolDefinition>) -> Vec<ClaudeTool> {
    tools
        .into_iter()
        .map(|t| ClaudeTool {
            name: t.name,
            description: t.description,
            input_schema: t.parameters,
        })
        .collect()
}

/// Convert Claude API response to ZeptoClaw LLMResponse.
fn convert_response(response: ClaudeResponse) -> LLMResponse {
    let mut content = String::new();
    let mut tool_calls: Vec<LLMToolCall> = Vec::new();

    for block in response.content {
        match block {
            ClaudeContentBlock::Text { text } => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&text);
            }
            ClaudeContentBlock::ToolUse { id, name, input } => {
                // Convert input Value back to JSON string
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(LLMToolCall::new(&id, &name, &arguments));
            }
            ClaudeContentBlock::ToolResult { .. } => {
                // Tool results shouldn't appear in responses, but handle gracefully
            }
        }
    }

    let usage = Usage::new(response.usage.input_tokens, response.usage.output_tokens);

    LLMResponse {
        content,
        tool_calls,
        usage: Some(usage),
    }
}

/// Convert ZeptoClaw ToolCall to LLMToolCall.
///
/// This is a helper for converting between the session's ToolCall type
/// and the provider's LLMToolCall type.
#[allow(dead_code)]
fn tool_call_to_llm_tool_call(tc: &ToolCall) -> LLMToolCall {
    LLMToolCall::new(&tc.id, &tc.name, &tc.arguments)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Message;

    #[test]
    fn test_claude_provider_creation() {
        let provider = ClaudeProvider::new("test-key");
        assert_eq!(provider.name(), "claude");
        assert_eq!(provider.default_model(), "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn test_claude_provider_with_client() {
        let client = Client::new();
        let provider = ClaudeProvider::with_client("test-key", client);
        assert_eq!(provider.name(), "claude");
    }

    #[test]
    fn test_message_conversion_simple() {
        let messages = vec![Message::user("Hello"), Message::assistant("Hi there!")];

        let (system, claude_messages) = convert_messages(messages).unwrap();

        assert!(system.is_none());
        assert_eq!(claude_messages.len(), 2);
        assert_eq!(claude_messages[0].role, "user");
        assert_eq!(claude_messages[1].role, "assistant");
    }

    #[test]
    fn test_message_conversion_with_system() {
        let messages = vec![
            Message::system("You are a helpful assistant"),
            Message::user("Hello"),
            Message::assistant("Hi there!"),
        ];

        let (system, claude_messages) = convert_messages(messages).unwrap();

        assert_eq!(system, Some("You are a helpful assistant".to_string()));
        assert_eq!(claude_messages.len(), 2);
        assert_eq!(claude_messages[0].role, "user");
        assert_eq!(claude_messages[1].role, "assistant");
    }

    #[test]
    fn test_message_conversion_with_tool_calls() {
        let tool_call = ToolCall::new("call_1", "web_search", r#"{"query": "rust"}"#);
        let messages = vec![
            Message::user("Search for Rust"),
            Message::assistant_with_tools("Let me search for that.", vec![tool_call]),
            Message::tool_result("call_1", "Found 100 results"),
            Message::assistant("I found 100 results about Rust."),
        ];

        let (system, claude_messages) = convert_messages(messages).unwrap();

        assert!(system.is_none());
        assert_eq!(claude_messages.len(), 4);

        // First: user message
        assert_eq!(claude_messages[0].role, "user");

        // Second: assistant with tool call
        assert_eq!(claude_messages[1].role, "assistant");
        if let ClaudeContent::Blocks(blocks) = &claude_messages[1].content {
            assert_eq!(blocks.len(), 2); // text + tool_use
            assert!(matches!(blocks[0], ClaudeContentBlock::Text { .. }));
            assert!(matches!(blocks[1], ClaudeContentBlock::ToolUse { .. }));
        } else {
            panic!("Expected blocks content for tool call message");
        }

        // Third: tool result (as user message)
        assert_eq!(claude_messages[2].role, "user");
        if let ClaudeContent::Blocks(blocks) = &claude_messages[2].content {
            assert_eq!(blocks.len(), 1);
            assert!(matches!(blocks[0], ClaudeContentBlock::ToolResult { .. }));
        } else {
            panic!("Expected blocks content for tool result");
        }

        // Fourth: assistant response
        assert_eq!(claude_messages[3].role, "assistant");
    }

    #[test]
    fn test_message_conversion_multiple_tool_results() {
        let tc1 = ToolCall::new("call_1", "tool_a", "{}");
        let tc2 = ToolCall::new("call_2", "tool_b", "{}");

        let messages = vec![
            Message::user("Do both"),
            Message::assistant_with_tools("Running both tools.", vec![tc1, tc2]),
            Message::tool_result("call_1", "Result A"),
            Message::tool_result("call_2", "Result B"),
            Message::assistant("Both completed."),
        ];

        let (_, claude_messages) = convert_messages(messages).unwrap();

        assert_eq!(claude_messages.len(), 4);

        // Tool results should be grouped in one user message
        assert_eq!(claude_messages[2].role, "user");
        if let ClaudeContent::Blocks(blocks) = &claude_messages[2].content {
            assert_eq!(blocks.len(), 2); // Both tool results grouped
        } else {
            panic!("Expected grouped tool results");
        }
    }

    #[test]
    fn test_convert_tools() {
        let tools = vec![
            ToolDefinition::new(
                "web_search",
                "Search the web",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
            ),
            ToolDefinition::new(
                "calculator",
                "Perform calculations",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string" }
                    }
                }),
            ),
        ];

        let claude_tools = convert_tools(tools);

        assert_eq!(claude_tools.len(), 2);
        assert_eq!(claude_tools[0].name, "web_search");
        assert_eq!(claude_tools[0].description, "Search the web");
        assert_eq!(claude_tools[1].name, "calculator");
    }

    #[test]
    fn test_convert_response_text_only() {
        let response = ClaudeResponse {
            content: vec![ClaudeContentBlock::Text {
                text: "Hello, world!".to_string(),
            }],
            usage: ClaudeUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: Some("end_turn".to_string()),
        };

        let llm_response = convert_response(response);

        assert_eq!(llm_response.content, "Hello, world!");
        assert!(!llm_response.has_tool_calls());
        assert!(llm_response.usage.is_some());

        let usage = llm_response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_convert_response_with_tool_calls() {
        let response = ClaudeResponse {
            content: vec![
                ClaudeContentBlock::Text {
                    text: "Let me search for that.".to_string(),
                },
                ClaudeContentBlock::ToolUse {
                    id: "toolu_01".to_string(),
                    name: "web_search".to_string(),
                    input: serde_json::json!({"query": "rust programming"}),
                },
            ],
            usage: ClaudeUsage {
                input_tokens: 20,
                output_tokens: 30,
            },
            stop_reason: Some("tool_use".to_string()),
        };

        let llm_response = convert_response(response);

        assert_eq!(llm_response.content, "Let me search for that.");
        assert!(llm_response.has_tool_calls());
        assert_eq!(llm_response.tool_calls.len(), 1);

        let tc = &llm_response.tool_calls[0];
        assert_eq!(tc.id, "toolu_01");
        assert_eq!(tc.name, "web_search");
        assert!(tc.arguments.contains("rust programming"));
    }

    #[test]
    fn test_convert_response_multiple_text_blocks() {
        let response = ClaudeResponse {
            content: vec![
                ClaudeContentBlock::Text {
                    text: "First part.".to_string(),
                },
                ClaudeContentBlock::Text {
                    text: "Second part.".to_string(),
                },
            ],
            usage: ClaudeUsage {
                input_tokens: 10,
                output_tokens: 10,
            },
            stop_reason: Some("end_turn".to_string()),
        };

        let llm_response = convert_response(response);

        assert_eq!(llm_response.content, "First part.\nSecond part.");
    }

    #[test]
    fn test_claude_request_serialization() {
        let request = ClaudeRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 1000,
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: ClaudeContent::Text("Hello".to_string()),
            }],
            system: Some("You are helpful.".to_string()),
            tools: None,
            temperature: Some(0.7),
            top_p: None,
            stop_sequences: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("claude-sonnet-4-5-20250929"));
        assert!(json.contains("max_tokens"));
        assert!(json.contains("Hello"));
        assert!(json.contains("You are helpful"));
        assert!(json.contains("temperature"));
        // top_p should not be present (skip_serializing_if)
        assert!(!json.contains("top_p"));
    }

    #[test]
    fn test_claude_request_without_optional_fields() {
        let request = ClaudeRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 1000,
            messages: vec![],
            system: None,
            tools: None,
            temperature: None,
            top_p: None,
            stop_sequences: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        // Optional fields should not be present
        assert!(!json.contains("system"));
        assert!(!json.contains("tools"));
        assert!(!json.contains("temperature"));
        assert!(!json.contains("top_p"));
        assert!(!json.contains("stop_sequences"));
    }

    #[test]
    fn test_content_block_serialization() {
        // Text block
        let text_block = ClaudeContentBlock::Text {
            text: "Hello".to_string(),
        };
        let json = serde_json::to_string(&text_block).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains(r#""text":"Hello""#));

        // Tool use block
        let tool_use = ClaudeContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "search".to_string(),
            input: serde_json::json!({"q": "test"}),
        };
        let json = serde_json::to_string(&tool_use).unwrap();
        assert!(json.contains(r#""type":"tool_use""#));
        assert!(json.contains(r#""id":"call_1""#));
        assert!(json.contains(r#""name":"search""#));

        // Tool result block
        let tool_result = ClaudeContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "Result".to_string(),
            is_error: None,
        };
        let json = serde_json::to_string(&tool_result).unwrap();
        assert!(json.contains(r#""type":"tool_result""#));
        assert!(json.contains(r#""tool_use_id":"call_1""#));
    }

    #[test]
    fn test_empty_messages() {
        let messages: Vec<Message> = vec![];
        let (system, claude_messages) = convert_messages(messages).unwrap();

        assert!(system.is_none());
        assert!(claude_messages.is_empty());
    }

    #[test]
    fn test_only_system_message() {
        let messages = vec![Message::system("You are helpful.")];
        let (system, claude_messages) = convert_messages(messages).unwrap();

        assert_eq!(system, Some("You are helpful.".to_string()));
        assert!(claude_messages.is_empty());
    }

    #[test]
    fn test_tool_call_to_llm_tool_call() {
        let tc = ToolCall::new("call_123", "web_search", r#"{"query": "test"}"#);
        let llm_tc = tool_call_to_llm_tool_call(&tc);

        assert_eq!(llm_tc.id, "call_123");
        assert_eq!(llm_tc.name, "web_search");
        assert_eq!(llm_tc.arguments, r#"{"query": "test"}"#);
    }
}
