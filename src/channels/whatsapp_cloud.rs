//! WhatsApp Cloud API channel implementation.
//!
//! Receives inbound messages via Meta's webhook system and sends outbound
//! replies via the WhatsApp Cloud API. Does not require the whatsmeow-rs bridge.
//!
//! # Endpoints
//!
//! - `GET /whatsapp` — Webhook verification (Meta sends challenge)
//! - `POST /whatsapp` — Inbound message/status notifications
//!
//! # Outbound
//!
//! Sends replies via `https://graph.facebook.com/v18.0/{phone_number_id}/messages`

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::WhatsAppCloudConfig;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

const WHATSAPP_API_BASE: &str = "https://graph.facebook.com/v18.0";

/// Maximum allowed request body size (1 MB).
const MAX_BODY_SIZE: usize = 1_048_576;

/// Maximum allowed header section size (8 KB).
const MAX_HEADER_SIZE: usize = 8_192;

/// WhatsApp text message character limit.
const MAX_MESSAGE_LENGTH: usize = 4096;

// --- HTTP response constants ---

const HTTP_200_OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_403_FORBIDDEN: &str =
    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

// --- Meta webhook payload types ---

/// Top-level webhook notification from Meta.
#[derive(Debug, Deserialize)]
struct WebhookNotification {
    /// Should always be "whatsapp_business_account".
    #[serde(default)]
    object: String,
    /// Array of change entries.
    #[serde(default)]
    entry: Vec<WebhookEntry>,
}

/// A single entry in the webhook notification.
#[derive(Debug, Deserialize)]
struct WebhookEntry {
    #[serde(default)]
    changes: Vec<WebhookChange>,
}

/// A change within an entry.
#[derive(Debug, Deserialize)]
struct WebhookChange {
    #[serde(default)]
    value: Option<WebhookValue>,
}

/// The value payload containing messages, contacts, and metadata.
#[derive(Debug, Deserialize)]
struct WebhookValue {
    #[serde(default)]
    messages: Vec<WebhookMessage>,
    #[serde(default)]
    contacts: Vec<WebhookContact>,
    #[serde(default)]
    #[allow(dead_code)]
    metadata: Option<WebhookMetadata>,
}

/// A single inbound message from the webhook.
#[derive(Debug, Deserialize)]
struct WebhookMessage {
    /// Sender phone number (e.g. "60123456789").
    #[serde(default)]
    from: String,
    /// WhatsApp message ID.
    #[serde(default)]
    id: String,
    /// Unix timestamp as string.
    #[serde(default)]
    timestamp: String,
    /// Message type: "text", "image", "video", etc.
    #[serde(default, rename = "type")]
    msg_type: String,
    /// Text content (only present when type = "text").
    #[serde(default)]
    text: Option<WebhookTextContent>,
}

/// Text content within a message.
#[derive(Debug, Deserialize)]
struct WebhookTextContent {
    #[serde(default)]
    body: String,
}

/// Contact info from the webhook.
#[derive(Debug, Deserialize)]
struct WebhookContact {
    #[serde(default)]
    profile: Option<WebhookProfile>,
}

/// Profile info within a contact.
#[derive(Debug, Deserialize)]
struct WebhookProfile {
    #[serde(default)]
    name: String,
}

/// Metadata about the receiving phone number.
#[derive(Debug, Deserialize)]
struct WebhookMetadata {
    #[serde(default)]
    #[allow(dead_code)]
    phone_number_id: String,
}

// --- Parsed HTTP request ---

struct ParsedHttpRequest {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: String,
}

// --- Helper functions ---

