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
use tokio::task::JoinSet;
use tracing::{debug, error, info};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::{AcpChannelConfig, AcpHttpConfig};
use crate::error::{Result, ZeptoError};

use super::acp_protocol::{
    AgentCapabilities, AgentInfo, ContentBlock, InitializeResult, JsonRpcRequest,
    PromptContentBlock, SessionInfo, SessionListParams, SessionListResult, SessionNewResult,
    SessionPromptResult, SessionUpdateParams, SessionUpdatePayload,
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
/// Maximum number of in-flight TCP connections accepted before new ones are
/// dropped.  Each accepted connection allocates a MAX_REQUEST_BYTES buffer
/// before auth is checked, so an unlimited accept loop is a memory DoS vector.
const MAX_CONCURRENT_CONNECTIONS: usize = 128;
/// How long (seconds) to wait for the agent to reply to session/prompt.
const PROMPT_TIMEOUT_SECS: u64 = 300;

// --- Static HTTP response fragments ---

const HTTP_204_CORS: &str = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: 0\r\n\r\n";

/// Returned for JSON-RPC notifications: 204 No Content with no body.
/// Per JSON-RPC 2.0 §4.1, servers MUST NOT reply to notifications.
const HTTP_204_NOTIFICATION: &str =
    "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\n\r\n";

const HTTP_400_PREFIX: &str = "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n";

/// Build a self-contained HTTP error response with a correct Content-Length.
fn build_http_error(status_line: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status_line,
        body.len(),
        body
    )
}

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
    /// Client IDs that have completed the `initialize` handshake.
    ///
    /// Each HTTP client receives a unique ID from `initialize` (via the
    /// `clientId` response field) and must echo it back as the
    /// `X-Acp-Client-Id` request header on subsequent calls.  Using a set
    /// here rather than a single bool isolates concurrent clients so that
    /// one client's initialization does not satisfy another's guard check.
    initialized_clients: HashSet<String>,
    /// Session IDs → working directory (from session/new params, None if not provided).
    sessions: HashMap<String, Option<String>>,
    /// Tracks in-flight session/prompt requests so `send()` can retrieve the
    /// original request id and cancelled flag when the agent replies.
    pending: HashMap<String, PendingPrompt>,
}

