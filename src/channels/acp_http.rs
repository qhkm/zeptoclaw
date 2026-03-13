//! ACP (Agent Client Protocol) streamable HTTP channel.
//!
//! Listens on a TCP port and accepts `POST /acp` requests carrying JSON-RPC 2.0
//! messages. For `session/prompt` requests the connection is kept open and the
//! agent reply is streamed back as Server-Sent Events:
//!
//! ```text
//! POST /acp  →  HTTP/1.1 200 OK
//!               Content-Type: text/event-stream
//!
//!               data: {"jsonrpc":"2.0","method":"session/update","params":{...}}\n\n
//!               data: {"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}\n\n
//! ```
//!
//! `initialize`, `session/new`, and `session/cancel` return synchronous JSON
//! responses on the same connection.
//!
//! The channel registers under the name `"acp_http"` so that its sessions are
//! independent from the stdio `"acp"` channel and the bus routes outbound
//! messages to the correct transport.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, error, info};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::{AcpChannelConfig, AcpHttpConfig};
use crate::error::{Result, ZeptoError};

use super::acp_protocol::{
    AgentCapabilities, AgentInfo, ContentBlock, InitializeResult, JsonRpcRequest,
    PromptContentBlock, SessionNewResult, SessionPromptResult, SessionUpdateParams,
    SessionUpdatePayload,
};
use super::{BaseChannelConfig, Channel};

/// Channel name used for bus routing. Must differ from the stdio channel ("acp").
pub const ACP_HTTP_CHANNEL_NAME: &str = "acp_http";
const ACP_HTTP_SENDER_ID: &str = "acp_client";

/// Maximum size of a complete HTTP request (headers + body).
const MAX_REQUEST_BYTES: usize = 118_784; // 8 KB headers + ~110 KB body
/// Maximum prompt content after text extraction.
const MAX_PROMPT_BYTES: usize = 102_400;
/// Maximum concurrent ACP sessions.
const MAX_ACP_SESSIONS: usize = 1_000;
/// How long (seconds) to wait for the agent to reply to session/prompt.
const PROMPT_TIMEOUT_SECS: u64 = 300;

// --- Static HTTP response fragments ---

const HTTP_204_CORS: &str = "HTTP/1.1 204 No Content\r\n\
    Access-Control-Allow-Origin: *\r\n\
    Access-Control-Allow-Methods: POST, OPTIONS\r\n\
    Access-Control-Allow-Headers: Content-Type, Authorization\r\n\
    Content-Length: 0\r\n\r\n";

const HTTP_400_PREFIX: &str = "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\
     Access-Control-Allow-Origin: *\r\nConnection: close\r\n";

const HTTP_401: &str = "HTTP/1.1 401 Unauthorized\r\n\
    Content-Type: application/json\r\n\
    Access-Control-Allow-Origin: *\r\n\
    Content-Length: 26\r\nConnection: close\r\n\r\n{\"error\":\"unauthorized\"}";

const HTTP_404: &str = "HTTP/1.1 404 Not Found\r\n\
    Content-Type: application/json\r\n\
    Access-Control-Allow-Origin: *\r\n\
    Content-Length: 23\r\nConnection: close\r\n\r\n{\"error\":\"not found\"}";

const HTTP_405: &str = "HTTP/1.1 405 Method Not Allowed\r\n\
    Content-Type: application/json\r\n\
    Access-Control-Allow-Origin: *\r\n\
    Content-Length: 32\r\nConnection: close\r\n\r\n{\"error\":\"method not allowed\"}";

const HTTP_413: &str = "HTTP/1.1 413 Payload Too Large\r\n\
    Content-Type: application/json\r\n\
    Access-Control-Allow-Origin: *\r\n\
    Content-Length: 31\r\nConnection: close\r\n\r\n{\"error\":\"payload too large\"}";

// --- Internal state ---

/// Per-session pending prompt: cancellation state set by session/cancel.
///
/// The original JSON-RPC request id is retained in the connection handler's
/// stack frame and passed directly to `stream_prompt_response`, so it does
/// not need to be stored here.
struct PendingPrompt {
    cancelled: bool,
}

