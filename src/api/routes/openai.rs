//! OpenAI-compatible API route handlers.
//!
//! `POST /v1/chat/completions` — chat completion (streaming + non-streaming).
//! `GET  /v1/models`           — list available models.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::unfold;
use tracing::error;

use super::super::openai_types::{self, ChatCompletionRequest, ModelObject, ModelsResponse};
use super::super::server::AppState;

// ---------------------------------------------------------------------------
// POST /v1/chat/completions
// ---------------------------------------------------------------------------

/// Handle `POST /v1/chat/completions`.
///
/// Supports both streaming (`stream: true`) and non-streaming modes.
/// When `tools` are provided in the request, they are forwarded to the LLM
/// provider and any resulting tool calls are returned in OpenAI format.
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Response {
    let provider = match &state.provider {
        Some(p) => Arc::clone(p),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error_body("No LLM provider configured")),
            )
                .into_response();
        }
    };

    let messages = match openai_types::messages_from_openai(&req.messages) {
        Ok(msgs) => msgs,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(error_body(&e))).into_response();
        }
    };

    if !openai_types::supports_tool_choice(req.tool_choice.as_ref()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_body(
                "unsupported tool_choice: only omitted, null, or \"auto\" are accepted",
            )),
        )
            .into_response();
    }

    let tools = req
        .tools
        .as_deref()
        .map(openai_types::tools_from_openai)
        .unwrap_or_default();

    let mut options = crate::providers::ChatOptions::new();
    if let Some(max) = req.max_tokens {
        options = options.with_max_tokens(max);
    }
    if let Some(temp) = req.temperature {
        options = options.with_temperature(temp);
    }

    let model_str = req.model.clone();

    if req.stream == Some(true) {
        stream_response(provider, messages, tools, options, model_str).await
    } else {
        non_stream_response(provider, messages, tools, options, model_str).await
    }
}

/// Non-streaming completion: call `provider.chat()` and return JSON.
async fn non_stream_response(
    provider: Arc<dyn crate::providers::LLMProvider>,
    messages: Vec<crate::session::Message>,
    tools: Vec<crate::providers::ToolDefinition>,
    options: crate::providers::ChatOptions,
    model: String,
) -> Response {
    match provider.chat(messages, tools, Some(&model), options).await {
        Ok(llm_resp) => {
            let resp = openai_types::response_from_llm(&llm_resp, &model);
            Json(resp).into_response()
        }
        Err(e) => {
            error!(error = %e, "Non-streaming chat completion failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_body("Internal server error")),
            )
                .into_response()
        }
    }
}

/// Streaming completion: call `provider.chat_stream()` and emit SSE.
async fn stream_response(
    provider: Arc<dyn crate::providers::LLMProvider>,
    messages: Vec<crate::session::Message>,
    tools: Vec<crate::providers::ToolDefinition>,
    options: crate::providers::ChatOptions,
    model: String,
) -> Response {
    let rx = match provider
        .chat_stream(messages, tools, Some(&model), options)
        .await
    {
        Ok(rx) => rx,
        Err(e) => {
            error!(error = %e, "Failed to start streaming chat completion");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_body("Internal server error")),
            )
                .into_response();
        }
    };

    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // State: (receiver, sent_first, done, sent_done_sentinel, pending_tool_stop).
    //
    // `pending_tool_stop`: when `true`, the next poll emits a stop chunk with
    // `finish_reason: "tool_calls"` before the `[DONE]` sentinel.
    let initial_state = (rx, false, false, false, false);

    let stream = unfold(
        initial_state,
        move |(mut rx, sent_first, done, sent_done_sentinel, pending_tool_stop)| {
            let model = model.clone();
            let id = id.clone();
            async move {
                // Already sent [DONE] — terminate the stream.
                if sent_done_sentinel {
                    return None;
                }

                // Emit the [DONE] sentinel after the stop chunk has been sent.
                if done {
                    let sse = Event::default().data("[DONE]");
                    return Some((Ok::<_, Infallible>(sse), (rx, true, true, true, false)));
                }

                // Emit a stop chunk with "tool_calls" finish reason after tool
                // calls have been streamed.
                if pending_tool_stop {
                    let chunk =
                        openai_types::done_chunk_with_reason(&model, &id, created, "tool_calls");
                    let data = serde_json::to_string(&chunk).unwrap_or_default();
                    let sse = Event::default().data(data);
                    return Some((Ok(sse), (rx, true, true, false, false)));
                }

                // Emit the role-only first chunk before any content.
                if !sent_first {
                    let first = openai_types::first_chunk(&model, &id, created);
                    let data = serde_json::to_string(&first).unwrap_or_default();
                    let event = Event::default().data(data);
                    return Some((Ok::<_, Infallible>(event), (rx, true, false, false, false)));
                }

                // Await the next stream event from the provider.
                match rx.recv().await {
                    Some(event) => {
                        // Check if this is a Done event so we can send [DONE] after.
                        let is_done = matches!(event, crate::providers::StreamEvent::Done { .. });

                        // Handle tool calls specially — need a follow-up stop chunk.
                        let is_tool_calls = matches!(
                            &event,
                            crate::providers::StreamEvent::ToolCalls(c) if !c.is_empty()
                        );

                        if let Some(chunk) =
                            openai_types::chunk_from_stream_event(&event, &model, &id, created)
                        {
                            let data = serde_json::to_string(&chunk).unwrap_or_default();
                            let sse = Event::default().data(data);
                            if is_tool_calls {
                                // Tool calls chunk emitted; next poll emits stop("tool_calls").
                                Some((Ok(sse), (rx, true, false, false, true)))
                            } else if is_done {
                                // The stop chunk was emitted; next poll emits [DONE].
                                Some((Ok(sse), (rx, true, true, false, false)))
                            } else {
                                Some((Ok(sse), (rx, true, false, false, false)))
                            }
                        } else {
                            // Skip events that don't produce chunks (e.g., empty ToolCalls).
                            let sse = Event::default().comment("skip");
                            Some((Ok(sse), (rx, true, false, false, false)))
                        }
                    }
                    None => {
                        // Channel closed — emit [DONE] sentinel and stop.
                        let sse = Event::default().data("[DONE]");
                        Some((Ok(sse), (rx, true, true, true, false)))
                    }
                }
            }
        },
    );

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/models
// ---------------------------------------------------------------------------

