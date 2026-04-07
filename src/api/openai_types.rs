//! OpenAI-compatible request/response types for the `/v1/chat/completions` API.
//!
//! These types allow any OpenAI SDK to target ZeptoClaw as a drop-in backend.
//! Supports chat completions with tool calling in both streaming and
//! non-streaming modes.

use serde::{Deserialize, Serialize};

use crate::providers::{LLMResponse, LLMToolCall, StreamEvent, Usage as ZeptoUsage};
use crate::session::{Message, Role};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// OpenAI-compatible chat completion request body.
#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    /// Model to use (e.g., "gpt-4o", "claude-sonnet-4-5-20250929").
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Whether to stream the response via SSE.
    #[serde(default)]
    pub stream: Option<bool>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 - 2.0).
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Tool definitions available for the model to call.
    #[serde(default)]
    pub tools: Option<Vec<ToolParam>>,
    /// Controls which tools the model may call.
    /// Only `null`, omitted, and `"auto"` are currently supported.
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

/// A single chat message in OpenAI format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Text content. `None` for assistant messages that only contain tool calls.
    #[serde(default)]
    pub content: Option<String>,
    /// Tool calls made by the assistant (present when role is "assistant").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallResponse>>,
    /// ID of the tool call this message responds to (present when role is "tool").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// An OpenAI tool definition in the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParam {
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub tool_type: String,
    /// The function definition.
    pub function: FunctionDef,
}

/// Function definition within a tool parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    /// The name of the function.
    pub name: String,
    /// A description of what the function does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the function's parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Non-streaming response types
// ---------------------------------------------------------------------------

/// OpenAI-compatible chat completion response.
#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    /// Unique completion ID.
    pub id: String,
    /// Always "chat.completion".
    pub object: &'static str,
    /// Unix timestamp of creation.
    pub created: u64,
    /// Model that generated the response.
    pub model: String,
    /// Completion choices (always exactly one for ZeptoClaw).
    pub choices: Vec<Choice>,
    /// Token usage statistics.
    pub usage: UsageResponse,
}

/// A single completion choice.
#[derive(Debug, Serialize)]
pub struct Choice {
    /// Choice index (always 0).
    pub index: u32,
    /// The assistant's reply.
    pub message: ChatMessage,
    /// Reason the model stopped: "stop", "length", or "tool_calls".
    pub finish_reason: String,
}

// ---------------------------------------------------------------------------
// Streaming (SSE) response types
// ---------------------------------------------------------------------------

/// A single SSE chunk for streaming completions.
#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    /// Unique completion ID (same across all chunks).
    pub id: String,
    /// Always "chat.completion.chunk".
    pub object: &'static str,
    /// Unix timestamp of creation.
    pub created: u64,
    /// Model name.
    pub model: String,
    /// Chunk choices.
    pub choices: Vec<ChunkChoice>,
}

/// A single choice within a streaming chunk.
#[derive(Debug, Serialize)]
pub struct ChunkChoice {
    /// Choice index (always 0).
    pub index: u32,
    /// Delta content for this chunk.
    pub delta: Delta,
    /// `None` while streaming, "stop" or "tool_calls" on final chunk.
    pub finish_reason: Option<String>,
}

/// Incremental content within a streaming chunk.
#[derive(Debug, Serialize)]
pub struct Delta {
    /// Role (only present in the first chunk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Content fragment (absent in the final stop chunk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls (present when the model invokes tools during streaming).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

/// A tool call within a streaming delta.
#[derive(Debug, Clone, Serialize)]
pub struct DeltaToolCall {
    /// Index of this tool call in the array.
    pub index: u32,
    /// Unique call ID (present in first chunk for this tool call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Always `"function"` (present in first chunk for this tool call).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    /// Function name and arguments.
    pub function: DeltaFunction,
}

/// Function call data within a streaming tool call delta.
#[derive(Debug, Clone, Serialize)]
pub struct DeltaFunction {
    /// Function name (present in first chunk for this tool call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Argument fragment (accumulated across chunks).
    pub arguments: String,
}

/// A tool call in an OpenAI response message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResponse {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub call_type: String,
    /// The function invocation details.
    pub function: FunctionCallResponse,
}

/// Function call data in a tool call response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallResponse {
    /// Name of the function to call.
    pub name: String,
    /// JSON-encoded arguments for the function.
    pub arguments: String,
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Token usage in OpenAI format.
#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ---------------------------------------------------------------------------
// Models listing
// ---------------------------------------------------------------------------

