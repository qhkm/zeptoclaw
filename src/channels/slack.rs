//! Slack channel implementation.
//!
//! Supports:
//! - outbound messaging via Slack Web API (`chat.postMessage`)
//! - inbound messaging via Slack Socket Mode (`apps.connections.open`)

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::SlackConfig;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

const SLACK_CHAT_POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";
const SLACK_SOCKET_OPEN_URL: &str = "https://slack.com/api/apps.connections.open";
const SLACK_RECONNECT_DELAY_SECS: u64 = 2;

#[derive(Debug, Deserialize)]
struct SlackSocketOpenResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SlackSocketEnvelope {
    #[serde(default)]
    envelope_id: Option<String>,
    #[serde(rename = "type")]
    envelope_type: String,
    #[serde(default)]
    payload: Option<SlackSocketPayload>,
}

#[derive(Debug, Deserialize)]
struct SlackSocketPayload {
    #[serde(default)]
    event: Option<SlackEvent>,
}

#[derive(Debug, Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    bot_id: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
}

struct ParsedSocketMessage {
    ack_message: Option<String>,
    inbound_message: Option<InboundMessage>,
}

/// Slack channel implementation backed by Slack Web API and Socket Mode.
pub struct SlackChannel {
    config: SlackConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    client: reqwest::Client,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl SlackChannel {
    /// Creates a new Slack channel.
    pub fn new(config: SlackConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "slack".to_string(),
            allowlist: config.allow_from.clone(),
            deny_by_default: config.deny_by_default,
        };

        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            client: reqwest::Client::new(),
            shutdown_tx: None,
        }
    }

    /// Returns a reference to the Slack configuration.
    pub fn slack_config(&self) -> &SlackConfig {
        &self.config
    }

    /// Returns whether the channel is enabled in configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    fn build_payload(msg: &OutboundMessage) -> Result<Value> {
        let channel = msg.chat_id.trim();
        if channel.is_empty() {
            return Err(ZeptoError::Channel(
                "Slack channel ID cannot be empty".to_string(),
            ));
        }

        let mut payload = json!({
            "channel": channel,
            "text": msg.content,
        });

        if let Some(ref reply_to) = msg.reply_to {
            if let Some(map) = payload.as_object_mut() {
                map.insert("thread_ts".to_string(), Value::String(reply_to.clone()));
            }
        }

        Ok(payload)
    }

    async fn open_socket_mode_url(client: &reqwest::Client, app_token: &str) -> Result<String> {
        let response = client
            .post(SLACK_SOCKET_OPEN_URL)
            .bearer_auth(app_token)
            .send()
            .await
            .map_err(|e| {
                ZeptoError::Channel(format!(
                    "Failed to open Slack Socket Mode connection: {}",
                    e
                ))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ZeptoError::Channel(format!("Failed to read Slack Socket Mode response: {}", e))
        })?;

        if !status.is_success() {
            return Err(ZeptoError::Channel(format!(
                "Slack Socket Mode HTTP {}: {}",
                status, body
            )));
        }

        let parsed: SlackSocketOpenResponse = serde_json::from_str(&body).map_err(|e| {
            ZeptoError::Channel(format!("Invalid Slack Socket Mode open response: {}", e))
        })?;

        if !parsed.ok {
            return Err(ZeptoError::Channel(format!(
                "Slack Socket Mode open failed: {}",
                parsed.error.unwrap_or_else(|| "unknown_error".to_string())
            )));
        }

        parsed.url.filter(|u| !u.trim().is_empty()).ok_or_else(|| {
            ZeptoError::Channel("Slack Socket Mode response missing URL".to_string())
        })
    }

    fn parse_socket_message(
        raw: &str,
        allowlist: &[String],
        deny_by_default: bool,
    ) -> Result<ParsedSocketMessage> {
        let envelope: SlackSocketEnvelope = serde_json::from_str(raw)
            .map_err(|e| ZeptoError::Channel(format!("Invalid Slack socket payload: {}", e)))?;

        let ack_message = envelope
            .envelope_id
            .as_deref()
            .map(|envelope_id| json!({ "envelope_id": envelope_id }).to_string());
        let inbound_message = Self::extract_inbound_message(&envelope, allowlist, deny_by_default);

        Ok(ParsedSocketMessage {
            ack_message,
            inbound_message,
        })
    }

    fn extract_inbound_message(
        envelope: &SlackSocketEnvelope,
        allowlist: &[String],
        deny_by_default: bool,
    ) -> Option<InboundMessage> {
        if envelope.envelope_type != "events_api" {
            return None;
        }

        let payload = envelope.payload.as_ref()?;
        let event = payload.event.as_ref()?;

        if event.event_type != "message" {
            return None;
        }
        if event.subtype.is_some() || event.bot_id.is_some() {
            return None;
        }

        let sender_id = event.user.as_deref()?.trim().to_string();
        let chat_id = event.channel.as_deref()?.trim().to_string();
        let content = event.text.as_deref()?.trim().to_string();
        if sender_id.is_empty() || chat_id.is_empty() || content.is_empty() {
            return None;
        }

        let allowed = if allowlist.is_empty() {
            !deny_by_default
        } else {
            allowlist.contains(&sender_id)
        };
        if !allowed {
            info!(
                "Slack: user {} not in allowlist, ignoring inbound message",
                sender_id
            );
            return None;
        }

        let mut inbound = InboundMessage::new("slack", &sender_id, &chat_id, &content);
        if let Some(ts) = event.ts.as_deref() {
            if !ts.trim().is_empty() {
                inbound = inbound.with_metadata("slack_ts", ts);
            }
        }
        if let Some(thread_ts) = event.thread_ts.as_deref() {
            if !thread_ts.trim().is_empty() {
                inbound = inbound.with_metadata("slack_thread_ts", thread_ts);
            }
        }

        Some(inbound)
    }

    async fn wait_for_reconnect_or_shutdown(shutdown_rx: &mut mpsc::Receiver<()>) -> bool {
        tokio::select! {
            _ = shutdown_rx.recv() => true,
            _ = tokio::time::sleep(Duration::from_secs(SLACK_RECONNECT_DELAY_SECS)) => false,
        }
    }

    async fn run_socket_mode_loop(
        client: reqwest::Client,
        app_token: String,
        bus: Arc<MessageBus>,
        allowlist: Vec<String>,
        deny_by_default: bool,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) {
        loop {
            let socket_url = tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Slack Socket Mode shutdown requested");
                    return;
                }
                opened = Self::open_socket_mode_url(&client, &app_token) => {
                    match opened {
                        Ok(url) => url,
                        Err(e) => {
                            warn!("Slack Socket Mode open failed: {}", e);
                            if Self::wait_for_reconnect_or_shutdown(&mut shutdown_rx).await {
                                return;
                            }
                            continue;
                        }
                    }
                }
            };

            let ws_stream = tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Slack Socket Mode shutdown requested");
                    return;
                }
                connected = connect_async(&socket_url) => {
                    match connected {
                        Ok((stream, _)) => stream,
                        Err(e) => {
                            warn!("Failed to connect Slack Socket Mode websocket: {}", e);
                            if Self::wait_for_reconnect_or_shutdown(&mut shutdown_rx).await {
                                return;
                            }
                            continue;
                        }
                    }
                }
            };

            info!("Slack Socket Mode connected");
            let (mut ws_writer, mut ws_reader) = ws_stream.split();

            loop {
                let next = tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Slack Socket Mode shutdown requested");
                        return;
                    }
                    message = ws_reader.next() => message,
                };

                match next {
                    Some(Ok(WsMessage::Text(raw))) => {
                        match Self::parse_socket_message(&raw, &allowlist, deny_by_default) {
                            Ok(parsed) => {
                                if let Some(ack_message) = parsed.ack_message {
                                    if let Err(e) =
                                        ws_writer.send(WsMessage::Text(ack_message)).await
                                    {
                                        warn!("Slack Socket Mode ack send failed: {}", e);
                                        break;
                                    }
                                }

                                if let Some(inbound) = parsed.inbound_message {
                                    if let Err(e) = bus.publish_inbound(inbound).await {
                                        error!("Failed to publish Slack inbound message: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Ignoring Slack socket payload: {}", e);
                            }
                        }
                    }
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if let Err(e) = ws_writer.send(WsMessage::Pong(payload)).await {
                            warn!("Slack Socket Mode pong send failed: {}", e);
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        info!("Slack Socket Mode closed by server: {:?}", frame);
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        warn!("Slack Socket Mode stream error: {}", e);
                        break;
                    }
                    None => {
                        warn!("Slack Socket Mode stream ended");
                        break;
                    }
                }
            }

            if Self::wait_for_reconnect_or_shutdown(&mut shutdown_rx).await {
                return;
            }
            info!("Reconnecting Slack Socket Mode");
        }
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("Slack channel already running");
            return Ok(());
        }

        if !self.config.enabled {
            warn!("Slack channel is disabled in configuration");
            self.running.store(false, Ordering::SeqCst);
            return Ok(());
        }

        if self.config.bot_token.trim().is_empty() {
            self.running.store(false, Ordering::SeqCst);
            return Err(ZeptoError::Config("Slack bot token is empty".to_string()));
        }

        let app_token = self.config.app_token.trim().to_string();
        if app_token.is_empty() {
            info!("Starting Slack channel (outbound only, app_token not configured)");
            return Ok(());
        }

        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        info!("Starting Slack channel with Socket Mode inbound");
        let running_clone = Arc::clone(&self.running);
        let client = self.client.clone();
        let bus = Arc::clone(&self.bus);
        let allow_from = self.config.allow_from.clone();
        let deny_by_default = self.config.deny_by_default;
        tokio::spawn(async move {
            Self::run_socket_mode_loop(
                client,
                app_token,
                bus,
                allow_from,
                deny_by_default,
                shutdown_rx,
            )
            .await;
            running_clone.store(false, Ordering::SeqCst);
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            info!("Slack channel already stopped");
            return Ok(());
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        info!("Slack channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ZeptoError::Channel("Slack channel not running".to_string()));
        }

        if self.config.bot_token.trim().is_empty() {
            return Err(ZeptoError::Config("Slack bot token is empty".to_string()));
        }

        let payload = Self::build_payload(&msg)?;

        let response = self
            .client
            .post(SLACK_CHAT_POST_MESSAGE_URL)
            .bearer_auth(&self.config.bot_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to call Slack API: {}", e)))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ZeptoError::Channel(format!("Failed to read Slack API response: {}", e))
        })?;

        if !status.is_success() {
            return Err(ZeptoError::Channel(format!(
                "Slack API returned HTTP {}: {}",
                status, body
            )));
        }

        let body_json: Value = serde_json::from_str(&body)
            .map_err(|e| ZeptoError::Channel(format!("Invalid Slack API response JSON: {}", e)))?;

        if !body_json
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let api_error = body_json
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error");
            return Err(ZeptoError::Channel(format!(
                "Slack API returned error: {}",
                api_error
            )));
        }

        info!("Slack: Message sent successfully");
        Ok(())
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

    fn test_bus() -> Arc<MessageBus> {
        Arc::new(MessageBus::new())
    }

    #[test]
    fn test_slack_channel_creation() {
        let config = SlackConfig {
            enabled: true,
            bot_token: "xoxb-test-token".to_string(),
            app_token: "xapp-test-token".to_string(),
            allow_from: vec!["U123".to_string()],
            ..Default::default()
        };
        let channel = SlackChannel::new(config, test_bus());

        assert_eq!(channel.name(), "slack");
        assert!(!channel.is_running());
        assert!(channel.is_allowed("U123"));
        assert!(!channel.is_allowed("U999"));
    }

    #[test]
    fn test_slack_empty_allowlist() {
        let config = SlackConfig {
            enabled: true,
            bot_token: "xoxb-test-token".to_string(),
            app_token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let channel = SlackChannel::new(config, test_bus());

        assert!(channel.is_allowed("anyone"));
    }

    #[test]
    fn test_slack_config_access() {
        let config = SlackConfig {
            enabled: true,
            bot_token: "xoxb-my-token".to_string(),
            app_token: "xapp-token".to_string(),
            allow_from: vec!["UADMIN".to_string()],
            ..Default::default()
        };
        let channel = SlackChannel::new(config, test_bus());

        assert!(channel.is_enabled());
        assert_eq!(channel.slack_config().bot_token, "xoxb-my-token");
        assert_eq!(channel.slack_config().allow_from, vec!["UADMIN"]);
    }

    #[tokio::test]
    async fn test_slack_start_without_token() {
        let config = SlackConfig {
            enabled: true,
            bot_token: String::new(),
            app_token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let mut channel = SlackChannel::new(config, test_bus());

        let result = channel.start().await;
        assert!(result.is_err());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_slack_start_disabled() {
        let config = SlackConfig {
            enabled: false,
            bot_token: "xoxb-test-token".to_string(),
            app_token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let mut channel = SlackChannel::new(config, test_bus());

        let result = channel.start().await;
        assert!(result.is_ok());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_slack_stop_not_running() {
        let config = SlackConfig {
            enabled: true,
            bot_token: "xoxb-test-token".to_string(),
            app_token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let mut channel = SlackChannel::new(config, test_bus());

        let result = channel.stop().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_slack_send_not_running() {
        let config = SlackConfig {
            enabled: true,
            bot_token: "xoxb-test-token".to_string(),
            app_token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let channel = SlackChannel::new(config, test_bus());

        let msg = OutboundMessage::new("slack", "C123456", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_slack_send_empty_chat_id() {
        let config = SlackConfig {
            enabled: true,
            bot_token: "xoxb-test-token".to_string(),
            app_token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let channel = SlackChannel::new(config, test_bus());
        channel.running.store(true, Ordering::SeqCst);

        let msg = OutboundMessage::new("slack", "", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_slack_payload_with_reply() {
        let msg = OutboundMessage::new("slack", "C123", "hello").with_reply("173401.000200");
        let payload = SlackChannel::build_payload(&msg).expect("payload should build");

        assert_eq!(payload["channel"], "C123");
        assert_eq!(payload["text"], "hello");
        assert_eq!(payload["thread_ts"], "173401.000200");
    }

    #[test]
    fn test_parse_socket_message_extracts_inbound_and_ack() {
        let raw = r#"{
            "envelope_id":"envelope-123",
            "type":"events_api",
            "payload":{
                "event":{
                    "type":"message",
                    "user":"U123",
                    "channel":"C999",
                    "text":"hello from slack",
                    "ts":"173401.000200",
                    "thread_ts":"173401.000100"
                }
            }
        }"#;

        let parsed =
            SlackChannel::parse_socket_message(raw, &[], false).expect("parse should succeed");
        assert_eq!(
            parsed.ack_message,
            Some(r#"{"envelope_id":"envelope-123"}"#.to_string())
        );
        let inbound = parsed.inbound_message.expect("inbound expected");
        assert_eq!(inbound.channel, "slack");
        assert_eq!(inbound.sender_id, "U123");
        assert_eq!(inbound.chat_id, "C999");
        assert_eq!(inbound.content, "hello from slack");
        assert_eq!(
            inbound.metadata.get("slack_ts"),
            Some(&"173401.000200".to_string())
        );
        assert_eq!(
            inbound.metadata.get("slack_thread_ts"),
            Some(&"173401.000100".to_string())
        );
    }

    #[test]
    fn test_parse_socket_message_ignores_non_message_event() {
        let raw = r#"{
            "envelope_id":"envelope-456",
            "type":"events_api",
            "payload":{"event":{"type":"reaction_added","user":"U123"}}
        }"#;

        let parsed =
            SlackChannel::parse_socket_message(raw, &[], false).expect("parse should succeed");
        assert!(parsed.ack_message.is_some());
        assert!(parsed.inbound_message.is_none());
    }

    #[test]
    fn test_parse_socket_message_ignores_disallowed_user() {
        let raw = r#"{
            "envelope_id":"envelope-789",
            "type":"events_api",
            "payload":{
                "event":{"type":"message","user":"U999","channel":"C123","text":"blocked"}
            }
        }"#;

        let parsed = SlackChannel::parse_socket_message(raw, &["U123".to_string()], false)
            .expect("parse should succeed");
        assert!(parsed.ack_message.is_some());
        assert!(parsed.inbound_message.is_none());
    }

    #[test]
    fn test_parse_socket_message_ignores_bot_or_subtype_messages() {
        let bot_message = r#"{
            "envelope_id":"e1",
            "type":"events_api",
            "payload":{"event":{"type":"message","bot_id":"B123","channel":"C1","text":"bot"}}
        }"#;
        let subtype_message = r#"{
            "envelope_id":"e2",
            "type":"events_api",
            "payload":{"event":{"type":"message","subtype":"message_changed","channel":"C1","text":"edit"}}
        }"#;

        let bot_parsed = SlackChannel::parse_socket_message(bot_message, &[], false)
            .expect("parse should succeed");
        let subtype_parsed = SlackChannel::parse_socket_message(subtype_message, &[], false)
            .expect("parse should succeed");

        assert!(bot_parsed.inbound_message.is_none());
        assert!(subtype_parsed.inbound_message.is_none());
    }
}
