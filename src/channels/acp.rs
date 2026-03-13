//! ACP (Agent Client Protocol) stdio channel.
//!
//! When enabled, ZeptoClaw acts as an ACP agent: it reads JSON-RPC from stdin
//! and writes responses/notifications to stdout. Supports initialize, session/new,
//! session/prompt, and session/cancel. Session/update is sent when the agent
//! produces a reply.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::AcpChannelConfig;
use crate::error::{Result, ZeptoError};

use super::acp_protocol::{
    AgentCapabilities, AgentInfo, ContentBlock, InitializeResult, JsonRpcRequest, JsonRpcResponse,
    SessionNewResult, SessionPromptResult, SessionUpdateParams, SessionUpdatePayload,
};
use super::{BaseChannelConfig, Channel};

const ACP_CHANNEL_NAME: &str = "acp";
const ACP_SENDER_ID: &str = "acp_client";
/// Maximum prompt content size in bytes (matches the safety validator limit).
const MAX_PROMPT_BYTES: usize = 102_400;
/// Maximum number of live ACP sessions at once.
const MAX_ACP_SESSIONS: usize = 1_000;

/// Pending session/prompt request: (JSON-RPC id, cancelled flag).
struct PendingPrompt {
    request_id: serde_json::Value,
    cancelled: bool,
}

/// Shared state for the ACP channel (sessions and pending prompt per session).
struct AcpState {
    /// Whether the client has called initialize.
    initialized: bool,
    /// Session IDs created via session/new.
    sessions: std::collections::HashSet<String>,
    /// Per-session pending prompt: we respond when we get the matching outbound message.
    pending: HashMap<String, PendingPrompt>,
}

impl AcpState {
    fn new() -> Self {
        Self {
            initialized: false,
            sessions: std::collections::HashSet::new(),
            pending: HashMap::new(),
        }
    }
}

/// ACP stdio channel: reads JSON-RPC from stdin, publishes to bus, sends responses on stdout.
pub struct AcpChannel {
    config: AcpChannelConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<AcpState>>,
    stdout: Arc<Mutex<tokio::io::Stdout>>,
}

