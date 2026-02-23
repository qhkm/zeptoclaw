//! WhatsApp channel implementation (via whatsmeow-rs bridge).
//!
//! Connects to an external whatsmeow-rs bridge binary over WebSocket.
//! The bridge handles WhatsApp protocol complexity (E2E encryption, QR pairing,
//! session persistence). ZeptoClaw just consumes/sends JSON messages.
//!
//! # Bridge Protocol (JSON over WebSocket)
//!
//! Inbound (bridge → ZeptoClaw):
//! ```json
//! {"type":"message","from":"60123456789","chat_id":"60123456789@s.whatsapp.net","content":"Hello","message_id":"wamid.xyz","timestamp":1707900000,"sender_name":"John"}
//! {"type":"connected"}
//! {"type":"disconnected","reason":"session expired"}
//! {"type":"qr_code","data":"2@base64data"}
//! ```
//!
//! Outbound (ZeptoClaw → bridge):
//! ```json
//! {"type":"send","to":"60123456789@s.whatsapp.net","content":"Reply text","reply_to":"wamid.xyz"}
//! ```

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::WhatsAppConfig;
use crate::deps::{DepKind, Dependency, HasDependencies, HealthCheck};
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum reconnect delay (in seconds) for exponential backoff.
const MAX_RECONNECT_DELAY_SECS: u64 = 120;
/// Base reconnect delay (in seconds).
const BASE_RECONNECT_DELAY_SECS: u64 = 2;
/// Maximum number of consecutive reconnect attempts before resetting backoff.
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

// ---------------------------------------------------------------------------
// Bridge protocol types
// ---------------------------------------------------------------------------

/// Inbound message from the whatsmeow-rs bridge.
#[derive(Debug, Deserialize)]
struct BridgeMessage {
    /// Message type: "message", "connected", "disconnected", "qr_code", etc.
    #[serde(rename = "type")]
    msg_type: String,
    /// Sender phone number (message type only).
    #[serde(default)]
    from: Option<String>,
    /// WhatsApp chat JID (e.g. "60123456789@s.whatsapp.net").
    #[serde(default)]
    chat_id: Option<String>,
    /// Message text content.
    #[serde(default)]
    content: Option<String>,
    /// WhatsApp message ID.
    #[serde(default)]
    message_id: Option<String>,
    /// Unix timestamp.
    #[serde(default)]
    timestamp: Option<u64>,
    /// Sender display name.
    #[serde(default)]
    sender_name: Option<String>,
    /// Disconnect reason (disconnected type only).
    #[serde(default)]
    reason: Option<String>,
    /// QR code data (qr_code type only).
    #[serde(default)]
    #[allow(dead_code)]
    data: Option<String>,
}

/// Outbound message to the whatsmeow-rs bridge.
#[derive(Debug, Serialize)]
struct BridgeSendMessage {
    /// Always "send".
    #[serde(rename = "type")]
    msg_type: String,
    /// Recipient chat JID.
    to: String,
    /// Message text content.
    content: String,
    /// Optional message ID to reply to.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<String>,
}

// ---------------------------------------------------------------------------
// WhatsAppChannel
// ---------------------------------------------------------------------------

/// WhatsApp channel backed by the whatsmeow-rs bridge over WebSocket.
pub struct WhatsAppChannel {
    config: WhatsAppConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    shutdown_tx: Option<watch::Sender<bool>>,
    outbound_tx: Option<mpsc::Sender<BridgeSendMessage>>,
}