/// Parse a raw HTTP request into structured parts.
fn parse_http_request(raw: &[u8]) -> Result<ParsedHttpRequest> {
    let raw_str = std::str::from_utf8(raw)
        .map_err(|_| ZeptoError::Channel("Invalid UTF-8 in HTTP request".to_string()))?;

    let (header_section, body) = match raw_str.find("\r\n\r\n") {
        Some(pos) => (&raw_str[..pos], raw_str[pos + 4..].to_string()),
        None => (raw_str, String::new()),
    };

    let mut lines = header_section.lines();

    let request_line = lines
        .next()
        .ok_or_else(|| ZeptoError::Channel("Empty HTTP request".to_string()))?;

    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ZeptoError::Channel("Missing HTTP method".to_string()))?
        .to_uppercase();
    let full_path = parts
        .next()
        .ok_or_else(|| ZeptoError::Channel("Missing HTTP path".to_string()))?
        .to_string();

    // Split path and query string
    let (path, query) = match full_path.find('?') {
        Some(pos) => (
            full_path[..pos].to_string(),
            full_path[pos + 1..].to_string(),
        ),
        None => (full_path, String::new()),
    };

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon_pos) = line.find(':') {
            let name = line[..colon_pos].trim().to_string();
            let value = line[colon_pos + 1..].trim().to_string();
            headers.push((name, value));
        }
    }

    Ok(ParsedHttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// Extract a query parameter value by name from a query string.
fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == name {
            Some(value)
        } else {
            None
        }
    })
}