/// Shared map from session ID to the oneshot sender that delivers the agent
/// reply to the waiting HTTP connection handler.
type PromptMap = Arc<Mutex<HashMap<String, oneshot::Sender<(String, bool)>>>>;

/// Mutable per-channel ACP state shared between the accept loop and `send()`.
struct AcpHttpState {
    initialized: bool,
    sessions: HashSet<String>,
    /// Tracks in-flight session/prompt requests so `send()` can retrieve the
    /// original request id and cancelled flag when the agent replies.
    pending: HashMap<String, PendingPrompt>,
}

impl AcpHttpState {
    fn new() -> Self {
        Self {
            initialized: false,
            sessions: HashSet::new(),
            pending: HashMap::new(),
        }
    }
}

/// Parsed representation of an inbound HTTP request.
struct ParsedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

// --- Channel struct ---

/// ACP streamable HTTP channel.
///
/// Registers as `"acp_http"` in the channel manager. When `send()` is called
/// with an `OutboundMessage` for this channel, it delivers the agent reply to
/// the waiting HTTP connection handler via an in-process oneshot channel,
/// which then writes the SSE events and closes the connection.
pub struct AcpHttpChannel {
    config: AcpChannelConfig,
    http_config: AcpHttpConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<AcpHttpState>>,
    /// Session ID → sender half of the oneshot that bridges `send()` to the
    /// HTTP connection handler waiting for the agent reply.
    pending_http: PromptMap,
}

