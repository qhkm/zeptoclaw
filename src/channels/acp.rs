//! ACP (Agent Client Protocol) stdio channel.
//!
//! When enabled, ZeptoClaw acts as an ACP agent: it reads JSON-RPC from stdin
//! and writes responses/notifications to stdout. Supports initialize, session/new,
//! session/prompt, and session/cancel. Session/update is sent when the agent
//! produces a reply.

use async_trait::async_trait;
use futures::FutureExt;
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

/// Pending session/prompt request: (JSON-RPC id, cancelled flag).
struct PendingPrompt {
    request_id: serde_json::Value,
    cancelled: bool,
}

/// Shared state for the ACP channel (sessions and pending prompt per session).
struct AcpState {
    /// Session IDs created via session/new.
    sessions: std::collections::HashSet<String>,
    /// Per-session pending prompt: we respond when we get the matching outbound message.
    pending: HashMap<String, PendingPrompt>,
}

impl AcpState {
    fn new() -> Self {
        Self {
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
        let _params: Option<super::acp_protocol::SessionNewParams> =
            params.and_then(|p| serde_json::from_value(p).ok());
        let session_id = format!("acp_{}", uuid::Uuid::new_v4().simple());
        {
            let mut state = self.state.lock().await;
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
                "initialize" => Self::handle_initialize_static(&stdout, &config, id.clone()).await,
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

    async fn handle_initialize_static(
        stdout: &Arc<Mutex<tokio::io::Stdout>>,
        config: &AcpChannelConfig,
        id: Option<serde_json::Value>,
    ) -> Result<()> {
        let protocol_version = config.protocol_version.parse::<i64>().unwrap_or(1);
        let result = InitializeResult {
            protocol_version: serde_json::json!(protocol_version),
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
        let line = serde_json::to_string(&response)
            .map_err(|e| ZeptoError::Channel(format!("ACP: serialize response: {}", e)))?;
        let mut out = stdout.lock().await;
        out.write_all(line.as_bytes()).await?;
        out.write_all(b"\n").await?;
        out.flush().await?;
        Ok(())
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
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(async {
                Self::run_stdin_loop(bus, state, stdout, config, base_config, running_clone).await
            })
            .catch_unwind()
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!(error = %e, "ACP stdin loop error"),
                Err(e) => error!(error = ?e, "ACP stdin loop panicked"),
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
        let pending = {
            let mut state = self.state.lock().await;
            state.pending.remove(&session_id)
        };
        let Some(pending) = pending else {
            debug!(session_id = %session_id, "ACP: no pending prompt for outbound, skipping");
            return Ok(());
        };
        // session/update (agent_message_chunk)
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

    #[test]
    fn test_acp_channel_name() {
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::new("acp");
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        assert_eq!(channel.name(), ACP_CHANNEL_NAME);
    }

    #[test]
    fn test_acp_prompt_blocks_to_text() {
        use super::acp_protocol::PromptContentBlock;
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
        use super::acp_protocol::PromptContentBlock;
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
    async fn test_acp_channel_is_allowed() {
        let config = AcpChannelConfig::default();
        let base = BaseChannelConfig::with_allowlist("acp", vec!["acp_client".to_string()]);
        let bus = Arc::new(MessageBus::new());
        let channel = AcpChannel::new(config, base, bus);
        assert!(channel.is_allowed("acp_client"));
        assert!(!channel.is_allowed("other"));
    }
}