/// Handle `GET /v1/models`.
///
/// Returns a list of model objects derived from configured providers.
pub async fn list_models(State(state): State<Arc<AppState>>) -> Json<ModelsResponse> {
    let mut models = Vec::new();

    if let Some(ref config) = state.config {
        let provider_names = crate::providers::configured_provider_names(config);
        for name in provider_names {
            let model_id = match crate::providers::provider_config_by_name(config, name) {
                Some(pc) => pc
                    .model
                    .clone()
                    .unwrap_or_else(|| config.agents.defaults.model.clone()),
                None => config.agents.defaults.model.clone(),
            };

            models.push(ModelObject {
                id: model_id,
                object: "model",
                created: 0,
                owned_by: format!("zeptoclaw/{name}"),
            });
        }
    }

    // If no provider-specific models, expose the default model.
    if models.is_empty() {
        if let Some(ref provider) = state.provider {
            models.push(ModelObject {
                id: provider.default_model().to_string(),
                object: "model",
                created: 0,
                owned_by: format!("zeptoclaw/{}", provider.name()),
            });
        }
    }

    Json(ModelsResponse {
        object: "list",
        data: models,
    })
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

fn error_body(message: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "message": message,
            "type": "server_error",
            "code": serde_json::Value::Null,
        }
    })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;
    use crate::api::server::AppState;
    use crate::error::Result;
    use crate::providers::{
        ChatOptions, LLMResponse, LLMToolCall, StreamEvent, ToolDefinition, Usage,
    };
    use crate::session::Message;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    // -----------------------------------------------------------------------
    // Mock provider
    // -----------------------------------------------------------------------

    #[derive(Debug)]
    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl crate::providers::LLMProvider for MockProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Ok(LLMResponse::text(&self.response).with_usage(Usage::new(5, 10)))
        }

        async fn chat_stream(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let resp = self.response.clone();
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::Delta(resp.clone())).await;
                let _ = tx
                    .send(StreamEvent::Done {
                        content: resp,
                        usage: Some(Usage::new(5, 10)),
                    })
                    .await;
            });
            Ok(rx)
        }

        fn default_model(&self) -> &str {
            "mock-model"
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    /// Mock provider that returns tool calls instead of text.
    #[derive(Debug)]
    struct MockToolCallProvider;

    #[async_trait]
    impl crate::providers::LLMProvider for MockToolCallProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            let tc = LLMToolCall::new("call_abc", "get_weather", r#"{"location":"NYC"}"#);
            Ok(LLMResponse::with_tools("", vec![tc]).with_usage(Usage::new(8, 12)))
        }

        fn default_model(&self) -> &str {
            "mock-tool-model"
        }

        fn name(&self) -> &str {
            "mock-tool"
        }
    }

    fn make_state_with_provider() -> Arc<AppState> {
        let bus = EventBus::new(8);
        let mut state = AppState::new("tok".into(), bus);
        state.provider = Some(Arc::new(MockProvider {
            response: "Hello from mock".into(),
        }));
        Arc::new(state)
    }

    fn make_state_with_tool_provider() -> Arc<AppState> {
        let bus = EventBus::new(8);
        let mut state = AppState::new("tok".into(), bus);
        state.provider = Some(Arc::new(MockToolCallProvider));
        Arc::new(state)
    }

    fn make_state_no_provider() -> Arc<AppState> {
        let bus = EventBus::new(8);
        Arc::new(AppState::new("tok".into(), bus))
    }

    fn make_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/chat/completions", post(chat_completions))
            .route("/v1/models", get(list_models))
            .with_state(state)
    }

    // -----------------------------------------------------------------------
    // Non-streaming tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_chat_completions_non_streaming() {
        let app = make_app(make_state_with_provider());
        let body = r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["choices"][0]["message"]["content"], "Hello from mock");
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
        assert_eq!(json["usage"]["prompt_tokens"], 5);
        assert_eq!(json["usage"]["completion_tokens"], 10);
    }

    #[tokio::test]
    async fn test_chat_completions_no_provider_returns_503() {
        let app = make_app(make_state_no_provider());
        let body = r#"{"model":"m","messages":[]}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_chat_completions_with_tools_non_streaming() {
        let app = make_app(make_state_with_tool_provider());
        let body = r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "weather in NYC?"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": {"type": "object", "properties": {"location": {"type": "string"}}}
                }
            }]
        }"#;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["choices"][0]["finish_reason"], "tool_calls");
        let tool_calls = &json["choices"][0]["message"]["tool_calls"];
        assert_eq!(tool_calls[0]["id"], "call_abc");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    }

    #[tokio::test]
    async fn test_chat_completions_rejects_unsupported_tool_choice() {
        let app = make_app(make_state_with_tool_provider());
        let body = r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "weather in NYC?"}],
            "tool_choice": "required"
        }"#;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let message = json["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("unsupported tool_choice"));
    }

    #[tokio::test]
    async fn test_chat_completions_tool_result_messages() {
        let app = make_app(make_state_with_provider());
        let body = r#"{
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
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -----------------------------------------------------------------------
    // Streaming tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_chat_completions_streaming_returns_200() {
        let app = make_app(make_state_with_provider());
        let body = r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Content-Type should be text/event-stream for SSE.
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("text/event-stream"), "Expected SSE, got: {ct}");

        // Consume the SSE body and verify chunk ordering + [DONE] sentinel.
        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);

        // Extract all `data:` lines from the SSE body.
        let data_lines: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("data:"))
            .map(|l| l.trim_start_matches("data:").trim())
            .collect();

        // Must have at least: first chunk (role), content chunk(s), stop chunk, [DONE].
        assert!(
            data_lines.len() >= 3,
            "Expected at least 3 data events, got {}: {:?}",
            data_lines.len(),
            data_lines,
        );

        // The last data line must be [DONE].
        assert_eq!(
            data_lines.last().copied(),
            Some("[DONE]"),
            "Last SSE data event must be [DONE], got: {:?}",
            data_lines.last(),
        );

        // All data lines before [DONE] must be valid JSON chunks.
        for line in &data_lines[..data_lines.len() - 1] {
            let json: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|_| panic!("Invalid JSON chunk: {line}"));
            assert_eq!(json["object"], "chat.completion.chunk");
        }
    }

    #[tokio::test]
    async fn test_chat_completions_streaming_tool_calls() {
        let app = make_app(make_state_with_tool_provider());
        let body = r#"{
            "model": "m",
            "messages": [{"role": "user", "content": "weather?"}],
            "tools": [{"type": "function", "function": {"name": "get_weather"}}],
            "stream": true
        }"#;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);

        let data_lines: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("data:"))
            .map(|l| l.trim_start_matches("data:").trim())
            .collect();

        // Should end with [DONE].
        assert_eq!(data_lines.last().copied(), Some("[DONE]"));

        // Find the chunk with tool_calls in the delta.
        let mut found_tool_calls = false;
        let mut found_tool_calls_finish = false;
        for line in &data_lines[..data_lines.len() - 1] {
            let json: serde_json::Value = serde_json::from_str(line).unwrap();
            if json["choices"][0]["delta"]["tool_calls"].is_array() {
                found_tool_calls = true;
                let tcs = &json["choices"][0]["delta"]["tool_calls"];
                assert_eq!(tcs[0]["function"]["name"], "get_weather");
                assert_eq!(tcs[0]["id"], "call_abc");
            }
            if json["choices"][0]["finish_reason"] == "tool_calls" {
                found_tool_calls_finish = true;
            }
        }
        assert!(found_tool_calls, "Expected a chunk with tool_calls delta");
        assert!(
            found_tool_calls_finish,
            "Expected a chunk with finish_reason 'tool_calls'"
        );
    }

    // -----------------------------------------------------------------------
    // Models endpoint
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_models_with_provider() {
        let app = make_app(make_state_with_provider());
        let req = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["object"], "list");
        // Should have at least the default model from the mock provider.
        assert!(!json["data"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_models_no_provider() {
        let app = make_app(make_state_no_provider());
        let req = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["object"], "list");
        assert!(json["data"].as_array().unwrap().is_empty());
    }
}