impl WhatsAppChannel {
    /// Creates a new WhatsApp channel.
    pub fn new(config: WhatsAppConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "whatsapp".to_string(),
            allowlist: config.allow_from.clone(),
            deny_by_default: config.deny_by_default,
        };

        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
            outbound_tx: None,
        }
    }

    /// Returns a reference to the WhatsApp configuration.
    pub fn whatsapp_config(&self) -> &WhatsAppConfig {
        &self.config
    }

    /// Returns whether the channel is enabled in configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    // -----------------------------------------------------------------------
    // Bridge message parsing
    // -----------------------------------------------------------------------

    /// Parses a bridge "message" event into an `InboundMessage`, returning
    /// `None` if it should be ignored (empty content, disallowed user, etc.).
    fn parse_bridge_message(
        msg: &BridgeMessage,
        allowlist: &[String],
        deny_by_default: bool,
    ) -> Option<InboundMessage> {
        let from = msg.from.as_deref().unwrap_or("").trim().to_string();
        if from.is_empty() {
            return None;
        }

        let chat_id = msg.chat_id.as_deref().unwrap_or("").trim().to_string();
        if chat_id.is_empty() {
            return None;
        }

        let content = msg.content.as_deref().unwrap_or("").trim().to_string();
        if content.is_empty() {
            return None;
        }

        // Allowlist check with deny_by_default support (by phone number).
        let allowed = if allowlist.is_empty() {
            !deny_by_default
        } else {
            allowlist.contains(&from)
        };
        if !allowed {
            info!("WhatsApp: user {} not in allowlist, ignoring message", from);
            return None;
        }

        let mut inbound = InboundMessage::new("whatsapp", &from, &chat_id, &content);

        if let Some(ref mid) = msg.message_id {
            inbound = inbound.with_metadata("whatsapp_message_id", mid);
        }
        if let Some(ts) = msg.timestamp {
            inbound = inbound.with_metadata("timestamp", &ts.to_string());
        }
        if let Some(ref name) = msg.sender_name {
            inbound = inbound.with_metadata("sender_name", name);
        }

        Some(inbound)
    }

    // -----------------------------------------------------------------------
    // Backoff calculation
    // -----------------------------------------------------------------------

    /// Calculates the exponential backoff delay for a given attempt number.
    fn backoff_delay(attempt: u32) -> Duration {
        let delay_secs = BASE_RECONNECT_DELAY_SECS
            .saturating_mul(2u64.saturating_pow(attempt))
            .min(MAX_RECONNECT_DELAY_SECS);
        Duration::from_secs(delay_secs)
    }

    // -----------------------------------------------------------------------
    // Error redaction
    // -----------------------------------------------------------------------

    /// Produce a safe log message for a WebSocket connection error.
    /// The tungstenite `Error` Debug representation may contain the full HTTP
    /// request including the `Authorization: Bearer <token>` header, so we
    /// must not log it verbatim.
    fn redact_ws_error(e: &tokio_tungstenite::tungstenite::Error) -> String {
        use tokio_tungstenite::tungstenite::Error as WsError;
        match e {
            WsError::ConnectionClosed => "connection closed".to_string(),
            WsError::AlreadyClosed => "already closed".to_string(),
            WsError::Io(io_err) => format!("IO error: {}", io_err.kind()),
            WsError::Tls(_) => "TLS error".to_string(),
            WsError::Capacity(msg) => format!("capacity: {}", msg),
            WsError::Protocol(p) => format!("protocol: {}", p),
            WsError::WriteBufferFull(_) => "write buffer full".to_string(),
            WsError::Utf8 => "UTF-8 error".to_string(),
            WsError::AttackAttempt => "attack attempt detected".to_string(),
            WsError::Url(u) => format!("URL error: {}", u),
            // Http variant may contain the full request with Authorization header.
            WsError::Http(resp) => format!("HTTP status {}", resp.status()),
            WsError::HttpFormat(_) => "HTTP format error".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Bridge WebSocket loop
    // -----------------------------------------------------------------------

    /// Main bridge loop: connects via WebSocket, dispatches inbound messages,
    /// and sends outbound messages. Reconnects with exponential backoff.
    async fn run_bridge_loop(
        bridge_url: String,
        bridge_token: Option<String>,
        bus: Arc<MessageBus>,
        allowlist: Vec<String>,
        deny_by_default: bool,
        mut shutdown_rx: watch::Receiver<bool>,
        mut outbound_rx: mpsc::Receiver<BridgeSendMessage>,
    ) {
        let mut reconnect_attempt: u32 = 0;

        loop {
            // Check shutdown before each connection attempt.
            if *shutdown_rx.borrow() {
                info!("WhatsApp bridge loop shutdown requested");
                return;
            }

            // --- WebSocket connect ---
            let ws_stream = tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("WhatsApp bridge loop shutdown requested");
                    return;
                }
                result = async {
                    if let Some(ref token) = bridge_token {
                        // Build request with Authorization header.
                        let request = tokio_tungstenite::tungstenite::http::Request::builder()
                            .uri(&bridge_url)
                            .header("Authorization", format!("Bearer {}", token))
                            .header("Host", tokio_tungstenite::tungstenite::http::Uri::try_from(bridge_url.as_str())
                                .map(|u| u.host().unwrap_or("localhost").to_string())
                                .unwrap_or_else(|_| "localhost".to_string()))
                            .header("Connection", "Upgrade")
                            .header("Upgrade", "websocket")
                            .header("Sec-WebSocket-Version", "13")
                            .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
                            .body(())
                            .expect("valid WebSocket request");
                        connect_async(request).await
                    } else {
                        connect_async(&bridge_url).await
                    }
                } => {
                    match result {
                        Ok((stream, _)) => stream,
                        Err(e) => {
                            // Redact the error to avoid leaking the bridge token.
                            // tungstenite may include the full HTTP request
                            // (with Authorization header) in error Debug output.
                            warn!("WhatsApp: bridge connect failed: {}", Self::redact_ws_error(&e));
                            let delay = Self::backoff_delay(reconnect_attempt);
                            reconnect_attempt =
                                (reconnect_attempt + 1).min(MAX_RECONNECT_ATTEMPTS);
                            tokio::select! {
                                _ = shutdown_rx.changed() => return,
                                _ = tokio::time::sleep(delay) => continue,
                            }
                        }
                    }
                }
            };

            info!("WhatsApp bridge WebSocket connected to {}", bridge_url);
            reconnect_attempt = 0;

            let (mut ws_writer, mut ws_reader) = ws_stream.split();

            // --- Main dispatch loop ---
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        info!("WhatsApp bridge loop shutdown requested");
                        return;
                    }

                    // Forward outbound messages to bridge.
                    outbound = outbound_rx.recv() => {
                        match outbound {
                            Some(send_msg) => {
                                match serde_json::to_string(&send_msg) {
                                    Ok(json) => {
                                        if let Err(e) = ws_writer.send(WsMessage::Text(json)).await {
                                            warn!("WhatsApp: failed to send to bridge: {}", e);
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("WhatsApp: failed to serialize outbound: {}", e);
                                    }
                                }
                            }
                            None => {
                                debug!("WhatsApp outbound channel closed");
                                break;
                            }
                        }
                    }

                    // Process incoming bridge events.
                    msg = ws_reader.next() => {
                        match msg {
                            Some(Ok(WsMessage::Text(raw))) => {
                                match serde_json::from_str::<BridgeMessage>(&raw) {
                                    Ok(bridge_msg) => {
                                        match bridge_msg.msg_type.as_str() {
                                            "message" => {
                                                if let Some(inbound) =
                                                    Self::parse_bridge_message(&bridge_msg, &allowlist, deny_by_default)
                                                {
                                                    if let Err(e) =
                                                        bus.publish_inbound(inbound).await
                                                    {
                                                        error!(
                                                            "Failed to publish WhatsApp inbound message: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                            "connected" => {
                                                info!("WhatsApp bridge: connected to WhatsApp");
                                            }
                                            "disconnected" => {
                                                let reason = bridge_msg
                                                    .reason
                                                    .as_deref()
                                                    .unwrap_or("unknown");
                                                warn!(
                                                    "WhatsApp bridge: disconnected (reason: {})",
                                                    reason
                                                );
                                                break; // Reconnect
                                            }
                                            "qr_code" => {
                                                info!(
                                                    "WhatsApp bridge: QR code received (display on bridge terminal)"
                                                );
                                            }
                                            other => {
                                                debug!(
                                                    "WhatsApp bridge: unknown message type '{}'",
                                                    other
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        debug!("WhatsApp: failed to parse bridge message: {}", e);
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Ping(payload))) => {
                                if let Err(e) = ws_writer.send(WsMessage::Pong(payload)).await {
                                    warn!("WhatsApp: pong send failed: {}", e);
                                    break;
                                }
                            }
                            Some(Ok(WsMessage::Close(frame))) => {
                                info!("WhatsApp: bridge WebSocket closed: {:?}", frame);
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                warn!("WhatsApp: bridge WebSocket error: {}", e);
                                break;
                            }
                            None => {
                                warn!("WhatsApp: bridge WebSocket stream ended");
                                break;
                            }
                        }
                    }
                }
            }

            // --- Wait before reconnecting ---
            let delay = Self::backoff_delay(reconnect_attempt);
            reconnect_attempt = (reconnect_attempt + 1).min(MAX_RECONNECT_ATTEMPTS);
            info!(
                "WhatsApp: reconnecting to bridge in {} seconds",
                delay.as_secs()
            );
            tokio::select! {
                _ = shutdown_rx.changed() => return,
                _ = tokio::time::sleep(delay) => {},
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Channel trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Channel for WhatsAppChannel {
    fn name(&self) -> &str {
        "whatsapp"
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("WhatsApp channel already running");
            return Ok(());
        }

        if !self.config.enabled {
            warn!("WhatsApp channel is disabled in configuration");
            self.running.store(false, Ordering::SeqCst);
            return Ok(());
        }

        let bridge_url = self.config.bridge_url.trim().to_string();
        if bridge_url.is_empty() {
            self.running.store(false, Ordering::SeqCst);
            return Err(ZeptoError::Config(
                "WhatsApp bridge URL is empty".to_string(),
            ));
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let (outbound_tx, outbound_rx) = mpsc::channel(64);
        self.outbound_tx = Some(outbound_tx);

        info!("Starting WhatsApp channel with bridge at {}", bridge_url);
        let running_clone = Arc::clone(&self.running);
        let bridge_token = self.config.bridge_token.clone();
        let bus = Arc::clone(&self.bus);
        let allow_from = self.config.allow_from.clone();
        let deny_by_default = self.config.deny_by_default;
        tokio::spawn(async move {
            Self::run_bridge_loop(
                bridge_url,
                bridge_token,
                bus,
                allow_from,
                deny_by_default,
                shutdown_rx,
                outbound_rx,
            )
            .await;
            running_clone.store(false, Ordering::SeqCst);
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            info!("WhatsApp channel already stopped");
            return Ok(());
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.outbound_tx = None;

        info!("WhatsApp channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ZeptoError::Channel(
                "WhatsApp channel not running".to_string(),
            ));
        }

        let tx = self.outbound_tx.as_ref().ok_or_else(|| {
            ZeptoError::Channel("WhatsApp outbound channel not initialized".to_string())
        })?;

        let to = msg.chat_id.trim().to_string();
        if to.is_empty() {
            return Err(ZeptoError::Channel(
                "WhatsApp recipient chat ID cannot be empty".to_string(),
            ));
        }

        let send_msg = BridgeSendMessage {
            msg_type: "send".to_string(),
            to,
            content: msg.content.clone(),
            reply_to: msg.reply_to.clone(),
        };

        tx.send(send_msg).await.map_err(|e| {
            ZeptoError::Channel(format!("Failed to queue WhatsApp outbound message: {}", e))
        })?;

        info!("WhatsApp: message queued for sending");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
    }
}

impl HasDependencies for WhatsAppChannel {
    fn dependencies(&self) -> Vec<Dependency> {
        if !self.config.bridge_managed {
            return vec![];
        }

        vec![Dependency {
            name: "whatsmeow-bridge".to_string(),
            kind: DepKind::Binary {
                repo: "qhkm/whatsmeow-rs".to_string(),
                asset_pattern: "whatsmeow-bridge-{os}-{arch}".to_string(),
                version: String::new(), // latest
            },
            health_check: HealthCheck::WebSocket {
                url: self.config.bridge_url.clone(),
            },
            env: std::collections::HashMap::new(),
            args: vec![],
        }]
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bus() -> Arc<MessageBus> {
        Arc::new(MessageBus::new())
    }

    fn test_config() -> WhatsAppConfig {
        WhatsAppConfig {
            enabled: true,
            bridge_url: "ws://localhost:3001".to_string(),
            allow_from: vec!["60123456789".to_string()],
            bridge_managed: true,
            ..Default::default()
        }
    }

    // -----------------------------------------------------------------------
    // 1. Channel name
    // -----------------------------------------------------------------------
    #[test]
    fn test_channel_name() {
        let channel = WhatsAppChannel::new(test_config(), test_bus());
        assert_eq!(channel.name(), "whatsapp");
    }

    // -----------------------------------------------------------------------
    // 2. Config initialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_config_initialization() {
        let config = WhatsAppConfig {
            enabled: true,
            bridge_url: "ws://bridge:3001".to_string(),
            allow_from: vec!["U1".to_string(), "U2".to_string()],
            bridge_managed: true,
            ..Default::default()
        };
        let channel = WhatsAppChannel::new(config, test_bus());

        assert!(channel.is_enabled());
        assert_eq!(channel.whatsapp_config().bridge_url, "ws://bridge:3001");
        assert_eq!(channel.whatsapp_config().allow_from.len(), 2);
        assert!(!channel.is_running());
    }

    // -----------------------------------------------------------------------
    // 3. is_allowed delegation
    // -----------------------------------------------------------------------
    #[test]
    fn test_is_allowed_delegation() {
        let channel = WhatsAppChannel::new(test_config(), test_bus());

        assert!(channel.is_allowed("60123456789"));
        assert!(!channel.is_allowed("999999999"));
    }

    #[test]
    fn test_is_allowed_empty_allowlist() {
        let config = WhatsAppConfig {
            enabled: true,
            bridge_url: "ws://localhost:3001".to_string(),
            allow_from: vec![],
            bridge_managed: true,
            ..Default::default()
        };
        let channel = WhatsAppChannel::new(config, test_bus());

        assert!(channel.is_allowed("anyone"));
        assert!(channel.is_allowed("literally_anyone"));
    }

    // -----------------------------------------------------------------------
    // 4. BridgeMessage deserialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_bridge_message_deser_message_type() {
        let json = r#"{
            "type": "message",
            "from": "60123456789",
            "chat_id": "60123456789@s.whatsapp.net",
            "content": "Hello!",
            "message_id": "wamid.xyz",
            "timestamp": 1707900000,
            "sender_name": "John"
        }"#;
        let msg: BridgeMessage = serde_json::from_str(json).expect("should parse");

        assert_eq!(msg.msg_type, "message");
        assert_eq!(msg.from.as_deref(), Some("60123456789"));
        assert_eq!(msg.chat_id.as_deref(), Some("60123456789@s.whatsapp.net"));
        assert_eq!(msg.content.as_deref(), Some("Hello!"));
        assert_eq!(msg.message_id.as_deref(), Some("wamid.xyz"));
        assert_eq!(msg.timestamp, Some(1707900000));
        assert_eq!(msg.sender_name.as_deref(), Some("John"));
    }

    #[test]
    fn test_bridge_message_deser_connected() {
        let json = r#"{"type": "connected"}"#;
        let msg: BridgeMessage = serde_json::from_str(json).expect("should parse");
        assert_eq!(msg.msg_type, "connected");
        assert!(msg.from.is_none());
    }

    #[test]
    fn test_bridge_message_deser_disconnected() {
        let json = r#"{"type": "disconnected", "reason": "session expired"}"#;
        let msg: BridgeMessage = serde_json::from_str(json).expect("should parse");
        assert_eq!(msg.msg_type, "disconnected");
        assert_eq!(msg.reason.as_deref(), Some("session expired"));
    }

    #[test]
    fn test_bridge_message_deser_qr_code() {
        let json = r#"{"type": "qr_code", "data": "2@base64data"}"#;
        let msg: BridgeMessage = serde_json::from_str(json).expect("should parse");
        assert_eq!(msg.msg_type, "qr_code");
        assert_eq!(msg.data.as_deref(), Some("2@base64data"));
    }

    #[test]
    fn test_bridge_message_deser_unknown_type() {
        let json = r#"{"type": "future_event", "extra": true}"#;
        let msg: BridgeMessage = serde_json::from_str(json).expect("should parse");
        assert_eq!(msg.msg_type, "future_event");
    }

    // -----------------------------------------------------------------------
    // 5. parse_bridge_message
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_bridge_message_valid() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("Hello!".to_string()),
            message_id: Some("wamid.xyz".to_string()),
            timestamp: Some(1707900000),
            sender_name: Some("John".to_string()),
            reason: None,
            data: None,
        };

        let inbound = WhatsAppChannel::parse_bridge_message(&msg, &[], false);
        assert!(inbound.is_some());
        let inbound = inbound.unwrap();
        assert_eq!(inbound.channel, "whatsapp");
        assert_eq!(inbound.sender_id, "60123456789");
        assert_eq!(inbound.chat_id, "60123456789@s.whatsapp.net");
        assert_eq!(inbound.content, "Hello!");
        assert_eq!(
            inbound.metadata.get("whatsapp_message_id"),
            Some(&"wamid.xyz".to_string())
        );
        assert_eq!(
            inbound.metadata.get("timestamp"),
            Some(&"1707900000".to_string())
        );
        assert_eq!(
            inbound.metadata.get("sender_name"),
            Some(&"John".to_string())
        );
    }

    #[test]
    fn test_parse_bridge_message_allowlist_allowed() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("test".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let result =
            WhatsAppChannel::parse_bridge_message(&msg, &["60123456789".to_string()], false);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_bridge_message_allowlist_denied() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("test".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let result =
            WhatsAppChannel::parse_bridge_message(&msg, &["60999999999".to_string()], false);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_bridge_message_empty_content() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("   ".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let result = WhatsAppChannel::parse_bridge_message(&msg, &[], false);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_bridge_message_missing_from() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: None,
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("Hello".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let result = WhatsAppChannel::parse_bridge_message(&msg, &[], false);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_bridge_message_missing_chat_id() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: None,
            content: Some("Hello".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let result = WhatsAppChannel::parse_bridge_message(&msg, &[], false);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_bridge_message_content_trimmed() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("  padded message  ".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let inbound = WhatsAppChannel::parse_bridge_message(&msg, &[], false).unwrap();
        assert_eq!(inbound.content, "padded message");
    }

    #[test]
    fn test_parse_bridge_message_no_optional_metadata() {
        let msg = BridgeMessage {
            msg_type: "message".to_string(),
            from: Some("60123456789".to_string()),
            chat_id: Some("60123456789@s.whatsapp.net".to_string()),
            content: Some("Hello".to_string()),
            message_id: None,
            timestamp: None,
            sender_name: None,
            reason: None,
            data: None,
        };

        let inbound = WhatsAppChannel::parse_bridge_message(&msg, &[], false).unwrap();
        assert!(inbound.metadata.get("whatsapp_message_id").is_none());
        assert!(inbound.metadata.get("timestamp").is_none());
        assert!(inbound.metadata.get("sender_name").is_none());
    }

    // -----------------------------------------------------------------------
    // 6. BridgeSendMessage serialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_bridge_send_message_with_reply() {
        let msg = BridgeSendMessage {
            msg_type: "send".to_string(),
            to: "60123456789@s.whatsapp.net".to_string(),
            content: "Reply text".to_string(),
            reply_to: Some("wamid.xyz".to_string()),
        };
        let json = serde_json::to_value(&msg).expect("should serialize");

        assert_eq!(json["type"], "send");
        assert_eq!(json["to"], "60123456789@s.whatsapp.net");
        assert_eq!(json["content"], "Reply text");
        assert_eq!(json["reply_to"], "wamid.xyz");
    }

    #[test]
    fn test_bridge_send_message_without_reply() {
        let msg = BridgeSendMessage {
            msg_type: "send".to_string(),
            to: "60123456789@s.whatsapp.net".to_string(),
            content: "Hello!".to_string(),
            reply_to: None,
        };
        let json = serde_json::to_value(&msg).expect("should serialize");

        assert_eq!(json["type"], "send");
        assert_eq!(json["to"], "60123456789@s.whatsapp.net");
        assert_eq!(json["content"], "Hello!");
        assert!(json.get("reply_to").is_none()); // skip_serializing_if
    }

    #[test]
    fn test_bridge_send_message_roundtrip() {
        let msg = BridgeSendMessage {
            msg_type: "send".to_string(),
            to: "60123456789@s.whatsapp.net".to_string(),
            content: "Test message".to_string(),
            reply_to: Some("wamid.abc".to_string()),
        };
        let json_str = serde_json::to_string(&msg).expect("should serialize");
        assert!(json_str.contains(r#""type":"send""#));
        assert!(json_str.contains(r#""reply_to":"wamid.abc""#));
    }

    // -----------------------------------------------------------------------
    // 7. Running state management
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_running_state_default() {
        let channel = WhatsAppChannel::new(test_config(), test_bus());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_start_disabled_config() {
        let config = WhatsAppConfig {
            enabled: false,
            bridge_url: "ws://localhost:3001".to_string(),
            allow_from: vec![],
            bridge_managed: true,
            ..Default::default()
        };
        let mut channel = WhatsAppChannel::new(config, test_bus());

        let result = channel.start().await;
        assert!(result.is_ok());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_start_empty_bridge_url() {
        let config = WhatsAppConfig {
            enabled: true,
            bridge_url: String::new(),
            allow_from: vec![],
            bridge_managed: true,
            ..Default::default()
        };
        let mut channel = WhatsAppChannel::new(config, test_bus());

        let result = channel.start().await;
        assert!(result.is_err());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_stop_not_running() {
        let mut channel = WhatsAppChannel::new(test_config(), test_bus());
        let result = channel.stop().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_not_running() {
        let channel = WhatsAppChannel::new(test_config(), test_bus());
        let msg = OutboundMessage::new("whatsapp", "60123456789@s.whatsapp.net", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_empty_chat_id() {
        // Start the channel so it's "running"
        let config = WhatsAppConfig {
            enabled: true,
            bridge_url: "ws://localhost:3001".to_string(),
            allow_from: vec![],
            bridge_managed: true,
            ..Default::default()
        };
        let mut channel = WhatsAppChannel::new(config, test_bus());
        // Manually set running + outbound channel (avoids actual WebSocket connect)
        channel.running.store(true, Ordering::SeqCst);
        let (tx, _rx) = mpsc::channel(64);
        channel.outbound_tx = Some(tx);

        let msg = OutboundMessage::new("whatsapp", "  ", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 8. Backoff delay calculations
    // -----------------------------------------------------------------------
    #[test]
    fn test_backoff_delay_increases_exponentially() {
        let d0 = WhatsAppChannel::backoff_delay(0);
        let d1 = WhatsAppChannel::backoff_delay(1);
        let d2 = WhatsAppChannel::backoff_delay(2);
        let d3 = WhatsAppChannel::backoff_delay(3);

        assert_eq!(d0, Duration::from_secs(2)); // 2 * 2^0 = 2
        assert_eq!(d1, Duration::from_secs(4)); // 2 * 2^1 = 4
        assert_eq!(d2, Duration::from_secs(8)); // 2 * 2^2 = 8
        assert_eq!(d3, Duration::from_secs(16)); // 2 * 2^3 = 16
    }

    #[test]
    fn test_backoff_delay_caps_at_max() {
        let d_high = WhatsAppChannel::backoff_delay(20);
        assert_eq!(d_high, Duration::from_secs(MAX_RECONNECT_DELAY_SECS));
    }

    #[test]
    fn test_backoff_delay_does_not_overflow() {
        let d = WhatsAppChannel::backoff_delay(u32::MAX);
        assert_eq!(d, Duration::from_secs(MAX_RECONNECT_DELAY_SECS));
    }

    // -----------------------------------------------------------------------
    // 9. WhatsAppConfig serde defaults
    // -----------------------------------------------------------------------
    #[test]
    fn test_whatsapp_config_deserialize_defaults() {
        let json = r#"{}"#;
        let config: WhatsAppConfig = serde_json::from_str(json).expect("should parse");

        assert!(!config.enabled);
        assert_eq!(config.bridge_url, "ws://localhost:3001");
        assert!(config.allow_from.is_empty());
    }

    #[test]
    fn test_whatsapp_config_deserialize_full() {
        let json = r#"{
            "enabled": true,
            "bridge_url": "ws://remote:9000",
            "allow_from": ["601", "602", "603"]
        }"#;
        let config: WhatsAppConfig = serde_json::from_str(json).expect("should parse");

        assert!(config.enabled);
        assert_eq!(config.bridge_url, "ws://remote:9000");
        assert_eq!(config.allow_from, vec!["601", "602", "603"]);
    }

    #[test]
    fn test_whatsapp_config_default_trait() {
        let config = WhatsAppConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.bridge_url, "ws://localhost:3001");
        assert!(config.allow_from.is_empty());
        assert!(config.bridge_managed);
    }

    // -----------------------------------------------------------------------
    // 10. bridge_managed config
    // -----------------------------------------------------------------------
    #[test]
    fn test_whatsapp_config_bridge_managed_default() {
        let json = r#"{}"#;
        let config: WhatsAppConfig = serde_json::from_str(json).expect("should parse");
        assert!(config.bridge_managed);
    }

    #[test]
    fn test_whatsapp_config_bridge_managed_false() {
        let json = r#"{"bridge_managed": false}"#;
        let config: WhatsAppConfig = serde_json::from_str(json).expect("should parse");
        assert!(!config.bridge_managed);
    }

    // -----------------------------------------------------------------------
    // 11. HasDependencies
    // -----------------------------------------------------------------------
    #[test]
    fn test_has_dependencies_managed() {
        let mut config = test_config();
        config.bridge_managed = true;
        let channel = WhatsAppChannel::new(config, test_bus());
        let deps = channel.dependencies();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "whatsmeow-bridge");
    }

    #[test]
    fn test_has_dependencies_unmanaged() {
        let mut config = test_config();
        config.bridge_managed = false;
        let channel = WhatsAppChannel::new(config, test_bus());
        let deps = channel.dependencies();
        assert!(deps.is_empty());
    }

    // -----------------------------------------------------------------------
    // 12. Bridge token auth
    // -----------------------------------------------------------------------
    #[test]
    fn test_bridge_token_default_none() {
        let config = WhatsAppConfig::default();
        assert!(config.bridge_token.is_none());
    }

    #[test]
    fn test_bridge_token_deserialize_with_token() {
        let json = r#"{"bridge_token": "secret-tok-123"}"#;
        let config: WhatsAppConfig = serde_json::from_str(json).expect("should parse");
        assert_eq!(config.bridge_token.as_deref(), Some("secret-tok-123"));
    }

    #[test]
    fn test_bridge_token_serde_roundtrip() {
        let config = WhatsAppConfig {
            bridge_token: Some("my-token".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).expect("should serialize");
        let parsed: WhatsAppConfig = serde_json::from_str(&json).expect("should parse");
        assert_eq!(parsed.bridge_token.as_deref(), Some("my-token"));
    }

    #[test]
    fn test_bridge_token_env_override() {
        // Env override is tested in config module; here just verify the field is accessible.
        let mut config = WhatsAppConfig::default();
        config.bridge_token = Some("env-token".to_string());
        assert_eq!(config.bridge_token.as_deref(), Some("env-token"));
    }
}