/// Response for `GET /v1/models`.
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: &'static str,
    pub data: Vec<ModelObject>,
}

/// A single model entry.
#[derive(Debug, Serialize)]
pub struct ModelObject {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert OpenAI-format tool parameters into ZeptoClaw `ToolDefinition` values.
pub fn tools_from_openai(tools: &[ToolParam]) -> Vec<crate::providers::ToolDefinition> {
    tools
        .iter()
        .map(|t| crate::providers::ToolDefinition {
            name: t.function.name.clone(),
            description: t.function.description.clone().unwrap_or_default(),
            parameters: t
                .function
                .parameters
                .clone()
                .unwrap_or(serde_json::json!({"type": "object"})),
        })
        .collect()
}

/// Return `true` when the request's `tool_choice` matches current behavior.
pub fn supports_tool_choice(choice: Option<&serde_json::Value>) -> bool {
    match choice {
        None => true,
        Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(mode)) if mode == "auto" => true,
        _ => false,
    }
}

/// Convert ZeptoClaw `LLMToolCall` values into OpenAI tool call response format.
fn tool_calls_from_llm(calls: &[LLMToolCall]) -> Vec<ToolCallResponse> {
    calls
        .iter()
        .map(|tc| ToolCallResponse {
            id: tc.id.clone(),
            call_type: "function".to_string(),
            function: FunctionCallResponse {
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        })
        .collect()
}

/// Convert OpenAI-format messages into ZeptoClaw `Message` values.
///
/// Returns an error if any message has an unrecognized role.
pub fn messages_from_openai(msgs: &[ChatMessage]) -> Result<Vec<Message>, String> {
    msgs.iter()
        .map(|m| {
            let role = match m.role.as_str() {
                "system" => Ok(Role::System),
                "user" => Ok(Role::User),
                "assistant" => Ok(Role::Assistant),
                "tool" => Ok(Role::Tool),
                other => Err(format!("unsupported message role: {other}")),
            }?;
            let content = m.content.clone().unwrap_or_default();
            let tool_calls = m.tool_calls.as_ref().map(|tcs| {
                tcs.iter()
                    .map(|tc| crate::session::ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    })
                    .collect()
            });
            Ok(Message {
                role,
                content: content.clone(),
                content_parts: vec![crate::session::ContentPart::Text { text: content }],
                tool_calls,
                tool_call_id: m.tool_call_id.clone(),
            })
        })
        .collect()
}

/// Build a `ChatCompletionResponse` from an `LLMResponse`.
pub fn response_from_llm(llm: &LLMResponse, model: &str) -> ChatCompletionResponse {
    let now = unix_now();
    let usage = llm
        .usage
        .as_ref()
        .map(usage_from_zepto)
        .unwrap_or(UsageResponse {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });

    let has_tool_calls = !llm.tool_calls.is_empty();
    let tool_calls = if has_tool_calls {
        Some(tool_calls_from_llm(&llm.tool_calls))
    } else {
        None
    };
    let content = if llm.content.is_empty() && has_tool_calls {
        None
    } else {
        Some(llm.content.clone())
    };
    let finish_reason = if has_tool_calls { "tool_calls" } else { "stop" };

    ChatCompletionResponse {
        id: completion_id(),
        object: "chat.completion",
        created: now,
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content,
                tool_calls,
                tool_call_id: None,
            },
            finish_reason: finish_reason.to_string(),
        }],
        usage,
    }
}

/// Build the first SSE chunk (carries the role).
pub fn first_chunk(model: &str, id: &str, created: u64) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created,
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: Delta {
                role: Some("assistant".to_string()),
                content: None,
                tool_calls: None,
            },
            finish_reason: None,
        }],
    }
}

/// Build a content delta chunk.
pub fn delta_chunk(text: &str, model: &str, id: &str, created: u64) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created,
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: Delta {
                role: None,
                content: Some(text.to_string()),
                tool_calls: None,
            },
            finish_reason: None,
        }],
    }
}

