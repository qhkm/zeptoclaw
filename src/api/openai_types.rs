//! OpenAI-compatible request/response types for the `/v1/chat/completions` API.
//!
//! These types allow any OpenAI SDK to target ZeptoClaw as a drop-in backend.
//! Only the subset needed for chat completions is implemented; tool-calling,
//! function-calling, and logprobs are intentionally omitted.

use serde::{Deserialize, Serialize};

use crate::providers::{LLMResponse, StreamEvent, Usage as ZeptoUsage};
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
}

/// A single chat message in OpenAI format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role: "system", "user", or "assistant".
    pub role: String,
    /// Text content.
    pub content: String,
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
    /// Reason the model stopped: "stop" or "length".
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
    /// `None` while streaming, "stop" on final chunk.
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
                other => Err(format!("unsupported message role: {other}")),
            }?;
            Ok(Message {
                role,
                content: m.content.clone(),
                content_parts: vec![crate::session::ContentPart::Text {
                    text: m.content.clone(),
                }],
                tool_calls: None,
                tool_call_id: None,
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

    ChatCompletionResponse {
        id: completion_id(),
        object: "chat.completion",
        created: now,
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: llm.content.clone(),
            },
            finish_reason: "stop".to_string(),
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
            },
            finish_reason: None,
        }],
    }
}

/// Build the final stop chunk (no content, finish_reason = "stop").
pub fn done_chunk(model: &str, id: &str, created: u64) -> ChatCompletionChunk {
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
            },
            finish_reason: Some("stop".to_string()),
        }],
    }
}

/// Map a `StreamEvent` to the corresponding SSE chunk (if any).
///
/// Returns `None` for events that have no chunk representation (e.g.,
/// `StreamEvent::ToolCalls` which is irrelevant to the OpenAI completions API).
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
        StreamEvent::ToolCalls(_) => {
            // Tool calls are not exposed through the OpenAI completions API.
            None
        }
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
            ChatMessage {
                role: "system".into(),
                content: "You are helpful.".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Hello".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "Hi!".into(),
            },
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
        let openai_msgs = vec![ChatMessage {
            role: "function".into(),
            content: "result".into(),
        }];
        let result = messages_from_openai(&openai_msgs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("function"));
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
        assert_eq!(resp.choices[0].message.content, "Hello, world!");
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

    // -----------------------------------------------------------------------
    // Streaming chunks
    // -----------------------------------------------------------------------

    #[test]
    fn test_first_chunk_has_role() {
        let c = first_chunk("m", "id-1", 1000);
        assert_eq!(c.object, "chat.completion.chunk");
        assert_eq!(c.choices[0].delta.role.as_deref(), Some("assistant"));
        assert!(c.choices[0].delta.content.is_none());
        assert!(c.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_delta_chunk_has_content() {
        let c = delta_chunk("hello", "m", "id-1", 1000);
        assert!(c.choices[0].delta.role.is_none());
        assert_eq!(c.choices[0].delta.content.as_deref(), Some("hello"));
        assert!(c.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_done_chunk_has_stop_reason() {
        let c = done_chunk("m", "id-1", 1000);
        assert!(c.choices[0].delta.role.is_none());
        assert!(c.choices[0].delta.content.is_none());
        assert_eq!(c.choices[0].finish_reason.as_deref(), Some("stop"));
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
    fn test_chunk_from_tool_calls_event_is_none() {
        let event = StreamEvent::ToolCalls(vec![]);
        let chunk = chunk_from_stream_event(&event, "m", "id", 1);
        assert!(chunk.is_none());
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
    fn test_chat_completion_chunk_serializes() {
        let c = delta_chunk("token", "m", "id", 42);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"object\":\"chat.completion.chunk\""));
        assert!(json.contains("\"content\":\"token\""));
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
    }

    #[test]
    fn test_chat_completion_request_minimal() {
        let json = r#"{"model": "m", "messages": []}"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert!(req.stream.is_none());
        assert!(req.max_tokens.is_none());
        assert!(req.temperature.is_none());
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
}