impl AcpHttpChannel {
    pub fn new(
        config: AcpChannelConfig,
        http_config: AcpHttpConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
    ) -> Self {
        Self {
            config,
            http_config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(AcpHttpState::new())),
            pending_http: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // -------------------------------------------------------------------------
    // HTTP parsing helpers
    // -------------------------------------------------------------------------

    fn find_header_end(data: &[u8]) -> Option<usize> {
        data.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn parse_request(raw: &[u8]) -> Option<ParsedRequest> {
        let s = std::str::from_utf8(raw).ok()?;
        let (header_section, body) = match s.find("\r\n\r\n") {
            Some(pos) => (&s[..pos], s[pos + 4..].to_string()),
            None => (s, String::new()),
        };
        let mut lines = header_section.lines();
        let request_line = lines.next()?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next()?.to_uppercase();
        let path = parts.next()?.to_string();
        let mut headers = Vec::new();
        for line in lines {
            if let Some(colon) = line.find(':') {
                headers.push((
                    line[..colon].trim().to_string(),
                    line[colon + 1..].trim().to_string(),
                ));
            }
        }
        Some(ParsedRequest {
            method,
            path,
            headers,
            body,
        })
    }

    fn content_length(headers: &[(String, String)]) -> usize {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
            .unwrap_or(0)
    }

    fn validate_auth(headers: &[(String, String)], token: &Option<String>) -> bool {
        let required = match token {
            Some(t) => t,
            None => return true,
        };
        let expected = format!("Bearer {}", required);
        headers.iter().any(|(n, v)| {
            n.eq_ignore_ascii_case("authorization") && constant_time_eq(v.trim(), &expected)
        })
    }

    // -------------------------------------------------------------------------
    // JSON-RPC / HTTP response builders
    // -------------------------------------------------------------------------

    fn json_rpc_result(id: Option<serde_json::Value>, result: serde_json::Value) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }))
        .unwrap_or_else(|_| {
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialize error"}}"#.to_string()
        })
    }

    fn json_rpc_error(id: Option<serde_json::Value>, code: i64, message: &str) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }))
        .unwrap_or_else(|_| {
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialize error"}}"#.to_string()
        })
    }

    /// Wrap a JSON-RPC body in an HTTP 200 response with CORS headers.
    fn http_200(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Access-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    /// Wrap a JSON-RPC body in an HTTP 400 response with CORS headers.
    fn http_400(body: &str) -> String {
        format!(
            "{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            HTTP_400_PREFIX,
            body.len(),
            body
        )
    }

    /// Format a single SSE data event: `data: <payload>\n\n`.
    fn sse_event(data: &str) -> String {
        format!("data: {}\n\n", data)
    }

    // -------------------------------------------------------------------------
    // Protocol handler helpers (synchronous methods)
    // -------------------------------------------------------------------------

    async fn do_initialize(
        state: &Arc<Mutex<AcpHttpState>>,
        config: &AcpChannelConfig,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> String {
        if let Some(init_params) = params
            .and_then(|p| serde_json::from_value::<super::acp_protocol::InitializeParams>(p).ok())
        {
            if let Some(ref ci) = init_params.client_info {
                info!(
                    client_name = ?ci.name,
                    client_version = ?ci.version,
                    "ACP-HTTP: client initialized"
                );
            }
        }
        {
            let mut st = state.lock().await;
            st.initialized = true;
        }
        let result = InitializeResult {
            protocol_version: serde_json::json!(config.protocol_version),
            agent_capabilities: AgentCapabilities {
                load_session: Some(false),
                prompt_capabilities: Some(serde_json::json!({
                    "image": false, "audio": false, "embeddedContext": false
                })),
                mcp_capabilities: Some(serde_json::json!({ "http": false, "sse": false })),
                session_capabilities: Some(HashMap::new()),
            },
            agent_info: Some(AgentInfo {
                name: Some("zeptoclaw".to_string()),
                title: Some("ZeptoClaw".to_string()),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            auth_methods: vec![],
        };
        match serde_json::to_value(result) {
            Ok(v) => Self::json_rpc_result(id, v),
            Err(e) => Self::json_rpc_error(id, -32603, &format!("serialize error: {}", e)),
        }
    }

    async fn do_session_new(
        state: &Arc<Mutex<AcpHttpState>>,
        base_config: &BaseChannelConfig,
        id: Option<serde_json::Value>,
    ) -> String {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            return Self::json_rpc_error(id, -32000, "Unauthorized");
        }
        let session_id = format!("acph_{}", uuid::Uuid::new_v4().simple());
        {
            let mut st = state.lock().await;
            if !st.initialized {
                return Self::json_rpc_error(
                    id,
                    -32600,
                    "initialize must be called before session/new",
                );
            }
            if st.sessions.len() >= MAX_ACP_SESSIONS {
                return Self::json_rpc_error(
                    id,
                    -32000,
                    &format!("too many sessions (limit: {})", MAX_ACP_SESSIONS),
                );
            }
            st.sessions.insert(session_id.clone());
        }
        let result = SessionNewResult { session_id };
        match serde_json::to_value(result) {
            Ok(v) => Self::json_rpc_result(id, v),
            Err(e) => Self::json_rpc_error(id, -32603, &format!("serialize error: {}", e)),
        }
    }

    async fn do_session_cancel(
        state: &Arc<Mutex<AcpHttpState>>,
        base_config: &BaseChannelConfig,
        params: Option<serde_json::Value>,
    ) -> String {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            // Notifications have no id; still return an empty 200 body.
            return "{}".to_string();
        }
        if let Some(p) = params.and_then(|p| {
            serde_json::from_value::<super::acp_protocol::SessionCancelParams>(p).ok()
        }) {
            let mut st = state.lock().await;
            if let Some(pending) = st.pending.get_mut(&p.session_id) {
                pending.cancelled = true;
                debug!(session_id = %p.session_id, "ACP-HTTP: marked prompt as cancelled");
            }
        }
        "{}".to_string()
    }

    // -------------------------------------------------------------------------
    // session/prompt: validation + bus publish, returns oneshot receiver
    // -------------------------------------------------------------------------

    /// Validate a session/prompt request and register it for async delivery.
    ///
    /// On success returns `Ok((session_id, rx))`.
    /// On validation failure returns `Ok(Err(error_body_string))`.
    /// On internal (bus) failure returns `Err(...)`.
    async fn register_prompt(
        state: &Arc<Mutex<AcpHttpState>>,
        pending_http: &PromptMap,
        base_config: &BaseChannelConfig,
        bus: &Arc<MessageBus>,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> Result<std::result::Result<(String, oneshot::Receiver<(String, bool)>), String>> {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            return Ok(Err(Self::json_rpc_error(id, -32000, "Unauthorized")));
        }
        {
            let st = state.lock().await;
            if !st.initialized {
                return Ok(Err(Self::json_rpc_error(
                    id,
                    -32600,
                    "initialize must be called before session/prompt",
                )));
            }
        }
        let params: super::acp_protocol::SessionPromptParams =
            match params.and_then(|p| serde_json::from_value(p).ok()) {
                Some(p) => p,
                None => {
                    return Ok(Err(Self::json_rpc_error(
                        id,
                        -32602,
                        "session/prompt: missing or invalid params",
                    )));
                }
            };
        let session_id = params.session_id.clone();
        let content = prompt_blocks_to_text(&params.prompt);
        if content.is_empty() {
            return Ok(Err(Self::json_rpc_error(
                id,
                -32602,
                "session/prompt: prompt content is empty",
            )));
        }
        if content.len() > MAX_PROMPT_BYTES {
            return Ok(Err(Self::json_rpc_error(
                id,
                -32602,
                &format!(
                    "session/prompt: content too large ({} bytes, limit {})",
                    content.len(),
                    MAX_PROMPT_BYTES
                ),
            )));
        }
        {
            let mut st = state.lock().await;
            if !st.sessions.contains(&session_id) {
                return Ok(Err(Self::json_rpc_error(
                    id,
                    -32602,
                    &format!("ACP: unknown session {}", session_id),
                )));
            }
            if st.pending.contains_key(&session_id) {
                return Ok(Err(Self::json_rpc_error(
                    id,
                    -32602,
                    "session/prompt: a prompt is already in flight for this session",
                )));
            }
            st.pending
                .insert(session_id.clone(), PendingPrompt { cancelled: false });
        }
        let (tx, rx) = oneshot::channel::<(String, bool)>();
        {
            pending_http.lock().await.insert(session_id.clone(), tx);
        }
        let inbound = InboundMessage::new(
            ACP_HTTP_CHANNEL_NAME,
            ACP_HTTP_SENDER_ID,
            &session_id,
            &content,
        );
        if let Err(e) = bus.publish_inbound(inbound).await {
            // Roll back state so the session can accept a future prompt.
            state.lock().await.pending.remove(&session_id);
            pending_http.lock().await.remove(&session_id);
            return Err(ZeptoError::Channel(format!(
                "ACP-HTTP: failed to publish inbound: {}",
                e
            )));
        }
        debug!(session_id = %session_id, "ACP-HTTP: published session/prompt to bus");
        Ok(Ok((session_id, rx)))
    }

    // -------------------------------------------------------------------------
    // SSE streaming for session/prompt
    // -------------------------------------------------------------------------

    async fn stream_prompt_response(
        stream: &mut tokio::net::TcpStream,
        session_id: &str,
        id: Option<serde_json::Value>,
        rx: oneshot::Receiver<(String, bool)>,
        state: &Arc<Mutex<AcpHttpState>>,
        pending_http: &PromptMap,
    ) {
        // Keep connection alive; client reads SSE events as they arrive.
        let sse_headers = "HTTP/1.1 200 OK\r\n\
            Content-Type: text/event-stream\r\n\
            Cache-Control: no-cache\r\n\
            Connection: keep-alive\r\n\
            Access-Control-Allow-Origin: *\r\n\
            X-Accel-Buffering: no\r\n\r\n";
        if stream.write_all(sse_headers.as_bytes()).await.is_err() {
            // Client disconnected before we could start.
            state.lock().await.pending.remove(session_id);
            pending_http.lock().await.remove(session_id);
            return;
        }
        let _ = stream.flush().await;

        // Wait for the agent reply.
        let (content, cancelled) =
            match tokio::time::timeout(Duration::from_secs(PROMPT_TIMEOUT_SECS), rx).await {
                Ok(Ok(payload)) => payload,
                Ok(Err(_)) => {
                    // Sender was dropped (process shutting down).
                    let ev =
                        Self::sse_event(&Self::json_rpc_error(id, -32603, "agent session closed"));
                    let _ = stream.write_all(ev.as_bytes()).await;
                    let _ = stream.flush().await;
                    return;
                }
                Err(_) => {
                    // Timeout — clean up pending state.
                    state.lock().await.pending.remove(session_id);
                    pending_http.lock().await.remove(session_id);
                    let ev = Self::sse_event(&Self::json_rpc_error(
                        id,
                        -32603,
                        "session/prompt timed out",
                    ));
                    let _ = stream.write_all(ev.as_bytes()).await;
                    let _ = stream.flush().await;
                    return;
                }
            };

        // Emit session/update notification.
        let update = SessionUpdateParams {
            session_id: session_id.to_string(),
            update: SessionUpdatePayload {
                session_update: "agent_message_chunk".to_string(),
                content: Some(ContentBlock::text(&content)),
                tool_call_id: None,
                title: None,
                kind: None,
                status: None,
            },
        };
        if let Ok(update_json) = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": serde_json::to_value(&update).unwrap_or(serde_json::Value::Null)
        })) {
            let ev = Self::sse_event(&update_json);
            if stream.write_all(ev.as_bytes()).await.is_err() {
                return;
            }
        }

        // Emit session/prompt JSON-RPC response.
        let stop_reason = if cancelled { "cancelled" } else { "end_turn" };
        let prompt_result = SessionPromptResult {
            stop_reason: stop_reason.to_string(),
        };
        if let Ok(result_val) = serde_json::to_value(&prompt_result) {
            let body = Self::json_rpc_result(id, result_val);
            let ev = Self::sse_event(&body);
            let _ = stream.write_all(ev.as_bytes()).await;
        }
        let _ = stream.flush().await;
    }

    // -------------------------------------------------------------------------
    // TCP connection handler
    // -------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn handle_connection(
        mut stream: tokio::net::TcpStream,
        config: AcpChannelConfig,
        http_config: AcpHttpConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
        state: Arc<Mutex<AcpHttpState>>,
        pending_http: PromptMap,
    ) {
        // Read the full request (headers + body) with a per-request size cap.
        let mut buf = vec![0u8; MAX_REQUEST_BYTES];
        let mut total = 0usize;
        loop {
            if total >= buf.len() {
                let _ = stream.write_all(HTTP_413.as_bytes()).await;
                return;
            }
            match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf[total..]))
                .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    total += n;
                    if let Some(hend) = Self::find_header_end(&buf[..total]) {
                        if let Some(req) = Self::parse_request(&buf[..total]) {
                            let body_received = total - hend - 4;
                            if body_received >= Self::content_length(&req.headers) {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                Ok(Err(e)) => {
                    debug!("ACP-HTTP: read error: {}", e);
                    return;
                }
                Err(_) => {
                    debug!("ACP-HTTP: read timeout");
                    return;
                }
            }
        }
        if total == 0 {
            return;
        }

        let req = match Self::parse_request(&buf[..total]) {
            Some(r) => r,
            None => {
                let _ = stream.write_all(HTTP_400_PREFIX.as_bytes()).await;
                return;
            }
        };

        // CORS preflight.
        if req.method == "OPTIONS" {
            let _ = stream.write_all(HTTP_204_CORS.as_bytes()).await;
            return;
        }

        // Only POST /acp or POST / is accepted.
        if req.path != "/acp" && req.path != "/" {
            let _ = stream.write_all(HTTP_404.as_bytes()).await;
            return;
        }
        if req.method != "POST" {
            let _ = stream.write_all(HTTP_405.as_bytes()).await;
            return;
        }

        // Bearer token auth.
        if !Self::validate_auth(&req.headers, &http_config.auth_token) {
            let _ = stream.write_all(HTTP_401.as_bytes()).await;
            return;
        }

        // Parse JSON-RPC envelope.
        let rpc: JsonRpcRequest = match serde_json::from_str(&req.body) {
            Ok(r) => r,
            Err(e) => {
                let body = Self::json_rpc_error(None, -32700, &format!("parse error: {}", e));
                let resp = Self::http_400(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
                return;
            }
        };
        if rpc.jsonrpc != "2.0" {
            let body =
                Self::json_rpc_error(rpc.id, -32600, "Invalid Request: jsonrpc must be \"2.0\"");
            let resp = Self::http_200(&body);
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        let id = rpc.id.clone();
        let params = rpc.params.clone();

        match rpc.method.as_str() {
            "initialize" => {
                let body = Self::do_initialize(&state, &config, id, params).await;
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
            "session/new" => {
                let body = Self::do_session_new(&state, &base_config, id).await;
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
            "session/cancel" => {
                let body = Self::do_session_cancel(&state, &base_config, params).await;
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
            "session/prompt" => {
                match Self::register_prompt(
                    &state,
                    &pending_http,
                    &base_config,
                    &bus,
                    id.clone(),
                    params,
                )
                .await
                {
                    Err(e) => {
                        error!("ACP-HTTP: session/prompt internal error: {}", e);
                        let body =
                            Self::json_rpc_error(id, -32603, &format!("internal error: {}", e));
                        let resp = Self::http_200(&body);
                        let _ = stream.write_all(resp.as_bytes()).await;
                    }
                    Ok(Err(err_body)) => {
                        // Validation failure — plain JSON response, no SSE.
                        let resp = Self::http_200(&err_body);
                        let _ = stream.write_all(resp.as_bytes()).await;
                    }
                    Ok(Ok((session_id, rx))) => {
                        // Validation passed — stream SSE response.
                        Self::stream_prompt_response(
                            &mut stream,
                            &session_id,
                            id,
                            rx,
                            &state,
                            &pending_http,
                        )
                        .await;
                    }
                }
            }
            _ => {
                let body =
                    Self::json_rpc_error(id, -32601, &format!("method not found: {}", rpc.method));
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        }
    }

    // -------------------------------------------------------------------------
    // TCP accept loop
    // -------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn run_accept_loop(
        listener: TcpListener,
        config: AcpChannelConfig,
        http_config: AcpHttpConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
        state: Arc<Mutex<AcpHttpState>>,
        pending_http: PromptMap,
        running: Arc<AtomicBool>,
    ) {
        info!(
            "ACP-HTTP: listening on {}:{}",
            http_config.bind, http_config.port
        );
        while running.load(Ordering::SeqCst) {
            match tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
                Ok(Ok((stream, addr))) => {
                    debug!("ACP-HTTP: accepted connection from {}", addr);
                    let config = config.clone();
                    let http_config = http_config.clone();
                    let base_config = base_config.clone();
                    let bus = Arc::clone(&bus);
                    let state = Arc::clone(&state);
                    let pending_http = Arc::clone(&pending_http);
                    tokio::spawn(async move {
                        Self::handle_connection(
                            stream,
                            config,
                            http_config,
                            base_config,
                            bus,
                            state,
                            pending_http,
                        )
                        .await;
                    });
                }
                Ok(Err(e)) => {
                    error!("ACP-HTTP: accept error: {}", e);
                }
                Err(_) => {
                    // accept timeout — loop back and recheck `running`
                }
            }
        }
        running.store(false, Ordering::SeqCst);
        info!("ACP-HTTP: accept loop exited");
    }
}

// -------------------------------------------------------------------------
// Channel trait implementation
// -------------------------------------------------------------------------

#[async_trait]
impl Channel for AcpHttpChannel {
    fn name(&self) -> &str {
        ACP_HTTP_CHANNEL_NAME
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("ACP-HTTP channel already running");
            return Ok(());
        }
        let addr = format!("{}:{}", self.http_config.bind, self.http_config.port);
        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            ZeptoError::Channel(format!("ACP-HTTP: failed to bind {}: {}", addr, e))
        })?;

        let config = self.config.clone();
        let http_config = self.http_config.clone();
        let base_config = self.base_config.clone();
        let bus = Arc::clone(&self.bus);
        let state = Arc::clone(&self.state);
        let pending_http = Arc::clone(&self.pending_http);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            Self::run_accept_loop(
                listener,
                config,
                http_config,
                base_config,
                bus,
                state,
                pending_http,
                running,
            )
            .await;
        });
        info!(
            "ACP-HTTP channel started on {}:{}",
            self.http_config.bind, self.http_config.port
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Called by the bus dispatcher when the agent produces a reply for a
    /// session that originated from this channel.
    ///
    /// Looks up the waiting HTTP connection handler via `pending_http` and
    /// delivers the content + cancellation flag through the oneshot channel.
    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if msg.channel != ACP_HTTP_CHANNEL_NAME {
            return Ok(());
        }
        let session_id = msg.chat_id.clone();

        // Check that the session is known and consume the pending prompt record.
        let (session_exists, cancelled) = {
            let mut st = self.state.lock().await;
            let exists = st.sessions.contains(&session_id);
            let cancelled = st
                .pending
                .remove(&session_id)
                .map(|p| p.cancelled)
                .unwrap_or(false);
            (exists, cancelled)
        };

        if !session_exists {
            debug!(
                session_id = %session_id,
                "ACP-HTTP: outbound for unknown session, skipping"
            );
            return Ok(());
        }

        // Hand the content off to the waiting HTTP handler.
        let sender = self.pending_http.lock().await.remove(&session_id);
        if let Some(tx) = sender {
            // If the receiver was dropped (client disconnected) this is a no-op.
            let _ = tx.send((msg.content, cancelled));
        } else {
            // Proactive message for a session that has no in-flight prompt.
            // Nothing to do — there is no persistent connection to write to.
            debug!(
                session_id = %session_id,
                "ACP-HTTP: proactive message with no waiting handler"
            );
        }
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
    }
}