/// Build a streaming chunk carrying tool calls.
pub fn tool_calls_chunk(
    calls: &[LLMToolCall],
    model: &str,
    id: &str,
    created: u64,
) -> ChatCompletionChunk {
    let delta_calls: Vec<DeltaToolCall> = calls
        .iter()
        .enumerate()
        .map(|(i, tc)| DeltaToolCall {
            index: i as u32,
            id: Some(tc.id.clone()),
            call_type: Some("function".to_string()),
            function: DeltaFunction {
                name: Some(tc.name.clone()),
                arguments: tc.arguments.clone(),
            },
        })
        .collect();

    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created,
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: Some(delta_calls),
            },
            finish_reason: None,
        }],
    }
}

/// Build the final stop chunk with a custom finish reason.
pub fn done_chunk_with_reason(
    model: &str,
    id: &str,
    created: u64,
    finish_reason: &str,
) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created,
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: None,
            },
            finish_reason: Some(finish_reason.to_string()),
        }],
    }
}

/// Build the final stop chunk (no content, finish_reason = "stop").
pub fn done_chunk(model: &str, id: &str, created: u64) -> ChatCompletionChunk {
    done_chunk_with_reason(model, id, created, "stop")
}

/// Map a `StreamEvent` to the corresponding SSE chunk (if any).
///
/// Returns `None` for events that have no chunk representation (e.g.,
/// empty `ToolCalls` or `Error` events).
pub fn chunk_from_stream_event(
    event: &StreamEvent,
    model: &str,
    id: &str,
    created: u64,
) -> Option<ChatCompletionChunk> {
    match event {
        StreamEvent::Delta(text) => Some(delta_chunk(text, model, id, created)),
        StreamEvent::Done { .. } => Some(done_chunk(model, id, created)),
        StreamEvent::Error(_) => {
            // Errors are handled by the route handler, not serialized as chunks.
            None
        }
        StreamEvent::ToolCalls(calls) if !calls.is_empty() => {
            Some(tool_calls_chunk(calls, model, id, created))
        }
        StreamEvent::ToolCalls(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn usage_from_zepto(u: &ZeptoUsage) -> UsageResponse {
    UsageResponse {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Usage;

    // Helper to create a simple text ChatMessage.
    fn text_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    // -----------------------------------------------------------------------
    // messages_from_openai
    // -----------------------------------------------------------------------

    #[test]
    fn test_messages_from_openai_empty() {
        let msgs = messages_from_openai(&[]).unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_messages_from_openai_maps_roles() {
        let openai_msgs = vec![
            text_msg("system", "You are helpful."),
            text_msg("user", "Hello"),
            text_msg("assistant", "Hi!"),
        ];
        let msgs = messages_from_openai(&openai_msgs).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[0].content, "You are helpful.");
        assert_eq!(msgs[1].role, Role::User);
        assert_eq!(msgs[2].role, Role::Assistant);
    }

    #[test]
    fn test_messages_from_openai_unknown_role_returns_error() {
        let openai_msgs = vec![text_msg("function", "result")];
        let result = messages_from_openai(&openai_msgs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("function"));
    }

    #[test]
    fn test_messages_from_openai_tool_role() {
        let msg = ChatMessage {
            role: "tool".to_string(),
            content: Some("72 degrees".to_string()),
            tool_calls: None,
            tool_call_id: Some("call_abc".to_string()),
        };
        let msgs = messages_from_openai(&[msg]).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Tool);
        assert_eq!(msgs[0].content, "72 degrees");
        assert_eq!(msgs[0].tool_call_id.as_deref(), Some("call_abc"));
    }

    #[test]
    fn test_messages_from_openai_assistant_with_tool_calls() {
        let msg = ChatMessage {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(vec![ToolCallResponse {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCallResponse {
                    name: "get_weather".to_string(),
                    arguments: r#"{"location":"Boston"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        };
        let msgs = messages_from_openai(&[msg]).unwrap();
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[0].content, ""); // None → empty string
        let tcs = msgs[0].tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].name, "get_weather");
        assert_eq!(tcs[0].id, "call_1");
    }

    // -----------------------------------------------------------------------
    // tools_from_openai
    // -----------------------------------------------------------------------

    #[test]
    fn test_tools_from_openai_empty() {
        let tools = tools_from_openai(&[]);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_tools_from_openai_converts() {
        let params = vec![ToolParam {
            tool_type: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get weather for a city".to_string()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"}
                    },
                    "required": ["location"]
                })),
            },
        }];
        let defs = tools_from_openai(&params);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "get_weather");
        assert_eq!(defs[0].description, "Get weather for a city");
        assert!(defs[0].parameters["properties"]["location"].is_object());
    }

    #[test]
    fn test_tools_from_openai_defaults() {
        let params = vec![ToolParam {
            tool_type: "function".to_string(),
            function: FunctionDef {
                name: "ping".to_string(),
                description: None,
                parameters: None,
            },
        }];
        let defs = tools_from_openai(&params);
        assert_eq!(defs[0].description, "");
        assert_eq!(defs[0].parameters, serde_json::json!({"type": "object"}));
    }

    #[test]
    fn test_supports_tool_choice() {
        assert!(supports_tool_choice(None));
        assert!(supports_tool_choice(Some(&serde_json::Value::Null)));
        assert!(supports_tool_choice(Some(&serde_json::Value::String(
            "auto".to_string()
        ))));
        assert!(!supports_tool_choice(Some(&serde_json::Value::String(
            "required".to_string()
        ))));
        assert!(!supports_tool_choice(Some(&serde_json::json!({
            "type": "function",
            "function": {"name": "search"}
        }))));
    }

    // -----------------------------------------------------------------------
    // tool_calls_from_llm
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_calls_from_llm() {
        let calls = vec![
            LLMToolCall::new("call_1", "search", r#"{"q":"rust"}"#),
            LLMToolCall::new("call_2", "read", r#"{"path":"foo"}"#),
        ];
        let resp = tool_calls_from_llm(&calls);
        assert_eq!(resp.len(), 2);
        assert_eq!(resp[0].id, "call_1");
        assert_eq!(resp[0].call_type, "function");
        assert_eq!(resp[0].function.name, "search");
        assert_eq!(resp[0].function.arguments, r#"{"q":"rust"}"#);
        assert_eq!(resp[1].function.name, "read");
    }

    // -----------------------------------------------------------------------
    // response_from_llm
    // -----------------------------------------------------------------------

    #[test]
    fn test_response_from_llm_basic() {
        let llm = LLMResponse::text("Hello, world!");
        let resp = response_from_llm(&llm, "test-model");
        assert_eq!(resp.object, "chat.completion");
        assert_eq!(resp.model, "test-model");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.role, "assistant");
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("Hello, world!")
        );
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason, "stop");
        assert!(resp.id.starts_with("chatcmpl-"));
    }

    #[test]
    fn test_response_from_llm_with_usage() {
        let llm = LLMResponse::text("ok").with_usage(Usage::new(10, 20));
        let resp = response_from_llm(&llm, "m");
        assert_eq!(resp.usage.prompt_tokens, 10);
        assert_eq!(resp.usage.completion_tokens, 20);
        assert_eq!(resp.usage.total_tokens, 30);
    }

    #[test]
    fn test_response_from_llm_without_usage_zeroes() {
        let llm = LLMResponse::text("ok");
        let resp = response_from_llm(&llm, "m");
        assert_eq!(resp.usage.prompt_tokens, 0);
        assert_eq!(resp.usage.total_tokens, 0);
    }

    #[test]
    fn test_response_from_llm_with_tool_calls() {
        let tc = LLMToolCall::new("call_1", "get_weather", r#"{"location":"NYC"}"#);
        let llm = LLMResponse::with_tools("", vec![tc]);
        let resp = response_from_llm(&llm, "m");
        assert_eq!(resp.choices[0].finish_reason, "tool_calls");
        assert!(resp.choices[0].message.content.is_none()); // empty content → None
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_1");
        assert_eq!(tcs[0].call_type, "function");
        assert_eq!(tcs[0].function.name, "get_weather");
    }

    #[test]
    fn test_response_from_llm_with_content_and_tool_calls() {
        let tc = LLMToolCall::new("call_1", "search", r#"{"q":"rust"}"#);
        let llm = LLMResponse::with_tools("Let me search that", vec![tc]);
        let resp = response_from_llm(&llm, "m");
        assert_eq!(resp.choices[0].finish_reason, "tool_calls");
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("Let me search that")
        );
        assert!(resp.choices[0].message.tool_calls.is_some());
    }

    // -----------------------------------------------------------------------
    // Streaming chunks
    // -----------------------------------------------------------------------

    #[test]
    fn test_first_chunk_has_role() {
        let c = first_chunk("m", "id-1", 1000);
        assert_eq!(c.object, "chat.completion.chunk");
        assert_eq!(c.choices[0].delta.role.as_deref(), Some("assistant"));
        assert!(c.choices[0].delta.content.is_none());
        assert!(c.choices[0].delta.tool_calls.is_none());
        assert!(c.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_delta_chunk_has_content() {
        let c = delta_chunk("hello", "m", "id-1", 1000);
        assert!(c.choices[0].delta.role.is_none());
        assert_eq!(c.choices[0].delta.content.as_deref(), Some("hello"));
        assert!(c.choices[0].delta.tool_calls.is_none());
        assert!(c.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_done_chunk_has_stop_reason() {
        let c = done_chunk("m", "id-1", 1000);
        assert!(c.choices[0].delta.role.is_none());
        assert!(c.choices[0].delta.content.is_none());
        assert!(c.choices[0].delta.tool_calls.is_none());
        assert_eq!(c.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_done_chunk_with_custom_reason() {
        let c = done_chunk_with_reason("m", "id-1", 1000, "tool_calls");
        assert_eq!(c.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn test_tool_calls_chunk() {
        let calls = vec![
            LLMToolCall::new("call_1", "search", r#"{"q":"hi"}"#),
            LLMToolCall::new("call_2", "read", r#"{"p":"f"}"#),
        ];
        let c = tool_calls_chunk(&calls, "m", "id-1", 1000);
        let delta = &c.choices[0].delta;
        assert!(delta.role.is_none());
        assert!(delta.content.is_none());
        let tcs = delta.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0].index, 0);
        assert_eq!(tcs[0].id.as_deref(), Some("call_1"));
        assert_eq!(tcs[0].call_type.as_deref(), Some("function"));
        assert_eq!(tcs[0].function.name.as_deref(), Some("search"));
        assert_eq!(tcs[0].function.arguments, r#"{"q":"hi"}"#);
        assert_eq!(tcs[1].index, 1);
        assert!(c.choices[0].finish_reason.is_none());
    }

    // -----------------------------------------------------------------------
    // chunk_from_stream_event
    // -----------------------------------------------------------------------

    #[test]
    fn test_chunk_from_delta_event() {
        let event = StreamEvent::Delta("hi".into());
        let chunk = chunk_from_stream_event(&event, "m", "id", 1);
        assert!(chunk.is_some());
        let c = chunk.unwrap();
        assert_eq!(c.choices[0].delta.content.as_deref(), Some("hi"));
    }

    #[test]
    fn test_chunk_from_done_event() {
        let event = StreamEvent::Done {
            content: "full".into(),
            usage: None,
        };
        let chunk = chunk_from_stream_event(&event, "m", "id", 1);
        assert!(chunk.is_some());
        let c = chunk.unwrap();
        assert_eq!(c.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_chunk_from_error_event_is_none() {
        let event = StreamEvent::Error(crate::error::ZeptoError::Provider("fail".into()));
        let chunk = chunk_from_stream_event(&event, "m", "id", 1);
        assert!(chunk.is_none());
    }

    #[test]
    fn test_chunk_from_empty_tool_calls_is_none() {
        let event = StreamEvent::ToolCalls(vec![]);
        let chunk = chunk_from_stream_event(&event, "m", "id", 1);
        assert!(chunk.is_none());
    }

    #[test]
    fn test_chunk_from_tool_calls_event() {
        let tc = LLMToolCall::new("call_1", "search", r#"{"q":"rust"}"#);
        let event = StreamEvent::ToolCalls(vec![tc]);
        let chunk = chunk_from_stream_event(&event, "m", "id", 1);
        assert!(chunk.is_some());
        let c = chunk.unwrap();
        let tcs = c.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name.as_deref(), Some("search"));
    }

    // -----------------------------------------------------------------------
    // Serialization round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn test_chat_completion_response_serializes() {
        let llm = LLMResponse::text("ok");
        let resp = response_from_llm(&llm, "m");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"object\":\"chat.completion\""));
        assert!(json.contains("\"finish_reason\":\"stop\""));
    }

    #[test]
    fn test_chat_completion_response_with_tools_serializes() {
        let tc = LLMToolCall::new("call_1", "search", r#"{"q":"hi"}"#);
        let llm = LLMResponse::with_tools("", vec![tc]);
        let resp = response_from_llm(&llm, "m");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"finish_reason\":\"tool_calls\""));
        assert!(json.contains("\"type\":\"function\""));
        assert!(json.contains("\"name\":\"search\""));
    }

    #[test]
    fn test_chat_completion_chunk_serializes() {
        let c = delta_chunk("token", "m", "id", 42);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"object\":\"chat.completion.chunk\""));
        assert!(json.contains("\"content\":\"token\""));
    }

    #[test]
    fn test_tool_calls_chunk_serializes() {
        let calls = vec![LLMToolCall::new("call_1", "fn", r#"{}"#)];
        let c = tool_calls_chunk(&calls, "m", "id", 1);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"tool_calls\""));
        assert!(json.contains("\"type\":\"function\""));
        assert!(json.contains("\"index\":0"));
    }

    #[test]
    fn test_models_response_serializes() {
        let resp = ModelsResponse {
            object: "list",
            data: vec![ModelObject {
                id: "gpt-4o".into(),
                object: "model",
                created: 1000,
                owned_by: "zeptoclaw".into(),
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"object\":\"list\""));
        assert!(json.contains("\"id\":\"gpt-4o\""));
    }

    #[test]
    fn test_chat_completion_request_deserializes() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
            "max_tokens": 100,
            "temperature": 0.7
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.max_tokens, Some(100));
        assert!((req.temperature.unwrap() - 0.7).abs() < f32::EPSILON);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
    }

    #[test]
    fn test_chat_completion_request_with_tools_deserializes() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "weather?"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get the weather",
                    "parameters": {
                        "type": "object",
                        "properties": {"location": {"type": "string"}},
                        "required": ["location"]
                    }
                }
            }],
            "tool_choice": "auto"
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_type, "function");
        assert_eq!(tools[0].function.name, "get_weather");
        assert!(tools[0].function.parameters.is_some());
        assert_eq!(req.tool_choice.unwrap(), "auto");
    }

    #[test]
    fn test_chat_completion_request_with_tool_messages() {
        let json = r#"{
            "model": "m",
            "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"location\":\"NYC\"}"}
                }]},
                {"role": "tool", "content": "72F sunny", "tool_call_id": "call_1"}
            ]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 3);
        // Assistant message with tool calls
        assert!(req.messages[1].content.is_none());
        let tcs = req.messages[1].tool_calls.as_ref().unwrap();
        assert_eq!(tcs[0].function.name, "get_weather");
        // Tool result message
        assert_eq!(req.messages[2].role, "tool");
        assert_eq!(req.messages[2].content.as_deref(), Some("72F sunny"));
        assert_eq!(req.messages[2].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn test_chat_completion_request_minimal() {
        let json = r#"{"model": "m", "messages": []}"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert!(req.stream.is_none());
        assert!(req.max_tokens.is_none());
        assert!(req.temperature.is_none());
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
    }

    // -----------------------------------------------------------------------
    // Helper functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_completion_id_format() {
        let id = completion_id();
        assert!(id.starts_with("chatcmpl-"));
        // UUID v4 after the prefix
        assert!(id.len() > "chatcmpl-".len());
    }

    #[test]
    fn test_unix_now_is_reasonable() {
        let now = unix_now();
        // Should be after 2024-01-01
        assert!(now > 1_704_067_200);
    }

    #[test]
    fn test_usage_from_zepto() {
        let zu = crate::providers::Usage::new(5, 10);
        let u = usage_from_zepto(&zu);
        assert_eq!(u.prompt_tokens, 5);
        assert_eq!(u.completion_tokens, 10);
        assert_eq!(u.total_tokens, 15);
    }

    // -----------------------------------------------------------------------
    // ToolParam / ToolCallResponse serialization
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_param_round_trip() {
        let tp = ToolParam {
            tool_type: "function".to_string(),
            function: FunctionDef {
                name: "search".to_string(),
                description: Some("Search things".to_string()),
                parameters: Some(serde_json::json!({"type": "object"})),
            },
        };
        let json = serde_json::to_string(&tp).unwrap();
        assert!(json.contains("\"type\":\"function\""));
        let parsed: ToolParam = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.function.name, "search");
    }

    #[test]
    fn test_tool_call_response_round_trip() {
        let tcr = ToolCallResponse {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCallResponse {
                name: "get_weather".to_string(),
                arguments: r#"{"location":"NYC"}"#.to_string(),
            },
        };
        let json = serde_json::to_string(&tcr).unwrap();
        assert!(json.contains("\"type\":\"function\""));
        let parsed: ToolCallResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "call_1");
        assert_eq!(parsed.function.name, "get_weather");
    }
}