impl AcpChannel {
    /// Create a new ACP channel.
    pub fn new(
        config: AcpChannelConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
    ) -> Self {
        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(AcpState::new())),
            stdout: Arc::new(Mutex::new(tokio::io::stdout())),
        }
    }

    /// Write a JSON-RPC message to stdout (newline-delimited per ACP stdio transport).
    async fn write_response(&self, response: &JsonRpcResponse) -> Result<()> {
        let line = serde_json::to_string(response).map_err(|e| {
            ZeptoError::Channel(format!("ACP: failed to serialize response: {}", e))
        })?;
        if line.contains('\n') {
            return Err(ZeptoError::Channel(
                "ACP: response must not contain newlines".into(),
            ));
        }
        let mut out = self.stdout.lock().await;
        out.write_all(line.as_bytes()).await?;
        out.write_all(b"\n").await?;
        out.flush().await?;
        Ok(())
    }

    /// Write a notification (no id) to stdout.
    async fn write_notification(&self, method: &str, params: &serde_json::Value) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let line = serde_json::to_string(&msg).map_err(|e| {
            ZeptoError::Channel(format!("ACP: failed to serialize notification: {}", e))
        })?;
        if line.contains('\n') {
            return Err(ZeptoError::Channel(
                "ACP: notification must not contain newlines".into(),
            ));
        }
        let mut out = self.stdout.lock().await;
        out.write_all(line.as_bytes()).await?;
        out.write_all(b"\n").await?;
        out.flush().await?;
        Ok(())
    }

    /// Handle session/new: create session and return sessionId.
    async fn handle_session_new(
        &self,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        if !self.is_allowed(ACP_SENDER_ID) {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(super::acp_protocol::JsonRpcError {
                    code: -32000,
                    message: "Unauthorized".to_string(),
                    data: None,
                }),
            };
            return self.write_response(&response).await;
        }
        let _params: Option<super::acp_protocol::SessionNewParams> =
            params.and_then(|p| serde_json::from_value(p).ok());
        let session_id = format!("acp_{}", uuid::Uuid::new_v4().simple());
        {
            let mut state = self.state.lock().await;
            if !state.initialized {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(super::acp_protocol::JsonRpcError {
                        code: -32600,
                        message: "initialize must be called before session/new".to_string(),
                        data: None,
                    }),
                };
                return self.write_response(&response).await;
            }
            if state.sessions.len() >= MAX_ACP_SESSIONS {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(super::acp_protocol::JsonRpcError {
                        code: -32000,
                        message: format!("Too many sessions (limit: {})", MAX_ACP_SESSIONS),
                        data: None,
                    }),
                };
                return self.write_response(&response).await;
            }
            state.sessions.insert(session_id.clone());
        }
        let result = SessionNewResult {
            session_id: session_id.clone(),
        };
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(serde_json::to_value(result).map_err(|e| {
                ZeptoError::Channel(format!("ACP: serialize session/new result: {}", e))
            })?),
            error: None,
        };
        self.write_response(&response).await
    }

    /// Extract plain text from session/prompt content blocks (minimal: text only).
    pub(crate) fn prompt_blocks_to_text(
        prompt: &[super::acp_protocol::PromptContentBlock],
    ) -> String {
        let mut parts = Vec::new();
        for block in prompt {
            if let super::acp_protocol::PromptContentBlock::Text { text } = block {
                parts.push(text.clone());
            }
        }
        parts.join("\n").trim().to_string()
    }

    /// Handle session/prompt: publish to bus and record pending response.
    async fn handle_session_prompt(
        &self,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        if !self.is_allowed(ACP_SENDER_ID) {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(super::acp_protocol::JsonRpcError {
                    code: -32000,
                    message: "Unauthorized".to_string(),
                    data: None,
                }),
            };
            return self.write_response(&response).await;
        }
        {
            let state = self.state.lock().await;
            if !state.initialized {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: id.clone(),
                    result: None,
                    error: Some(super::acp_protocol::JsonRpcError {
                        code: -32600,
                        message: "initialize must be called before session/prompt".to_string(),
                        data: None,
                    }),
                };
                return self.write_response(&response).await;
            }
        }
        let params: super::acp_protocol::SessionPromptParams = params
            .and_then(|p| serde_json::from_value(p).ok())
            .ok_or_else(|| {
                ZeptoError::Channel("ACP: session/prompt missing or invalid params".into())
            })?;
        let session_id = params.session_id;
        let content = Self::prompt_blocks_to_text(&params.prompt);
        if content.is_empty() {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: id.clone(),
                result: None,
                error: Some(super::acp_protocol::JsonRpcError {
                    code: -32602,
                    message: "session/prompt: prompt content is empty".to_string(),
                    data: None,
                }),
            };
            return self.write_response(&response).await;
        }
        if content.len() > MAX_PROMPT_BYTES {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: id.clone(),
                result: None,
                error: Some(super::acp_protocol::JsonRpcError {
                    code: -32602,
                    message: format!(
                        "session/prompt: content too large ({} bytes, limit {} bytes)",
                        content.len(),
                        MAX_PROMPT_BYTES
                    ),
                    data: None,
                }),
            };
            return self.write_response(&response).await;
        }
        {
            let mut state = self.state.lock().await;
            if !state.sessions.contains(&session_id) {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: id.clone(),
                    result: None,
                    error: Some(super::acp_protocol::JsonRpcError {
                        code: -32602,
                        message: format!("ACP: unknown session {}", session_id),
                        data: None,
                    }),
                };
                return self.write_response(&response).await;
            }
            if state.pending.contains_key(&session_id) {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: id.clone(),
                    result: None,
                    error: Some(super::acp_protocol::JsonRpcError {
                        code: -32602,
                        message: "session/prompt: a prompt is already in flight for this session"
                            .to_string(),
                        data: None,
                    }),
                };
                return self.write_response(&response).await;
            }
            state.pending.insert(
                session_id.clone(),
                PendingPrompt {
                    request_id: id.clone().unwrap_or(serde_json::Value::Null),
                    cancelled: false,
                },
            );
        }
        let inbound = InboundMessage::new(ACP_CHANNEL_NAME, ACP_SENDER_ID, &session_id, &content);
        if let Err(e) = self.bus.publish_inbound(inbound).await {
            let mut state = self.state.lock().await;
            state.pending.remove(&session_id);
            return Err(ZeptoError::Channel(format!(
                "ACP: failed to publish inbound: {}",
                e
            )));
        }
        debug!(session_id = %session_id, "ACP: published session/prompt to bus");
        Ok(())
    }

    /// Handle session/cancel: mark pending prompt as cancelled for that session.
    async fn handle_session_cancel(&self, params: Option<serde_json::Value>) -> Result<()> {
        let params: super::acp_protocol::SessionCancelParams = params
            .and_then(|p| serde_json::from_value(p).ok())
            .ok_or_else(|| {
                ZeptoError::Channel("ACP: session/cancel missing or invalid params".into())
            })?;
        let mut state = self.state.lock().await;
        if !state.sessions.contains(&params.session_id) {
            debug!(session_id = %params.session_id, "ACP: session/cancel for unknown session, ignoring");
            return Ok(());
        }
        if let Some(pending) = state.pending.get_mut(&params.session_id) {
            pending.cancelled = true;
            debug!(session_id = %params.session_id, "ACP: marked prompt as cancelled");
        }
        Ok(())
    }

    /// Stdin read loop: parse JSON-RPC and dispatch.
    async fn run_stdin_loop(
        bus: Arc<MessageBus>,
        state: Arc<Mutex<AcpState>>,
        stdout: Arc<Mutex<tokio::io::Stdout>>,
        config: AcpChannelConfig,
        base_config: BaseChannelConfig,
        running: Arc<AtomicBool>,
    ) -> Result<()> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();
        while running.load(Ordering::SeqCst) {
            let line = match reader.next_line().await {
                Ok(Some(l)) => l,
                Ok(None) => break,
                Err(e) => {
                    error!(error = %e, "ACP: stdin read error");
                    break;
                }
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let request: JsonRpcRequest = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(e) => {
                    let _ = Self::write_error_response(
                        &stdout,
                        None,
                        -32700,
                        format!("Parse error: {}", e),
                    )
                    .await;
                    continue;
                }
            };
            if request.jsonrpc != "2.0" {
                let _ = Self::write_error_response(
                    &stdout,
                    request.id,
                    -32600,
                    "Invalid Request: jsonrpc must be 2.0".to_string(),
                )
                .await;
                continue;
            }
            let method = request.method.as_str();
            let id = request.id.clone();
            let params = request.params.clone();
            let result = match method {
                "initialize" => {
                    let channel =
                        Self::channel_ref(&bus, &state, &stdout, &config, &base_config, &running);
                    channel.handle_initialize(id.clone(), params).await
                }
                "session/new" => {
                    let channel =
                        Self::channel_ref(&bus, &state, &stdout, &config, &base_config, &running);
                    channel.handle_session_new(id.clone(), params).await
                }
                "session/prompt" => {
                    let channel =
                        Self::channel_ref(&bus, &state, &stdout, &config, &base_config, &running);
                    channel.handle_session_prompt(id.clone(), params).await
                }
                "session/cancel" => {
                    let channel =
                        Self::channel_ref(&bus, &state, &stdout, &config, &base_config, &running);
                    channel.handle_session_cancel(params).await
                }
                _ => {
                    let _ = Self::write_error_response(
                        &stdout,
                        id.clone(),
                        -32601,
                        format!("Method not found: {}", method),
                    )
                    .await;
                    Ok(())
                }
            };
            if let Err(e) = result {
                error!(method = %method, error = %e, "ACP: handler error");
                let _ = Self::write_error_response(
                    &stdout,
                    id,
                    -32603,
                    format!("Internal error: {}", e),
                )
                .await;
            }
        }
        // Graceful shutdown: complete any in-flight session/prompt requests with
        // stopReason "error" so clients don't hang waiting for a reply forever.
        let orphans: Vec<(String, PendingPrompt)> = {
            let mut st = state.lock().await;
            st.pending.drain().collect()
        };
        for (session_id, pending) in orphans {
            let result = SessionPromptResult {
                stop_reason: "error".to_string(),
            };
            if let Ok(result_val) = serde_json::to_value(result) {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: Some(pending.request_id),
                    result: Some(result_val),
                    error: None,
                };
                if let Ok(line) = serde_json::to_string(&response) {
                    let mut out = stdout.lock().await;
                    let _ = out.write_all(line.as_bytes()).await;
                    let _ = out.write_all(b"\n").await;
                    let _ = out.flush().await;
                    debug!(session_id = %session_id, "ACP: sent shutdown error for orphaned prompt");
                }
            }
        }
        running.store(false, Ordering::SeqCst);
        info!("ACP: stdin loop exited");
        Ok(())
    }

    fn channel_ref(
        bus: &Arc<MessageBus>,
        state: &Arc<Mutex<AcpState>>,
        stdout: &Arc<Mutex<tokio::io::Stdout>>,
        config: &AcpChannelConfig,
        base_config: &BaseChannelConfig,
        running: &Arc<AtomicBool>,
    ) -> AcpChannel {
        AcpChannel {
            config: config.clone(),
            base_config: base_config.clone(),
            bus: Arc::clone(bus),
            running: Arc::clone(running),
            state: Arc::clone(state),
            stdout: Arc::clone(stdout),
        }
    }

    /// Handle initialize: log client info, set initialized flag, return capabilities.
    async fn handle_initialize(
        &self,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        // Parse client info for diagnostics; missing or malformed params are fine.
        if let Some(init_params) = params
            .and_then(|p| serde_json::from_value::<super::acp_protocol::InitializeParams>(p).ok())
        {
            if let Some(ref client_info) = init_params.client_info {
                info!(
                    client_name = ?client_info.name,
                    client_version = ?client_info.version,
                    protocol_version = ?init_params.protocol_version,
                    "ACP: client initialized"
                );
            } else {
                debug!(
                    protocol_version = ?init_params.protocol_version,
                    "ACP: client initialized (no clientInfo)"
                );
            }
        }
        {
            let mut state = self.state.lock().await;
            state.initialized = true;
        }
        let result = InitializeResult {
            protocol_version: serde_json::json!(self.config.protocol_version),
            agent_capabilities: AgentCapabilities {
                load_session: Some(false),
                prompt_capabilities: Some(
                    serde_json::json!({ "image": false, "audio": false, "embeddedContext": false }),
                ),
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
        let response =
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::to_value(result).map_err(|e| {
                    ZeptoError::Channel(format!("ACP: serialize init result: {}", e))
                })?),
                error: None,
            };
        self.write_response(&response).await
    }

    async fn write_error_response(
        stdout: &Arc<Mutex<tokio::io::Stdout>>,
        id: Option<serde_json::Value>,
        code: i64,
        message: String,
    ) -> Result<()> {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(super::acp_protocol::JsonRpcError {
                code,
                message,
                data: None,
            }),
        };
        let line = serde_json::to_string(&response)
            .map_err(|e| ZeptoError::Channel(format!("ACP: serialize error: {}", e)))?;
        let mut out = stdout.lock().await;
        out.write_all(line.as_bytes()).await?;
        out.write_all(b"\n").await?;
        out.flush().await?;
        Ok(())
    }
}