// -------------------------------------------------------------------------
// Free helpers
// -------------------------------------------------------------------------

/// Extract plain text from ACP prompt content blocks (text blocks only).
fn prompt_blocks_to_text(blocks: &[PromptContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| {
            if let PromptContentBlock::Text { text } = b {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Constant-time string comparison (prevents timing side-channels on auth tokens).
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;
    use crate::config::{AcpChannelConfig, AcpHttpConfig};

    fn make_channel() -> AcpHttpChannel {
        let bus = Arc::new(MessageBus::new());
        AcpHttpChannel::new(
            AcpChannelConfig::default(),
            AcpHttpConfig::default(),
            BaseChannelConfig::new(ACP_HTTP_CHANNEL_NAME),
            bus,
        )
    }

    #[test]
    fn test_channel_name() {
        let ch = make_channel();
        assert_eq!(ch.name(), ACP_HTTP_CHANNEL_NAME);
    }

    #[test]
    fn test_is_not_running_initially() {
        assert!(!make_channel().is_running());
    }

    #[test]
    fn test_prompt_blocks_to_text_text_only() {
        use super::super::acp_protocol::PromptContentBlock;
        let blocks = vec![
            PromptContentBlock::Text {
                text: "Hello".to_string(),
            },
            PromptContentBlock::Text {
                text: "World".to_string(),
            },
        ];
        assert_eq!(prompt_blocks_to_text(&blocks), "Hello\nWorld");
    }

    #[test]
    fn test_prompt_blocks_to_text_skips_non_text() {
        use super::super::acp_protocol::PromptContentBlock;
        let blocks = vec![
            PromptContentBlock::Text {
                text: "only this".to_string(),
            },
            PromptContentBlock::Other,
        ];
        assert_eq!(prompt_blocks_to_text(&blocks), "only this");
    }

    #[tokio::test]
    async fn test_send_ignores_wrong_channel() {
        let ch = make_channel();
        let session_id = "acph_test".to_string();
        {
            let mut st = ch.state.lock().await;
            st.sessions.insert(session_id.clone());
            st.pending
                .insert(session_id.clone(), PendingPrompt { cancelled: false });
        }
        let msg = OutboundMessage {
            channel: "telegram".to_string(),
            chat_id: session_id.clone(),
            content: "hello".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        assert!(ch.send(msg).await.is_ok());
        // pending entry must be untouched
        let st = ch.state.lock().await;
        assert!(
            st.pending.contains_key(&session_id),
            "wrong-channel send must not consume the pending entry"
        );
    }

    #[tokio::test]
    async fn test_send_skips_unknown_session() {
        let ch = make_channel();
        let msg = OutboundMessage {
            channel: ACP_HTTP_CHANNEL_NAME.to_string(),
            chat_id: "acph_ghost".to_string(),
            content: "hello".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        assert!(ch.send(msg).await.is_ok());
        assert!(ch.state.lock().await.sessions.is_empty());
    }

    #[tokio::test]
    async fn test_send_delivers_via_oneshot() {
        let ch = make_channel();
        let session_id = "acph_deliver".to_string();
        let (tx, rx) = oneshot::channel::<(String, bool)>();
        {
            let mut st = ch.state.lock().await;
            st.sessions.insert(session_id.clone());
            st.pending
                .insert(session_id.clone(), PendingPrompt { cancelled: false });
            ch.pending_http.lock().await.insert(session_id.clone(), tx);
        }
        let msg = OutboundMessage {
            channel: ACP_HTTP_CHANNEL_NAME.to_string(),
            chat_id: session_id.clone(),
            content: "agent reply".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        assert!(ch.send(msg).await.is_ok());
        let (content, cancelled) = rx.await.expect("must receive payload");
        assert_eq!(content, "agent reply");
        assert!(!cancelled);
        // pending entry must be consumed
        assert!(!ch.state.lock().await.pending.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_send_marks_cancelled() {
        let ch = make_channel();
        let session_id = "acph_cancel".to_string();
        let (tx, rx) = oneshot::channel::<(String, bool)>();
        {
            let mut st = ch.state.lock().await;
            st.sessions.insert(session_id.clone());
            st.pending
                .insert(session_id.clone(), PendingPrompt { cancelled: true });
            ch.pending_http.lock().await.insert(session_id.clone(), tx);
        }
        let msg = OutboundMessage {
            channel: ACP_HTTP_CHANNEL_NAME.to_string(),
            chat_id: session_id.clone(),
            content: "reply after cancel".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        assert!(ch.send(msg).await.is_ok());
        let (_content, cancelled) = rx.await.expect("must receive payload");
        assert!(
            cancelled,
            "cancelled flag must be forwarded to HTTP handler"
        );
    }

    #[tokio::test]
    async fn test_deny_by_default_blocks_session_new() {
        let bus = Arc::new(MessageBus::new());
        let base = BaseChannelConfig {
            name: ACP_HTTP_CHANNEL_NAME.to_string(),
            allowlist: vec![],
            deny_by_default: true,
        };
        let ch = AcpHttpChannel::new(
            AcpChannelConfig {
                deny_by_default: true,
                ..AcpChannelConfig::default()
            },
            AcpHttpConfig::default(),
            base,
            bus,
        );
        ch.state.lock().await.initialized = true;
        let result =
            AcpHttpChannel::do_session_new(&ch.state, &ch.base_config, Some(serde_json::json!(1)))
                .await;
        assert!(
            result.contains("Unauthorized"),
            "deny_by_default must block session/new"
        );
        assert!(ch.state.lock().await.sessions.is_empty());
    }

    #[test]
    fn test_constant_time_eq_same() {
        assert!(constant_time_eq("hello", "hello"));
    }

    #[test]
    fn test_constant_time_eq_different() {
        assert!(!constant_time_eq("hello", "world"));
        assert!(!constant_time_eq("hello", "hello!"));
    }

    #[test]
    fn test_sse_event_format() {
        let ev = AcpHttpChannel::sse_event(r#"{"foo":"bar"}"#);
        assert_eq!(ev, "data: {\"foo\":\"bar\"}\n\n");
    }

    #[test]
    fn test_http_200_content_length() {
        let body = r#"{"result":"ok"}"#;
        let resp = AcpHttpChannel::http_200(body);
        assert!(resp.contains(&format!("Content-Length: {}", body.len())));
        assert!(resp.ends_with(body));
    }
}