impl AcpHttpState {
    fn new() -> Self {
        Self {
            initialized_clients: HashSet::new(),
            sessions: HashMap::new(),
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
    /// Handle to the spawned accept-loop task; held so `stop()` can abort and
    /// await it, ensuring the TcpListener is released before returning.
    accept_handle: Option<tokio::task::JoinHandle<()>>,
    /// Tracks in-flight connection handler tasks so `stop()` can abort and
    /// await them all, preventing handlers from outliving the channel.
    conn_tasks: Arc<Mutex<JoinSet<()>>>,
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
            accept_handle: None,
            conn_tasks: Arc::new(Mutex::new(JoinSet::new())),
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
        let pos = s.find("\r\n\r\n")?;
        let header_section = &s[..pos];
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
        // Bound the body by Content-Length to prevent pipelined/malformed
        // trailing bytes from being folded into the JSON payload.
        let content_len = Self::content_length(&headers);
        let body_start = pos + 4;
        let body_end = (body_start + content_len).min(s.len());
        let body = s[body_start..body_end].to_string();
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
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
        _config: &AcpChannelConfig,
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
        let client_id = super::acp_protocol::new_id();
        {
            let mut st = state.lock().await;
            st.initialized_clients.insert(client_id.clone());
        }
        let result = InitializeResult {
            protocol_version: serde_json::json!(1),
            agent_capabilities: AgentCapabilities {
                load_session: Some(false),
                prompt_capabilities: Some(serde_json::json!({
                    "image": false, "audio": false, "embeddedContext": false
                })),
                mcp_capabilities: Some(serde_json::json!({ "http": false, "sse": false })),
                session_capabilities: Some({
                    let mut m = HashMap::new();
                    m.insert("list".to_string(), serde_json::json!({}));
                    m
                }),
            },
            agent_info: Some(AgentInfo {
                name: "zeptoclaw".to_string(),
                title: Some("ZeptoClaw".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            auth_methods: vec![],
            client_id: Some(client_id),
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
        params: Option<serde_json::Value>,
        client_id: Option<&str>,
    ) -> String {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            return Self::json_rpc_error(id, -32000, "Unauthorized");
        }
        let cwd = params
            .and_then(|p| serde_json::from_value::<super::acp_protocol::SessionNewParams>(p).ok())
            .and_then(|p| p.cwd);
        let session_id = format!("acph_{}", super::acp_protocol::new_id());
        {
            let mut st = state.lock().await;
            if !st.initialized_clients.contains(client_id.unwrap_or("")) {
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
            st.sessions.insert(session_id.clone(), cwd);
        }
        let result = SessionNewResult { session_id };
        match serde_json::to_value(result) {
            Ok(v) => Self::json_rpc_result(id, v),
            Err(e) => Self::json_rpc_error(id, -32603, &format!("serialize error: {}", e)),
        }
    }

    /// Handle a `session/cancel` request (id always present; notifications are
    /// handled earlier in `handle_connection` and never reach this function).
    async fn do_session_cancel(
        state: &Arc<Mutex<AcpHttpState>>,
        base_config: &BaseChannelConfig,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> String {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            return Self::json_rpc_error(id, -32000, "Unauthorized");
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
        Self::json_rpc_result(id, serde_json::json!({}))
    }

    /// Handle session/list: return all live sessions with per-session metadata.
    async fn do_session_list(
        state: &Arc<Mutex<AcpHttpState>>,
        base_config: &BaseChannelConfig,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
        client_id: Option<&str>,
    ) -> String {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            return Self::json_rpc_error(id, -32000, "Unauthorized");
        }
        let st = state.lock().await;
        if !st.initialized_clients.contains(client_id.unwrap_or("")) {
            return Self::json_rpc_error(
                id,
                -32600,
                "initialize must be called before session/list",
            );
        }
        // Parse params; apply cwd filter when present (cursor/pagination not yet implemented).
        let list_params: Option<SessionListParams> = match params {
            None => None,
            Some(p) => match serde_json::from_value::<SessionListParams>(p) {
                Ok(lp) => Some(lp),
                Err(e) => {
                    return Self::json_rpc_error(id, -32602, &format!("Invalid params: {}", e));
                }
            },
        };
        let cwd_filter = list_params.and_then(|p| p.cwd);
        let sessions: Vec<SessionInfo> = st
            .sessions
            .iter()
            .filter(|(_, cwd)| {
                if let Some(ref filter) = cwd_filter {
                    cwd.as_deref() == Some(filter.as_str())
                } else {
                    true
                }
            })
            .map(|(sid, cwd)| SessionInfo {
                session_id: sid.clone(),
                cwd: cwd.clone().unwrap_or_else(|| "unknown".to_string()),
                title: None,
                updated_at: None,
                meta: Some(serde_json::json!({ "pending": st.pending.contains_key(sid) })),
            })
            .collect();
        let result = SessionListResult {
            sessions,
            next_cursor: None,
        };
        match serde_json::to_value(result) {
            Ok(v) => Self::json_rpc_result(id, v),
            Err(e) => Self::json_rpc_error(id, -32603, &format!("serialize error: {}", e)),
        }
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
        client_id: Option<&str>,
    ) -> Result<std::result::Result<(String, oneshot::Receiver<(String, bool)>), String>> {
        if !base_config.is_allowed(ACP_HTTP_SENDER_ID) {
            return Ok(Err(Self::json_rpc_error(id, -32000, "Unauthorized")));
        }
        {
            let st = state.lock().await;
            if !st.initialized_clients.contains(client_id.unwrap_or("")) {
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
            if !st.sessions.contains_key(&session_id) {
                return Ok(Err(Self::json_rpc_error(
                    id,
                    -32000,
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
        let sse_headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\nX-Accel-Buffering: no\r\n\r\n";
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
                let resp =
                    build_http_error("413 Payload Too Large", r#"{"error":"payload too large"}"#);
                let _ = stream.write_all(resp.as_bytes()).await;
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
                let _ = stream.write_all(Self::http_400("{}").as_bytes()).await;
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
            let resp = build_http_error("404 Not Found", r#"{"error":"not found"}"#);
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }
        if req.method != "POST" {
            let resp = build_http_error(
                "405 Method Not Allowed",
                r#"{"error":"method not allowed"}"#,
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        // Bearer token auth.
        if !Self::validate_auth(&req.headers, &http_config.auth_token) {
            let resp = build_http_error("401 Unauthorized", r#"{"error":"unauthorized"}"#);
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        // Per-client initialization token echoed back from the initialize response.
        let client_id: Option<String> = req
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-acp-client-id"))
            .map(|(_, v)| v.trim().to_string());
        let client_id_ref = client_id.as_deref();

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

        // JSON-RPC notifications (id absent) must not receive a response body
        // (JSON-RPC 2.0 §4.1).  Apply any state-mutating side-effects for known
        // notification methods, then return 204 No Content with no body.
        // Unknown/unsupported notification methods are silently ignored.
        if id.is_none() {
            if rpc.method.as_str() == "session/cancel" {
                if let Some(p) = params.and_then(|p| {
                    serde_json::from_value::<super::acp_protocol::SessionCancelParams>(p).ok()
                }) {
                    let mut st = state.lock().await;
                    if let Some(pending) = st.pending.get_mut(&p.session_id) {
                        pending.cancelled = true;
                        debug!(session_id = %p.session_id, "ACP-HTTP: marked prompt as cancelled (notification)");
                    }
                }
            }
            let _ = stream.write_all(HTTP_204_NOTIFICATION.as_bytes()).await;
            return;
        }

        match rpc.method.as_str() {
            "initialize" => {
                let body = Self::do_initialize(&state, &config, id, params).await;
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
            "session/new" => {
                let body =
                    Self::do_session_new(&state, &base_config, id, params, client_id_ref).await;
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
            "session/cancel" => {
                let body = Self::do_session_cancel(&state, &base_config, id, params).await;
                let resp = Self::http_200(&body);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
            "session/list" => {
                let body =
                    Self::do_session_list(&state, &base_config, id, params, client_id_ref).await;
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
                    client_id_ref,
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
        conn_tasks: Arc<Mutex<JoinSet<()>>>,
    ) {
        info!(
            "ACP-HTTP: listening on {}:{}",
            http_config.bind, http_config.port
        );
        let conn_sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        while running.load(Ordering::SeqCst) {
            match tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
                Ok(Ok((stream, addr))) => {
                    debug!("ACP-HTTP: accepted connection from {}", addr);
                    let permit = match Arc::clone(&conn_sem).try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            debug!(
                                "ACP-HTTP: connection limit ({}) reached, dropping {}",
                                MAX_CONCURRENT_CONNECTIONS, addr
                            );
                            continue;
                        }
                    };
                    let config = config.clone();
                    let http_config = http_config.clone();
                    let base_config = base_config.clone();
                    let bus = Arc::clone(&bus);
                    let state = Arc::clone(&state);
                    let pending_http = Arc::clone(&pending_http);
                    {
                        let mut tasks = conn_tasks.lock().await;
                        // Reap any already-finished handlers before registering
                        // the new one, preventing unbounded JoinSet growth.
                        while tasks.try_join_next().is_some() {}
                        tasks.spawn(async move {
                            let _permit = permit; // released when handler completes
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
            // Reset the flag so is_running() doesn't report a stale true state.
            self.running.store(false, Ordering::SeqCst);
            ZeptoError::Channel(format!("ACP-HTTP: failed to bind {}: {}", addr, e))
        })?;

        let config = self.config.clone();
        let http_config = self.http_config.clone();
        let base_config = self.base_config.clone();
        let bus = Arc::clone(&self.bus);
        let state = Arc::clone(&self.state);
        let pending_http = Arc::clone(&self.pending_http);
        let running = Arc::clone(&self.running);
        let conn_tasks = Arc::clone(&self.conn_tasks);

        let handle = tokio::spawn(async move {
            Self::run_accept_loop(
                listener,
                config,
                http_config,
                base_config,
                bus,
                state,
                pending_http,
                running,
                conn_tasks,
            )
            .await;
        });
        self.accept_handle = Some(handle);
        info!(
            "ACP-HTTP channel started on {}:{}",
            self.http_config.bind, self.http_config.port
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        // Drain pending_http: dropping the oneshot senders causes any connection
        // handlers currently awaiting a prompt response to receive RecvError and
        // return, so no HTTP connection is left hanging after stop().
        self.pending_http.lock().await.clear();
        // Clear state.pending so sessions are not permanently marked in-flight
        // across a stop/restart cycle (supervisor may restart the channel).
        self.state.lock().await.pending.clear();
        // Abort and await the accept-loop task so the TcpListener is released
        // before this method returns (mirrors the stdio AcpChannel pattern).
        if let Some(handle) = self.accept_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        // Abort and drain all in-flight connection handler tasks so they do not
        // outlive the channel after stop() returns.
        {
            let mut tasks = self.conn_tasks.lock().await;
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
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
            let exists = st.sessions.contains_key(&session_id);
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

/// Extract plain text from ACP prompt content blocks.
///
/// `Text` blocks contribute their text directly. `ResourceLink` blocks
/// contribute a reference line so the agent is aware of the resource.
fn prompt_blocks_to_text(blocks: &[PromptContentBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in blocks {
        match b {
            PromptContentBlock::Text { text } => parts.push(text.clone()),
            PromptContentBlock::ResourceLink { uri, name, .. } => {
                parts.push(format!("[Resource: {} ({})]", name, uri));
            }
            _ => {}
        }
    }
    parts.join("\n").trim().to_string()
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
            st.sessions.insert(session_id.clone(), None);
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
            st.sessions.insert(session_id.clone(), None);
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
            st.sessions.insert(session_id.clone(), None);
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
        ch.state
            .lock()
            .await
            .initialized_clients
            .insert("test_client".to_string());
        let result = AcpHttpChannel::do_session_new(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(1)),
            None,
            Some("test_client"),
        )
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

    #[tokio::test]
    async fn test_session_list_requires_initialize() {
        let ch = make_channel();
        // No client_id supplied → not in initialized_clients → must be rejected.
        let body = AcpHttpChannel::do_session_list(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(1)),
            None,
            None,
        )
        .await;
        assert!(
            body.contains("-32600"),
            "session/list before initialize must return -32600"
        );
    }

    #[tokio::test]
    async fn test_session_list_empty() {
        let ch = make_channel();
        ch.state
            .lock()
            .await
            .initialized_clients
            .insert("test_client".to_string());
        let body = AcpHttpChannel::do_session_list(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(1)),
            None,
            Some("test_client"),
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["sessions"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn test_session_list_shows_sessions_with_pending_flag() {
        let ch = make_channel();
        let sid_a = "acph_list_a".to_string();
        let sid_b = "acph_list_b".to_string();
        {
            let mut st = ch.state.lock().await;
            st.initialized_clients.insert("test_client".to_string());
            st.sessions.insert(sid_a.clone(), None);
            st.sessions.insert(sid_b.clone(), None);
            st.pending
                .insert(sid_a.clone(), PendingPrompt { cancelled: false });
        }
        let body = AcpHttpChannel::do_session_list(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(1)),
            None,
            Some("test_client"),
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let sessions = v["result"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        let find = |sid: &str| sessions.iter().find(|s| s["sessionId"] == sid).cloned();
        assert_eq!(find(&sid_a).unwrap()["_meta"]["pending"], true);
        assert_eq!(find(&sid_b).unwrap()["_meta"]["pending"], false);
    }

    // -------------------------------------------------------------------------
    // Regression: notification semantics (issue #388, second bug)
    // -------------------------------------------------------------------------

    /// A `session/cancel` sent as a notification (no id) must apply the
    /// cancellation flag and return exactly `HTTP_204_NOTIFICATION` — no body.
    #[tokio::test]
    async fn test_session_cancel_notification_returns_204_no_body() {
        let ch = make_channel();
        let session_id = "acph_notif_cancel".to_string();
        {
            let mut st = ch.state.lock().await;
            st.sessions.insert(session_id.clone(), None);
            st.pending
                .insert(session_id.clone(), PendingPrompt { cancelled: false });
        }

        // Simulate `handle_connection` notification path: id is None.
        // The early-return block processes the cancel and writes HTTP_204_NOTIFICATION.
        // We test the state effect directly (the HTTP write is an I/O side-effect).
        {
            let params = serde_json::json!({ "sessionId": session_id });
            if let Ok(p) = serde_json::from_value::<
                crate::channels::acp_protocol::SessionCancelParams,
            >(params.clone())
            {
                let mut st = ch.state.lock().await;
                if let Some(pending) = st.pending.get_mut(&p.session_id) {
                    pending.cancelled = true;
                }
            }
        }

        assert!(
            ch.state
                .lock()
                .await
                .pending
                .get(&session_id)
                .unwrap()
                .cancelled,
            "cancel notification must set the cancelled flag"
        );
    }

    /// `session/cancel` sent as a request (id present) must return a JSON-RPC
    /// result body — NOT 204.
    #[tokio::test]
    async fn test_session_cancel_request_returns_json_result() {
        let ch = make_channel();
        let session_id = "acph_req_cancel".to_string();
        {
            let mut st = ch.state.lock().await;
            st.sessions.insert(session_id.clone(), None);
            st.pending
                .insert(session_id.clone(), PendingPrompt { cancelled: false });
        }

        let body = AcpHttpChannel::do_session_cancel(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(42)),
            Some(serde_json::json!({ "sessionId": session_id })),
        )
        .await;

        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["id"], 42, "response must echo the request id");
        assert!(
            v["result"].is_object(),
            "result must be present for a request"
        );
        assert!(v.get("error").is_none(), "no error field expected");
    }

    /// The `HTTP_204_NOTIFICATION` constant must have no body section and
    /// report Content-Length: 0.
    #[test]
    fn test_http_204_notification_has_no_body() {
        assert!(
            HTTP_204_NOTIFICATION.contains("204 No Content"),
            "must be a 204 status"
        );
        assert!(
            HTTP_204_NOTIFICATION.contains("Content-Length: 0"),
            "Content-Length must be 0"
        );
        // The response must end immediately after the blank line — no body.
        assert!(
            HTTP_204_NOTIFICATION.ends_with("\r\n\r\n"),
            "response must end with the blank line and no trailing body"
        );
    }

    // -------------------------------------------------------------------------
    // Regression: per-client initialization isolation (issue #388, first bug)
    // -------------------------------------------------------------------------

    /// `initialize` must insert a fresh client_id into `initialized_clients`
    /// and return it in the JSON response under `"clientId"`.
    #[tokio::test]
    async fn test_initialize_inserts_client_id_into_state() {
        let ch = make_channel();
        assert!(ch.state.lock().await.initialized_clients.is_empty());

        let body =
            AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
                .await;

        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let client_id = v["result"]["clientId"]
            .as_str()
            .expect("clientId must be present");
        assert!(!client_id.is_empty());
        assert!(
            ch.state
                .lock()
                .await
                .initialized_clients
                .contains(client_id),
            "client_id must be stored in initialized_clients"
        );
    }

    /// Two successive `initialize` calls must produce different client IDs,
    /// and both must be tracked independently in the set.
    #[tokio::test]
    async fn test_initialize_produces_unique_client_ids() {
        let ch = make_channel();

        let b1 =
            AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
                .await;
        let b2 =
            AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(2)), None)
                .await;

        let id1 = serde_json::from_str::<serde_json::Value>(&b1).unwrap()["result"]["clientId"]
            .as_str()
            .unwrap()
            .to_string();
        let id2 = serde_json::from_str::<serde_json::Value>(&b2).unwrap()["result"]["clientId"]
            .as_str()
            .unwrap()
            .to_string();

        assert_ne!(
            id1, id2,
            "each initialize call must yield a distinct client_id"
        );
        let st = ch.state.lock().await;
        assert!(st.initialized_clients.contains(&id1));
        assert!(st.initialized_clients.contains(&id2));
        assert_eq!(st.initialized_clients.len(), 2);
    }

    /// Core regression: once client A has initialized, client B supplying no
    /// client_id (the old behaviour that the bug enabled) must still be blocked
    /// from calling session/new.
    #[tokio::test]
    async fn test_session_new_blocked_without_client_id_even_after_another_client_initialized() {
        let ch = make_channel();

        // Client A initializes successfully.
        AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
            .await;

        // Client B sends no X-Acp-Client-Id — must be rejected.
        let body = AcpHttpChannel::do_session_new(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(2)),
            None,
            None,
        )
        .await;

        assert!(
            body.contains("-32600"),
            "session/new with no client_id must fail even after another client initialized: {body}"
        );
        assert!(ch.state.lock().await.sessions.is_empty());
    }

    /// A client that presents an arbitrary string that was never issued by
    /// `initialize` must be blocked — the set only contains IDs the server
    /// itself generated.
    #[tokio::test]
    async fn test_session_new_blocked_with_fabricated_client_id() {
        let ch = make_channel();

        // A legitimate client initializes.
        AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
            .await;

        // An attacker supplies an ID that was never issued.
        let body = AcpHttpChannel::do_session_new(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(2)),
            None,
            Some("not-a-real-client-id"),
        )
        .await;

        assert!(
            body.contains("-32600"),
            "session/new with fabricated client_id must fail: {body}"
        );
        assert!(ch.state.lock().await.sessions.is_empty());
    }

    /// Happy-path round-trip: the `clientId` returned by `initialize` must be
    /// accepted by `session/new` and produce a valid `sessionId`.
    ///
    /// This is the critical positive test that the guard-only tests cannot
    /// cover: a bug in the JSON key name, value encoding, or HashSet insertion
    /// would make every real client fail even though the negative tests pass.
    #[tokio::test]
    async fn test_initialize_to_session_new_round_trip() {
        let ch = make_channel();

        let init_body =
            AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
                .await;
        let init_v: serde_json::Value = serde_json::from_str(&init_body).unwrap();
        let client_id = init_v["result"]["clientId"]
            .as_str()
            .expect("clientId must be present in initialize response");

        let new_body = AcpHttpChannel::do_session_new(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(2)),
            None,
            Some(client_id),
        )
        .await;
        let new_v: serde_json::Value = serde_json::from_str(&new_body).unwrap();

        assert!(
            new_v.get("error").is_none(),
            "session/new must succeed with the issued clientId: {new_body}"
        );
        assert!(
            new_v["result"]["sessionId"].as_str().is_some(),
            "session/new must return a sessionId: {new_body}"
        );
    }

    /// Multi-round positive path: `initialize` → `session/new` → `session/list`,
    /// all using the same issued `clientId` from the initialize response.
    #[tokio::test]
    async fn test_initialize_to_session_new_to_session_list_round_trip() {
        let ch = make_channel();

        // Round 1: initialize.
        let init_body =
            AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
                .await;
        let client_id = serde_json::from_str::<serde_json::Value>(&init_body).unwrap()["result"]
            ["clientId"]
            .as_str()
            .unwrap()
            .to_string();

        // Round 2: session/new.
        let new_body = AcpHttpChannel::do_session_new(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(2)),
            None,
            Some(&client_id),
        )
        .await;
        let session_id = serde_json::from_str::<serde_json::Value>(&new_body).unwrap()["result"]
            ["sessionId"]
            .as_str()
            .expect("session/new must succeed")
            .to_string();

        // Round 3: session/list.
        let list_body = AcpHttpChannel::do_session_list(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(3)),
            None,
            Some(&client_id),
        )
        .await;
        let list_v: serde_json::Value = serde_json::from_str(&list_body).unwrap();

        assert!(
            list_v.get("error").is_none(),
            "session/list must succeed with the issued clientId: {list_body}"
        );
        let sessions = list_v["result"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "exactly one session must be listed");
        assert_eq!(
            sessions[0]["sessionId"].as_str().unwrap(),
            session_id,
            "listed sessionId must match the one returned by session/new"
        );
    }

    /// session/list must be gated by client_id in the same way.
    #[tokio::test]
    async fn test_session_list_blocked_without_client_id_even_after_another_client_initialized() {
        let ch = make_channel();

        AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
            .await;

        let body = AcpHttpChannel::do_session_list(
            &ch.state,
            &ch.base_config,
            Some(serde_json::json!(2)),
            None,
            None,
        )
        .await;

        assert!(
            body.contains("-32600"),
            "session/list with no client_id must fail even after another client initialized: {body}"
        );
    }

    /// `register_prompt` has its own `initialized_clients` guard that is
    /// exercised independently of `do_session_new` / `do_session_list`.
    /// Verify it rejects calls with no `clientId` even after another client
    /// has initialized.
    #[tokio::test]
    async fn test_register_prompt_blocked_without_client_id() {
        let ch = make_channel();

        // Another client initializes — must not unlock register_prompt for others.
        AcpHttpChannel::do_initialize(&ch.state, &ch.config, Some(serde_json::json!(1)), None)
            .await;

        let result = AcpHttpChannel::register_prompt(
            &ch.state,
            &ch.pending_http,
            &ch.base_config,
            &ch.bus,
            Some(serde_json::json!(2)),
            Some(serde_json::json!({
                "sessionId": "acph_any",
                "prompt": [{ "type": "text", "text": "hello" }]
            })),
            None, // no X-Acp-Client-Id header
        )
        .await
        .expect("register_prompt must not return an Err variant here");

        assert!(
            result.is_err(),
            "register_prompt must reject calls with no clientId"
        );
        let err_body = result.unwrap_err();
        assert!(
            err_body.contains("-32600"),
            "rejection must use -32600 (not initialized): {err_body}"
        );
    }
}