/// Extract `Content-Length` from headers.
fn content_length(headers: &[(String, String)]) -> usize {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// Find the byte offset of the `\r\n\r\n` header/body separator.
fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Extract text messages from a webhook notification, applying allowlist.
fn extract_text_messages(
    notification: &WebhookNotification,
    allowlist: &[String],
    deny_by_default: bool,
) -> Vec<InboundMessage> {
    let mut messages = Vec::new();

    for entry in &notification.entry {
        for change in &entry.changes {
            let value = match &change.value {
                Some(v) => v,
                None => continue,
            };

            // Get first contact name (if available)
            let sender_name = value
                .contacts
                .first()
                .and_then(|c| c.profile.as_ref())
                .map(|p| p.name.clone())
                .unwrap_or_default();

            for msg in &value.messages {
                // Only handle text messages
                if msg.msg_type != "text" {
                    debug!(
                        "WhatsApp Cloud: ignoring non-text message type '{}'",
                        msg.msg_type
                    );
                    continue;
                }

                let body = match &msg.text {
                    Some(t) if !t.body.trim().is_empty() => t.body.trim().to_string(),
                    _ => continue,
                };

                let from = msg.from.trim().to_string();
                if from.is_empty() {
                    continue;
                }

                // Allowlist check
                let allowed = if allowlist.is_empty() {
                    !deny_by_default
                } else {
                    allowlist.contains(&from)
                };
                if !allowed {
                    info!("WhatsApp Cloud: user {} not in allowlist, ignoring", from);
                    continue;
                }

                let mut inbound = InboundMessage::new("whatsapp_cloud", &from, &from, &body);

                if !msg.id.is_empty() {
                    inbound = inbound.with_metadata("whatsapp_message_id", &msg.id);
                }
                if !msg.timestamp.is_empty() {
                    inbound = inbound.with_metadata("timestamp", &msg.timestamp);
                }
                if !sender_name.is_empty() {
                    inbound = inbound.with_metadata("sender_name", &sender_name);
                }

                messages.push(inbound);
            }
        }
    }

    messages
}

/// Truncate a message to the WhatsApp character limit.
fn truncate_message(content: &str) -> String {
    if content.len() <= MAX_MESSAGE_LENGTH {
        content.to_string()
    } else {
        let suffix = "...(truncated)";
        let cut = MAX_MESSAGE_LENGTH - suffix.len();
        format!("{}{}", &content[..cut], suffix)
    }
}

// --- WhatsAppCloudChannel ---

/// WhatsApp Cloud API channel.
///
/// Listens for Meta webhook callbacks (inbound) and sends replies via Cloud API (outbound).
pub struct WhatsAppCloudChannel {
    config: WhatsAppCloudConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    client: Client,
    running: Arc<AtomicBool>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WhatsAppCloudChannel {
    /// Creates a new WhatsApp Cloud API channel.
    pub fn new(config: WhatsAppCloudConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "whatsapp_cloud".to_string(),
            allowlist: config.allow_from.clone(),
            deny_by_default: config.deny_by_default,
        };

        Self {
            config,
            base_config,
            bus,
            client: Client::new(),
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
        }
    }

    /// Handle webhook verification GET request.
    /// Meta sends: GET /whatsapp?hub.mode=subscribe&hub.verify_token=TOKEN&hub.challenge=CHALLENGE
    /// We must return the challenge value as plain text if the token matches.
    fn handle_verification(query: &str, verify_token: &str) -> Option<String> {
        let mode = query_param(query, "hub.mode")?;
        if mode != "subscribe" {
            return None;
        }
        let token = query_param(query, "hub.verify_token")?;
        if token != verify_token {
            return None;
        }
        let challenge = query_param(query, "hub.challenge")?;
        Some(challenge.to_string())
    }

    /// Handle a single TCP connection.
    async fn handle_connection(
        mut stream: tokio::net::TcpStream,
        config: &WhatsAppCloudConfig,
        base_config: &BaseChannelConfig,
        bus: &MessageBus,
    ) {
        // Read request
        let mut buf = vec![0u8; MAX_HEADER_SIZE + MAX_BODY_SIZE];
        let mut total_read = 0usize;

        loop {
            if total_read >= buf.len() {
                let resp = "HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                return;
            }

            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                stream.read(&mut buf[total_read..]),
            )
            .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    total_read += n;
                    if let Some(header_end) = find_header_end(&buf[..total_read]) {
                        if let Ok(req) = parse_http_request(&buf[..total_read]) {
                            let cl = content_length(&req.headers);
                            let body_received = total_read - header_end - 4;
                            if body_received >= cl {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                Ok(Err(e)) => {
                    debug!("WhatsApp Cloud: connection read error: {}", e);
                    return;
                }
                Err(_) => break,
            }
        }

        if total_read == 0 {
            return;
        }

        let request = match parse_http_request(&buf[..total_read]) {
            Ok(req) => req,
            Err(e) => {
                debug!("WhatsApp Cloud: failed to parse HTTP request: {}", e);
                let _ = stream.write_all(HTTP_200_OK.as_bytes()).await;
                return;
            }
        };

        // Strip query for path comparison
        if request.path != config.path {
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }

        match request.method.as_str() {
            "GET" => {
                // Webhook verification
                match Self::handle_verification(&request.query, &config.webhook_verify_token) {
                    Some(challenge) => {
                        info!("WhatsApp Cloud: webhook verification successful");
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            challenge.len(),
                            challenge
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                    }
                    None => {
                        warn!("WhatsApp Cloud: webhook verification failed");
                        let _ = stream.write_all(HTTP_403_FORBIDDEN.as_bytes()).await;
                    }
                }
            }
            "POST" => {
                // Always respond 200 immediately (Meta requirement)
                let _ = stream.write_all(HTTP_200_OK.as_bytes()).await;

                // Parse notification
                let notification: WebhookNotification = match serde_json::from_str(&request.body) {
                    Ok(n) => n,
                    Err(e) => {
                        debug!("WhatsApp Cloud: failed to parse webhook body: {}", e);
                        return;
                    }
                };

                if notification.object != "whatsapp_business_account" {
                    debug!("WhatsApp Cloud: ignoring non-whatsapp notification object");
                    return;
                }

                // Extract and publish text messages
                let messages = extract_text_messages(
                    &notification,
                    &base_config.allowlist,
                    base_config.deny_by_default,
                );

                for inbound in messages {
                    info!(
                        "WhatsApp Cloud: received message from {} in chat {}",
                        inbound.sender_id, inbound.chat_id
                    );
                    if let Err(e) = bus.publish_inbound(inbound).await {
                        error!("WhatsApp Cloud: failed to publish inbound message: {}", e);
                    }
                }
            }
            _ => {
                let resp = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        }
    }
}

#[async_trait]
impl Channel for WhatsAppCloudChannel {
    fn name(&self) -> &str {
        "whatsapp_cloud"
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("WhatsApp Cloud channel already running");
            return Ok(());
        }

        if !self.config.enabled {
            warn!("WhatsApp Cloud channel is disabled in configuration");
            self.running.store(false, Ordering::SeqCst);
            return Ok(());
        }

        if self.config.phone_number_id.is_empty() || self.config.access_token.is_empty() {
            self.running.store(false, Ordering::SeqCst);
            return Err(ZeptoError::Config(
                "WhatsApp Cloud API requires phone_number_id and access_token".to_string(),
            ));
        }

        let bind_addr = format!("{}:{}", self.config.bind_address, self.config.port);

        let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
            self.running.store(false, Ordering::SeqCst);
            ZeptoError::Channel(format!(
                "Failed to bind WhatsApp Cloud webhook on {}: {}",
                bind_addr, e
            ))
        })?;

        info!(
            "WhatsApp Cloud channel listening on {} (path: {})",
            bind_addr, self.config.path
        );

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        let config = self.config.clone();
        let base_config = self.base_config.clone();
        let bus = Arc::clone(&self.bus);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;

            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, addr)) => {
                                debug!("WhatsApp Cloud: accepted connection from {}", addr);
                                let cfg = config.clone();
                                let bc = base_config.clone();
                                let bus_ref = Arc::clone(&bus);
                                tokio::spawn(async move {
                                    Self::handle_connection(stream, &cfg, &bc, &bus_ref).await;
                                });
                            }
                            Err(e) => {
                                warn!("WhatsApp Cloud: failed to accept connection: {}", e);
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        info!("WhatsApp Cloud channel shutdown signal received");
                        break;
                    }
                }
            }

            running.store(false, Ordering::SeqCst);
            info!("WhatsApp Cloud channel stopped");
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            info!("WhatsApp Cloud channel already stopped");
            return Ok(());
        }

        if let Some(tx) = self.shutdown_tx.take() {
            if tx.send(()).is_err() {
                warn!("WhatsApp Cloud shutdown receiver already dropped");
            }
        }

        info!("WhatsApp Cloud channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ZeptoError::Channel(
                "WhatsApp Cloud channel not running".to_string(),
            ));
        }

        let to = msg.chat_id.trim().to_string();
        if to.is_empty() {
            return Err(ZeptoError::Channel(
                "WhatsApp Cloud recipient cannot be empty".to_string(),
            ));
        }

        let content = truncate_message(&msg.content);

        let payload = json!({
            "messaging_product": "whatsapp",
            "recipient_type": "individual",
            "to": to,
            "type": "text",
            "text": {
                "preview_url": false,
                "body": content
            }
        });

        let endpoint = format!(
            "{}/{}/messages",
            WHATSAPP_API_BASE, self.config.phone_number_id
        );
        let response = self
            .client
            .post(&endpoint)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.access_token),
            )
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ZeptoError::Channel(format!("WhatsApp Cloud API request failed: {}", e))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body: Value = response.json().await.unwrap_or_default();
            let detail = body
                .get("error")
                .and_then(Value::as_object)
                .and_then(|err| err.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Unknown API error");
            warn!("WhatsApp Cloud API error {}: {}", status, detail);
            return Err(ZeptoError::Channel(format!(
                "WhatsApp Cloud API error {}: {}",
                status, detail
            )));
        }

        info!("WhatsApp Cloud: message sent to {}", to);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
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

    fn test_config() -> WhatsAppCloudConfig {
        WhatsAppCloudConfig {
            enabled: true,
            phone_number_id: "123456".to_string(),
            access_token: "test-token".to_string(),
            webhook_verify_token: "verify-secret".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 0,
            path: "/whatsapp".to_string(),
            allow_from: vec!["60123456789".to_string()],
            deny_by_default: false,
        }
    }

    // -----------------------------------------------------------------------
    // 1. Channel name and creation
    // -----------------------------------------------------------------------

    #[test]
    fn test_channel_name() {
        let channel = WhatsAppCloudChannel::new(test_config(), test_bus());
        assert_eq!(channel.name(), "whatsapp_cloud");
    }

    #[test]
    fn test_channel_not_running_initially() {
        let channel = WhatsAppCloudChannel::new(test_config(), test_bus());
        assert!(!channel.is_running());
    }

    // -----------------------------------------------------------------------
    // 2. Webhook verification
    // -----------------------------------------------------------------------

    #[test]
    fn test_verification_valid() {
        let query = "hub.mode=subscribe&hub.verify_token=verify-secret&hub.challenge=challenge123";
        let result = WhatsAppCloudChannel::handle_verification(query, "verify-secret");
        assert_eq!(result, Some("challenge123".to_string()));
    }

    #[test]
    fn test_verification_wrong_token() {
        let query = "hub.mode=subscribe&hub.verify_token=wrong-token&hub.challenge=challenge123";
        let result = WhatsAppCloudChannel::handle_verification(query, "verify-secret");
        assert!(result.is_none());
    }

    #[test]
    fn test_verification_missing_mode() {
        let query = "hub.verify_token=verify-secret&hub.challenge=challenge123";
        let result = WhatsAppCloudChannel::handle_verification(query, "verify-secret");
        assert!(result.is_none());
    }

    #[test]
    fn test_verification_wrong_mode() {
        let query =
            "hub.mode=unsubscribe&hub.verify_token=verify-secret&hub.challenge=challenge123";
        let result = WhatsAppCloudChannel::handle_verification(query, "verify-secret");
        assert!(result.is_none());
    }

    #[test]
    fn test_verification_missing_challenge() {
        let query = "hub.mode=subscribe&hub.verify_token=verify-secret";
        let result = WhatsAppCloudChannel::handle_verification(query, "verify-secret");
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // 3. Inbound message parsing
    // -----------------------------------------------------------------------

    fn sample_webhook_json() -> &'static str {
        r#"{
            "object": "whatsapp_business_account",
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "from": "60123456789",
                            "id": "wamid.abc123",
                            "timestamp": "1707900000",
                            "type": "text",
                            "text": { "body": "Hello there!" }
                        }],
                        "contacts": [{
                            "profile": { "name": "John Doe" }
                        }],
                        "metadata": { "phone_number_id": "123456" }
                    }
                }]
            }]
        }"#
    }

    #[test]
    fn test_extract_text_messages_valid() {
        let notification: WebhookNotification =
            serde_json::from_str(sample_webhook_json()).unwrap();
        let messages = extract_text_messages(&notification, &[], false);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.channel, "whatsapp_cloud");
        assert_eq!(msg.sender_id, "60123456789");
        assert_eq!(msg.chat_id, "60123456789");
        assert_eq!(msg.content, "Hello there!");
        assert_eq!(
            msg.metadata.get("whatsapp_message_id"),
            Some(&"wamid.abc123".to_string())
        );
        assert_eq!(
            msg.metadata.get("timestamp"),
            Some(&"1707900000".to_string())
        );
        assert_eq!(
            msg.metadata.get("sender_name"),
            Some(&"John Doe".to_string())
        );
    }

    #[test]
    fn test_extract_text_messages_non_text_ignored() {
        let json = r#"{
            "object": "whatsapp_business_account",
            "entry": [{"changes": [{"value": {
                "messages": [{"from": "60123", "id": "x", "timestamp": "1", "type": "image"}],
                "contacts": [], "metadata": {"phone_number_id": "123"}
            }}]}]
        }"#;
        let notification: WebhookNotification = serde_json::from_str(json).unwrap();
        let messages = extract_text_messages(&notification, &[], false);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_extract_text_messages_empty_body_ignored() {
        let json = r#"{
            "object": "whatsapp_business_account",
            "entry": [{"changes": [{"value": {
                "messages": [{"from": "60123", "id": "x", "timestamp": "1", "type": "text", "text": {"body": "   "}}],
                "contacts": [], "metadata": {"phone_number_id": "123"}
            }}]}]
        }"#;
        let notification: WebhookNotification = serde_json::from_str(json).unwrap();
        let messages = extract_text_messages(&notification, &[], false);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_extract_text_messages_status_update_ignored() {
        let json = r#"{
            "object": "whatsapp_business_account",
            "entry": [{"changes": [{"value": {
                "messages": [],
                "contacts": [],
                "metadata": {"phone_number_id": "123"}
            }}]}]
        }"#;
        let notification: WebhookNotification = serde_json::from_str(json).unwrap();
        let messages = extract_text_messages(&notification, &[], false);
        assert!(messages.is_empty());
    }

    // -----------------------------------------------------------------------
    // 4. Allowlist
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_text_messages_allowlist_allowed() {
        let notification: WebhookNotification =
            serde_json::from_str(sample_webhook_json()).unwrap();
        let messages = extract_text_messages(&notification, &["60123456789".to_string()], false);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_extract_text_messages_allowlist_denied() {
        let notification: WebhookNotification =
            serde_json::from_str(sample_webhook_json()).unwrap();
        let messages = extract_text_messages(&notification, &["60999999999".to_string()], false);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_extract_text_messages_deny_by_default() {
        let notification: WebhookNotification =
            serde_json::from_str(sample_webhook_json()).unwrap();
        let messages = extract_text_messages(&notification, &[], true);
        assert!(messages.is_empty());
    }

    // -----------------------------------------------------------------------
    // 5. is_allowed delegation
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_allowed_in_list() {
        let channel = WhatsAppCloudChannel::new(test_config(), test_bus());
        assert!(channel.is_allowed("60123456789"));
    }

    #[test]
    fn test_is_allowed_not_in_list() {
        let channel = WhatsAppCloudChannel::new(test_config(), test_bus());
        assert!(!channel.is_allowed("60999999999"));
    }

    // -----------------------------------------------------------------------
    // 6. Outbound truncation
    // -----------------------------------------------------------------------

    #[test]
    fn test_truncate_message_short() {
        let msg = "Hello!";
        assert_eq!(truncate_message(msg), "Hello!");
    }

    #[test]
    fn test_truncate_message_at_limit() {
        let msg = "a".repeat(MAX_MESSAGE_LENGTH);
        assert_eq!(truncate_message(&msg).len(), MAX_MESSAGE_LENGTH);
    }

    #[test]
    fn test_truncate_message_over_limit() {
        let msg = "a".repeat(MAX_MESSAGE_LENGTH + 100);
        let result = truncate_message(&msg);
        assert!(result.len() <= MAX_MESSAGE_LENGTH);
        assert!(result.ends_with("...(truncated)"));
    }

    // -----------------------------------------------------------------------
    // 7. Query param extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_query_param_found() {
        assert_eq!(
            query_param("hub.mode=subscribe&hub.challenge=abc", "hub.challenge"),
            Some("abc")
        );
    }

    #[test]
    fn test_query_param_not_found() {
        assert_eq!(query_param("hub.mode=subscribe", "hub.challenge"), None);
    }

    // -----------------------------------------------------------------------
    // 8. Channel lifecycle
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_send_when_not_running() {
        let channel = WhatsAppCloudChannel::new(test_config(), test_bus());
        let msg = OutboundMessage::new("whatsapp_cloud", "60123456789", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stop_when_not_running() {
        let mut channel = WhatsAppCloudChannel::new(test_config(), test_bus());
        let result = channel.stop().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_start_disabled_config() {
        let mut config = test_config();
        config.enabled = false;
        let mut channel = WhatsAppCloudChannel::new(config, test_bus());
        let result = channel.start().await;
        assert!(result.is_ok());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_start_missing_credentials() {
        let mut config = test_config();
        config.phone_number_id = String::new();
        let mut channel = WhatsAppCloudChannel::new(config, test_bus());
        let result = channel.start().await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 9. HTTP request parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_http_request_get_with_query() {
        let raw = b"GET /whatsapp?hub.mode=subscribe&hub.challenge=abc HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req = parse_http_request(raw).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/whatsapp");
        assert_eq!(req.query, "hub.mode=subscribe&hub.challenge=abc");
    }

    #[test]
    fn test_parse_http_request_post() {
        let raw = b"POST /whatsapp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}";
        let req = parse_http_request(raw).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/whatsapp");
        assert!(req.query.is_empty());
        assert_eq!(req.body, "{}");
    }

    // -----------------------------------------------------------------------
    // 10. End-to-end: start, verify, POST, verify bus message
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_end_to_end_webhook_verification() {
        let bus = test_bus();
        let temp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let mut config = test_config();
        config.port = port;
        let mut channel = WhatsAppCloudChannel::new(config, Arc::clone(&bus));
        channel.start().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send verification GET
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let request = "GET /whatsapp?hub.mode=subscribe&hub.verify_token=verify-secret&hub.challenge=test_challenge_123 HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();

        let response = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("test_challenge_123"));

        channel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_end_to_end_inbound_message() {
        let bus = test_bus();
        let temp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let mut config = test_config();
        config.port = port;
        config.allow_from = vec![]; // Allow all
        let mut channel = WhatsAppCloudChannel::new(config, Arc::clone(&bus));
        channel.start().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // POST inbound message
        let body = sample_webhook_json();
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let request = format!(
            "POST /whatsapp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        // Read response
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();

        let response = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));

        // Verify message on bus
        let received =
            tokio::time::timeout(std::time::Duration::from_secs(2), bus.consume_inbound())
                .await
                .unwrap()
                .unwrap();

        assert_eq!(received.channel, "whatsapp_cloud");
        assert_eq!(received.sender_id, "60123456789");
        assert_eq!(received.content, "Hello there!");

        channel.stop().await.unwrap();
    }
}