#[async_trait]
impl Channel for AcpChannel {
    fn name(&self) -> &str {
        ACP_CHANNEL_NAME
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("ACP channel already running");
            return Ok(());
        }
        let bus = Arc::clone(&self.bus);
        let state = Arc::clone(&self.state);
        let stdout = Arc::clone(&self.stdout);
        let config = self.config.clone();
        let base_config = self.base_config.clone();
        let running = Arc::clone(&self.running);
        tokio::spawn(async move {
            if let Err(e) =
                Self::run_stdin_loop(bus, state, stdout, config, base_config, running).await
            {
                error!(error = %e, "ACP stdin loop error");
            }
        });
        info!("ACP channel started (stdio)");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if msg.channel != ACP_CHANNEL_NAME {
            return Ok(());
        }
        let session_id = msg.chat_id.clone();
        let (pending, session_exists) = {
            let mut state = self.state.lock().await;
            let exists = state.sessions.contains(&session_id);
            (state.pending.remove(&session_id), exists)
        };
        if !session_exists {
            debug!(session_id = %session_id, "ACP: outbound for unknown session, skipping");
            return Ok(());
        }
        // session/update (agent_message_chunk) — sent for both prompted and proactive replies
        let update = SessionUpdateParams {
            session_id: session_id.clone(),
            update: SessionUpdatePayload {
                session_update: "agent_message_chunk".to_string(),
                content: Some(ContentBlock::text(&msg.content)),
                tool_call_id: None,
                title: None,
                kind: None,
                status: None,
            },
        };
        let params = serde_json::to_value(&update)
            .map_err(|e| ZeptoError::Channel(format!("ACP: serialize session/update: {}", e)))?;
        self.write_notification("session/update", &params).await?;
        // For proactive messages there is no pending prompt to complete.
        let Some(pending) = pending else {
            debug!(session_id = %session_id, "ACP: proactive session/update sent");
            return Ok(());
        };
        // session/prompt response
        let stop_reason = if pending.cancelled {
            "cancelled"
        } else {
            "end_turn"
        };
        let result = SessionPromptResult {
            stop_reason: stop_reason.to_string(),
        };
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(pending.request_id),
            result: Some(serde_json::to_value(result).map_err(|e| {
                ZeptoError::Channel(format!("ACP: serialize prompt result: {}", e))
            })?),
            error: None,
        };
        self.write_response(&response).await
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AcpChannelConfig;

    #[tokio::test]
    async fn test_send_ignores_wrong_channel() {
        // send() with a channel other than "acp" must be a no-op: the pending
        // prompt must not be consumed and the session must remain intact.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let session_id = "acp_some_session".to_string();
        {
            let mut state = channel.state.lock().await;
            state.sessions.insert(session_id.clone());
            state.pending.insert(
                session_id.clone(),
                PendingPrompt {
                    request_id: serde_json::json!(1),
                    cancelled: false,
                },
            );
        }
        let msg = OutboundMessage {
            channel: "telegram".to_string(),
            chat_id: session_id.clone(),
            content: "hello".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        let result = channel.send(msg).await;
        assert!(result.is_ok());
        let state = channel.state.lock().await;
        assert!(
            state.pending.contains_key(&session_id),
            "wrong-channel send must not consume the pending prompt"
        );
    }

    #[test]
    fn test_acp_prompt_blocks_to_text() {
        use crate::channels::acp_protocol::PromptContentBlock;
        let blocks = vec![
            PromptContentBlock::Text {
                text: "Hello".to_string(),
            },
            PromptContentBlock::Text {
                text: "World".to_string(),
            },
        ];
        let text = AcpChannel::prompt_blocks_to_text(&blocks);
        assert_eq!(text, "Hello\nWorld");
    }

    #[test]
    fn test_acp_prompt_blocks_to_text_skips_non_text() {
        use crate::channels::acp_protocol::PromptContentBlock;
        let blocks = vec![
            PromptContentBlock::Text {
                text: "Only this".to_string(),
            },
            PromptContentBlock::Other,
        ];
        let text = AcpChannel::prompt_blocks_to_text(&blocks);
        assert_eq!(text, "Only this");
    }

    #[tokio::test]
    async fn test_deny_by_default_blocks_session_new() {
        // With deny_by_default=true and an empty allowlist, handle_session_new
        // must reject the request before creating any session.
        let config = AcpChannelConfig {
            deny_by_default: true,
            ..AcpChannelConfig::default()
        };
        let base = BaseChannelConfig {
            name: "acp".to_string(),
            allowlist: vec![],
            deny_by_default: true,
        };
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        {
            let mut state = channel.state.lock().await;
            state.initialized = true;
        }
        let _ = channel
            .handle_session_new(Some(serde_json::json!(1)), None)
            .await;
        let state = channel.state.lock().await;
        assert!(
            state.sessions.is_empty(),
            "deny_by_default must prevent session creation"
        );
    }

    #[tokio::test]
    async fn test_prompt_size_limit_does_not_insert_pending() {
        // An oversized prompt must be rejected before state is touched: no pending entry.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let session_id = "acp_test_size".to_string();
        {
            let mut state = channel.state.lock().await;
            state.initialized = true;
            state.sessions.insert(session_id.clone());
        }
        let oversized = "a".repeat(MAX_PROMPT_BYTES + 1);
        let params = serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": oversized}]
        });
        let _ = channel
            .handle_session_prompt(Some(serde_json::json!(1)), Some(params))
            .await;
        let state = channel.state.lock().await;
        assert!(
            !state.pending.contains_key(&session_id),
            "oversized prompt must not insert a pending entry"
        );
    }

    #[tokio::test]
    async fn test_in_flight_prompt_guard_preserves_first_request_id() {
        // A second session/prompt while one is in flight must be rejected and must
        // not overwrite the first request's id in state.pending.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let session_id = "acp_test_inflight".to_string();
        {
            let mut state = channel.state.lock().await;
            state.initialized = true;
            state.sessions.insert(session_id.clone());
            state.pending.insert(
                session_id.clone(),
                PendingPrompt {
                    request_id: serde_json::json!(1),
                    cancelled: false,
                },
            );
        }
        let params = serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "second prompt"}]
        });
        let _ = channel
            .handle_session_prompt(Some(serde_json::json!(2)), Some(params))
            .await;
        let state = channel.state.lock().await;
        let pending = state
            .pending
            .get(&session_id)
            .expect("original pending entry must still exist");
        assert_eq!(
            pending.request_id,
            serde_json::json!(1),
            "first request_id must not be overwritten by the second prompt"
        );
    }

    #[tokio::test]
    async fn test_initialize_sets_initialized_flag() {
        // handle_initialize must set state.initialized = true.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        assert!(!channel.state.lock().await.initialized);
        let _ = channel
            .handle_initialize(Some(serde_json::json!(1)), None)
            .await;
        assert!(
            channel.state.lock().await.initialized,
            "initialized must be true after handle_initialize"
        );
    }

    #[tokio::test]
    async fn test_session_new_requires_initialize() {
        // session/new must be rejected when initialize has not been called.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let _ = channel
            .handle_session_new(Some(serde_json::json!(1)), None)
            .await;
        let state = channel.state.lock().await;
        assert!(
            state.sessions.is_empty(),
            "no session must be created before initialize"
        );
    }

    #[tokio::test]
    async fn test_session_prompt_requires_initialize() {
        // session/prompt must be rejected when initialize has not been called.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let session_id = "acp_test_noinit".to_string();
        {
            // Seed a session directly to isolate the initialized check.
            let mut state = channel.state.lock().await;
            state.sessions.insert(session_id.clone());
        }
        let params = serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "hello"}]
        });
        let _ = channel
            .handle_session_prompt(Some(serde_json::json!(1)), Some(params))
            .await;
        let state = channel.state.lock().await;
        assert!(
            !state.pending.contains_key(&session_id),
            "no pending entry must be created before initialize"
        );
    }

    #[tokio::test]
    async fn test_session_cancel_unknown_does_not_affect_known_pending() {
        // Cancelling an unknown session must not touch pending entries for other sessions.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let known = "acp_known".to_string();
        {
            let mut state = channel.state.lock().await;
            state.sessions.insert(known.clone());
            state.pending.insert(
                known.clone(),
                PendingPrompt {
                    request_id: serde_json::json!(42),
                    cancelled: false,
                },
            );
        }
        let params = serde_json::json!({"sessionId": "acp_nonexistent"});
        let result = channel.handle_session_cancel(Some(params)).await;
        assert!(result.is_ok());
        let state = channel.state.lock().await;
        let pending = state
            .pending
            .get(&known)
            .expect("known session pending must be untouched");
        assert!(
            !pending.cancelled,
            "cancel of unknown session must not mark a different session as cancelled"
        );
    }

    #[tokio::test]
    async fn test_send_skips_unknown_session() {
        // send() for a session not in state.sessions must return Ok without
        // adding the session to state.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        // Deliberately do not seed any session.
        let msg = OutboundMessage {
            channel: ACP_CHANNEL_NAME.to_string(),
            chat_id: "acp_ghost".to_string(),
            content: "hello".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        let result = channel.send(msg).await;
        assert!(result.is_ok());
        let state = channel.state.lock().await;
        assert!(
            state.sessions.is_empty(),
            "send to unknown session must not create the session"
        );
    }

    #[tokio::test]
    async fn test_send_proactive_known_session_does_not_remove_session() {
        // send() for a known session with no pending prompt (proactive path) must
        // succeed and leave the session in state so it can accept future prompts.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        let session_id = "acp_proactive".to_string();
        {
            let mut state = channel.state.lock().await;
            state.sessions.insert(session_id.clone());
        }
        let msg = OutboundMessage {
            channel: ACP_CHANNEL_NAME.to_string(),
            chat_id: session_id.clone(),
            content: "proactive message".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        let result = channel.send(msg).await;
        assert!(result.is_ok());
        let state = channel.state.lock().await;
        assert!(
            state.sessions.contains(&session_id),
            "proactive send must not remove the session from state"
        );
    }

    #[tokio::test]
    async fn test_session_cap_does_not_insert_beyond_limit() {
        // When state.sessions is full, session/new must be rejected and must not
        // add a new entry.
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        {
            let mut state = channel.state.lock().await;
            state.initialized = true;
            for i in 0..MAX_ACP_SESSIONS {
                state.sessions.insert(format!("acp_{}", i));
            }
        }
        let _ = channel
            .handle_session_new(Some(serde_json::json!(1)), None)
            .await;
        let state = channel.state.lock().await;
        assert_eq!(
            state.sessions.len(),
            MAX_ACP_SESSIONS,
            "session count must not exceed the cap"
        );
    }
}
