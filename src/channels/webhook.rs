//! Webhook Inbound Channel Implementation
//!
//! This module provides a generic HTTP webhook channel for ZeptoClaw. External
//! services can POST JSON payloads to a configurable endpoint and have them
//! published to the message bus as inbound messages.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────┐         ┌──────────────────┐
//! │  External Service│ ──POST─>│  WebhookChannel  │
//! │  (any HTTP       │         │  (TcpListener)   │
//! │   client)        │         └────────┬─────────┘
//! └──────────────────┘                  │
//!                                       │ InboundMessage
//!                                       ▼
//!                              ┌──────────────────┐
//!                              │    MessageBus    │
//!                              └──────────────────┘
//! ```
//!
//! # Request Format
//!
//! ```json
//! POST /webhook HTTP/1.1
//! Content-Type: application/json
//! Authorization: Bearer <optional-token>
//!
//! {
//!     "message": "Hello, ZeptoClaw!",
//!     "sender": "external-service",
//!     "chat_id": "webhook-chat-123"
//! }
//! ```
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use zeptoclaw::bus::MessageBus;
//! use zeptoclaw::channels::webhook::{WebhookChannel, WebhookChannelConfig};
//! use zeptoclaw::channels::BaseChannelConfig;
//!
//! let config = WebhookChannelConfig::default();
//! let base_config = BaseChannelConfig::new("webhook");
//! let bus = Arc::new(MessageBus::new());
//! let channel = WebhookChannel::new(config, base_config, bus);
//! ```

use async_trait::async_trait;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

/// Constant-time string comparison to prevent timing side-channel attacks.
///
/// Always compares the full length of both strings regardless of where
/// they first differ. Returns `false` immediately if lengths differ
/// (length is not considered secret for Bearer tokens).
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

/// Maximum allowed request body size (1 MB).
const MAX_BODY_SIZE: usize = 1_048_576;

/// Maximum allowed header section size (8 KB).
const MAX_HEADER_SIZE: usize = 8_192;

// --- HTTP response constants ---

const HTTP_200_OK: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}";
const HTTP_400_BAD_REQUEST: &str =
    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n";
const HTTP_401_UNAUTHORIZED: &str = "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"error\":\"unauthorized\"}";
const HTTP_404_NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 23\r\nConnection: close\r\n\r\n{\"error\":\"not found\"}";
const HTTP_405_METHOD_NOT_ALLOWED: &str = "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: application/json\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{\"error\":\"method not allowed\"}";
const HTTP_413_PAYLOAD_TOO_LARGE: &str = "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\nContent-Length: 31\r\nConnection: close\r\n\r\n{\"error\":\"payload too large\"}";

/// Runtime configuration for the webhook HTTP server.
///
/// This is the internal runtime configuration, not the serde config struct
/// that lives in `config/types.rs`.
#[derive(Debug, Clone)]
pub struct WebhookChannelConfig {
    /// Address to bind the HTTP server to.
    pub bind_address: String,
    /// Port to listen on.
    pub port: u16,
    /// URL path to accept webhook requests on.
    pub path: String,
    /// Optional Bearer token for request authentication.
    /// When set, all requests must include a matching `Authorization: Bearer <token>` header.
    pub auth_token: Option<String>,
}

impl Default for WebhookChannelConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            port: 9876,
            path: "/webhook".to_string(),
            auth_token: None,
        }
    }
}

/// JSON body expected from webhook POST requests.
#[derive(Debug, Deserialize)]
struct WebhookPayload {
    /// The message content.
    message: String,
    /// Identifier of the sender.
    sender: String,
    /// Chat/conversation identifier for session routing.
    chat_id: String,
}

/// Parsed representation of an incoming HTTP request (first line + headers + body).
struct ParsedHttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Generic HTTP webhook channel for ZeptoClaw.
///
/// Accepts POST requests on a configurable path, validates an optional Bearer
/// token, parses the JSON body, and publishes an `InboundMessage` to the
/// message bus.
///
/// The channel is primarily inbound-only. The `send()` method logs the
/// outbound message but does not deliver it anywhere because there is no
/// persistent connection back to the caller.
pub struct WebhookChannel {
    /// Webhook-specific configuration (bind address, port, path, auth).
    config: WebhookChannelConfig,
    /// Base channel configuration (name, allowlist).
    base_config: BaseChannelConfig,
    /// Reference to the message bus for publishing inbound messages.
    bus: Arc<MessageBus>,
    /// Atomic flag indicating if the channel is currently running.
    running: Arc<AtomicBool>,
    /// One-shot sender to signal the TCP listener loop to shut down.
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WebhookChannel {
    /// Creates a new webhook channel.
    ///
    /// # Arguments
    ///
    /// * `config` - Webhook-specific runtime configuration
    /// * `base_config` - Base channel configuration (name, allowlist)
    /// * `bus` - Reference to the message bus for publishing messages
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use zeptoclaw::bus::MessageBus;
    /// use zeptoclaw::channels::webhook::{WebhookChannel, WebhookChannelConfig};
    /// use zeptoclaw::channels::BaseChannelConfig;
    ///
    /// let config = WebhookChannelConfig::default();
    /// let base_config = BaseChannelConfig::new("webhook");
    /// let bus = Arc::new(MessageBus::new());
    /// let channel = WebhookChannel::new(config, base_config, bus);
    ///
    /// assert_eq!(channel.name(), "webhook");
    /// assert!(!channel.is_running());
    /// ```
    pub fn new(
        config: WebhookChannelConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
    ) -> Self {
        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
        }
    }

    /// Returns a reference to the webhook configuration.
    pub fn webhook_config(&self) -> &WebhookChannelConfig {
        &self.config
    }

    // --- Internal helpers ---

    /// Validate the `Authorization` header against the configured token.
    ///
    /// Returns `true` if:
    /// - No auth token is configured (open access), OR
    /// - The request carries a matching `Bearer <token>` header.
    ///
    /// Uses constant-time comparison to prevent timing side-channel attacks
    /// that could leak the token value byte-by-byte.
    fn validate_auth(headers: &[(String, String)], required_token: &Option<String>) -> bool {
        let token = match required_token {
            Some(t) => t,
            None => return true, // No auth required
        };

        let expected = format!("Bearer {}", token);

        headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && constant_time_eq(value.trim(), &expected)
        })
    }

    /// Parse a raw HTTP request from bytes into structured parts.
    ///
    /// This is intentionally minimal — it only handles what the webhook needs:
    /// method, path, headers, and a UTF-8 body.
    fn parse_http_request(raw: &[u8]) -> Result<ParsedHttpRequest> {
        let raw_str = std::str::from_utf8(raw)
            .map_err(|_| ZeptoError::Channel("Invalid UTF-8 in HTTP request".to_string()))?;

        // Split headers from body at the blank line
        let (header_section, body) = match raw_str.find("\r\n\r\n") {
            Some(pos) => (&raw_str[..pos], raw_str[pos + 4..].to_string()),
            None => (raw_str, String::new()),
        };

        let mut lines = header_section.lines();

        // Parse request line: METHOD PATH HTTP/x.x
        let request_line = lines
            .next()
            .ok_or_else(|| ZeptoError::Channel("Empty HTTP request".to_string()))?;

        let mut parts = request_line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| ZeptoError::Channel("Missing HTTP method".to_string()))?
            .to_uppercase();
        let path = parts
            .next()
            .ok_or_else(|| ZeptoError::Channel("Missing HTTP path".to_string()))?
            .to_string();

        // Parse headers
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
            headers,
            body,
        })
    }

    /// Extract the `Content-Length` value from headers, defaulting to 0.
    fn content_length(headers: &[(String, String)]) -> usize {
        headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0)
    }

    /// Handle a single TCP connection: read the request, validate, parse, publish.
    async fn handle_connection(
        mut stream: tokio::net::TcpStream,
        config: &WebhookChannelConfig,
        base_config: &BaseChannelConfig,
        bus: &MessageBus,
    ) {
        // Read request data with size limits
        let mut buf = vec![0u8; MAX_HEADER_SIZE + MAX_BODY_SIZE];
        let mut total_read = 0usize;

        // Read in a loop until we have the full request or hit limits.
        // For simplicity we read until the connection is idle briefly.
        loop {
            if total_read >= buf.len() {
                let _ = stream
                    .write_all(HTTP_413_PAYLOAD_TOO_LARGE.as_bytes())
                    .await;
                return;
            }

            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                stream.read(&mut buf[total_read..]),
            )
            .await
            {
                Ok(Ok(0)) => break, // EOF
                Ok(Ok(n)) => {
                    total_read += n;
                    // Check if we have the complete request (headers + body)
                    let data = &buf[..total_read];
                    if let Some(header_end) = Self::find_header_end(data) {
                        // Parse Content-Length to know how much body to expect
                        if let Ok(req) = Self::parse_http_request(&buf[..total_read]) {
                            let cl = Self::content_length(&req.headers);
                            let body_received = total_read - header_end - 4; // 4 for \r\n\r\n
                            if body_received >= cl {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                Ok(Err(e)) => {
                    debug!("Webhook: connection read error: {}", e);
                    return;
                }
                Err(_) => break, // Timeout — process what we have
            }
        }

        if total_read == 0 {
            return;
        }

        let request = match Self::parse_http_request(&buf[..total_read]) {
            Ok(req) => req,
            Err(e) => {
                debug!("Webhook: failed to parse HTTP request: {}", e);
                let body = format!("{{\"error\":\"{}\"}}", "malformed request");
                let response = format!(
                    "{}Content-Length: {}\r\n\r\n{}",
                    HTTP_400_BAD_REQUEST,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return;
            }
        };

        // Route: only accept POST to the configured path
        if request.method != "POST" {
            let _ = stream
                .write_all(HTTP_405_METHOD_NOT_ALLOWED.as_bytes())
                .await;
            return;
        }

        // Strip query string for path comparison
        let request_path = request.path.split('?').next().unwrap_or(&request.path);

        if request_path != config.path {
            let _ = stream.write_all(HTTP_404_NOT_FOUND.as_bytes()).await;
            return;
        }

        // Authenticate
        if !Self::validate_auth(&request.headers, &config.auth_token) {
            let _ = stream.write_all(HTTP_401_UNAUTHORIZED.as_bytes()).await;
            return;
        }

        // Parse JSON body
        let payload: WebhookPayload = match serde_json::from_str(&request.body) {
            Ok(p) => p,
            Err(e) => {
                debug!("Webhook: invalid JSON body: {}", e);
                let body = format!("{{\"error\":\"invalid JSON: {}\"}}", e);
                let response = format!(
                    "{}Content-Length: {}\r\n\r\n{}",
                    HTTP_400_BAD_REQUEST,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return;
            }
        };

        // Validate required fields are non-empty
        if payload.message.trim().is_empty()
            || payload.sender.trim().is_empty()
            || payload.chat_id.trim().is_empty()
        {
            let body = r#"{"error":"message, sender, and chat_id must be non-empty"}"#;
            let response = format!(
                "{}Content-Length: {}\r\n\r\n{}",
                HTTP_400_BAD_REQUEST,
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            return;
        }

        // Check allowlist
        if !base_config.is_allowed(&payload.sender) {
            info!(
                "Webhook: sender {} not in allowlist, rejecting",
                payload.sender
            );
            let _ = stream.write_all(HTTP_401_UNAUTHORIZED.as_bytes()).await;
            return;
        }

        // Build and publish inbound message
        let inbound = InboundMessage::new(
            "webhook",
            payload.sender.trim(),
            payload.chat_id.trim(),
            payload.message.trim(),
        );

        if let Err(e) = bus.publish_inbound(inbound).await {
            error!("Webhook: failed to publish inbound message: {}", e);
            let body = r#"{"error":"internal server error"}"#;
            let response = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            return;
        }

        info!(
            "Webhook: received message from {} in chat {}",
            payload.sender, payload.chat_id
        );
        let _ = stream.write_all(HTTP_200_OK.as_bytes()).await;
    }

    /// Find the byte offset of the `\r\n\r\n` header/body separator.
    fn find_header_end(data: &[u8]) -> Option<usize> {
        data.windows(4).position(|w| w == b"\r\n\r\n")
    }
}

#[async_trait]
impl Channel for WebhookChannel {
    /// Returns the channel name ("webhook").
    fn name(&self) -> &str {
        "webhook"
    }

    /// Starts the webhook HTTP server.
    ///
    /// Binds a `TcpListener` on the configured address and port, then spawns
    /// a background tokio task that accepts connections until a shutdown signal
    /// is received.
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP listener fails to bind (e.g., port already
    /// in use, permission denied).
    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("Webhook channel already running");
            return Ok(());
        }

        let bind_addr = format!("{}:{}", self.config.bind_address, self.config.port);

        let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
            self.running.store(false, Ordering::SeqCst);
            ZeptoError::Channel(format!(
                "Failed to bind webhook listener on {}: {}",
                bind_addr, e
            ))
        })?;

        info!(
            "Webhook channel listening on {} (path: {})",
            bind_addr, self.config.path
        );

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        let config = self.config.clone();
        let base_config = self.base_config.clone();
        let bus = Arc::clone(&self.bus);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            // Convert the oneshot into a future we can select against
            let mut shutdown_rx = shutdown_rx;

            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, addr)) => {
                                debug!("Webhook: accepted connection from {}", addr);
                                let cfg = config.clone();
                                let bc = base_config.clone();
                                let bus_ref = Arc::clone(&bus);
                                tokio::spawn(async move {
                                    Self::handle_connection(stream, &cfg, &bc, &bus_ref).await;
                                });
                            }
                            Err(e) => {
                                warn!("Webhook: failed to accept connection: {}", e);
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        info!("Webhook channel shutdown signal received");
                        break;
                    }
                }
            }

            running.store(false, Ordering::SeqCst);
            info!("Webhook channel stopped");
        });

        Ok(())
    }

    /// Stops the webhook HTTP server by sending the shutdown signal.
    async fn stop(&mut self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            info!("Webhook channel already stopped");
            return Ok(());
        }

        info!("Stopping webhook channel");

        if let Some(tx) = self.shutdown_tx.take() {
            if tx.send(()).is_err() {
                warn!("Webhook shutdown receiver already dropped");
            }
        }

        info!("Webhook channel stopped");
        Ok(())
    }

    /// Webhook is primarily inbound-only; outbound messages are logged but
    /// not delivered because there is no persistent return channel to the
    /// original HTTP caller.
    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ZeptoError::Channel(
                "Webhook channel not running".to_string(),
            ));
        }

        info!(
            "Webhook: outbound message to chat {} (logged only, no delivery): {}",
            msg.chat_id,
            crate::utils::string::preview(&msg.content, 80)
        );

        Ok(())
    }

    /// Returns whether the channel is currently running.
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Checks if a sender is allowed to use this channel.
    ///
    /// Delegates to the base configuration's allowlist logic.
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

    // -----------------------------------------------------------------------
    // 1. Config defaults
    // -----------------------------------------------------------------------

    #[test]
    fn test_webhook_config_defaults() {
        let config = WebhookChannelConfig::default();
        assert_eq!(config.bind_address, "127.0.0.1");
        assert_eq!(config.port, 9876);
        assert_eq!(config.path, "/webhook");
        assert!(config.auth_token.is_none());
    }

    #[test]
    fn test_webhook_config_custom() {
        let config = WebhookChannelConfig {
            bind_address: "0.0.0.0".to_string(),
            port: 8080,
            path: "/api/hook".to_string(),
            auth_token: Some("secret-token".to_string()),
        };
        assert_eq!(config.bind_address, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.path, "/api/hook");
        assert_eq!(config.auth_token, Some("secret-token".to_string()));
    }

    // -----------------------------------------------------------------------
    // 2. Channel creation and name
    // -----------------------------------------------------------------------

    #[test]
    fn test_webhook_channel_creation() {
        let config = WebhookChannelConfig::default();
        let base = BaseChannelConfig::new("webhook");
        let channel = WebhookChannel::new(config, base, test_bus());

        assert_eq!(channel.name(), "webhook");
        assert!(!channel.is_running());
    }

    #[test]
    fn test_webhook_channel_name() {
        let channel = WebhookChannel::new(
            WebhookChannelConfig::default(),
            BaseChannelConfig::new("webhook"),
            test_bus(),
        );
        assert_eq!(channel.name(), "webhook");
    }

    // -----------------------------------------------------------------------
    // 3. is_allowed delegation
    // -----------------------------------------------------------------------

    #[test]
    fn test_webhook_is_allowed_empty_allowlist() {
        let channel = WebhookChannel::new(
            WebhookChannelConfig::default(),
            BaseChannelConfig::new("webhook"),
            test_bus(),
        );
        // Empty allowlist means everyone is allowed
        assert!(channel.is_allowed("anyone"));
        assert!(channel.is_allowed("external-service"));
    }

    #[test]
    fn test_webhook_is_allowed_with_allowlist() {
        let base = BaseChannelConfig::with_allowlist(
            "webhook",
            vec!["service-a".to_string(), "service-b".to_string()],
        );
        let channel = WebhookChannel::new(WebhookChannelConfig::default(), base, test_bus());

        assert!(channel.is_allowed("service-a"));
        assert!(channel.is_allowed("service-b"));
        assert!(!channel.is_allowed("service-c"));
        assert!(!channel.is_allowed("unknown"));
    }

    // -----------------------------------------------------------------------
    // 4. Auth token validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_auth_validation_no_token_required() {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        assert!(WebhookChannel::validate_auth(&headers, &None));
    }

    #[test]
    fn test_auth_validation_valid_token() {
        let token = Some("my-secret".to_string());
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), "Bearer my-secret".to_string()),
        ];
        assert!(WebhookChannel::validate_auth(&headers, &token));
    }

    #[test]
    fn test_auth_validation_invalid_token() {
        let token = Some("my-secret".to_string());
        let headers = vec![(
            "Authorization".to_string(),
            "Bearer wrong-token".to_string(),
        )];
        assert!(!WebhookChannel::validate_auth(&headers, &token));
    }

    #[test]
    fn test_auth_validation_missing_header_when_required() {
        let token = Some("my-secret".to_string());
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        assert!(!WebhookChannel::validate_auth(&headers, &token));
    }

    #[test]
    fn test_auth_validation_case_insensitive_header_name() {
        let token = Some("tok123".to_string());
        let headers = vec![("authorization".to_string(), "Bearer tok123".to_string())];
        assert!(WebhookChannel::validate_auth(&headers, &token));

        let headers_upper = vec![("AUTHORIZATION".to_string(), "Bearer tok123".to_string())];
        assert!(WebhookChannel::validate_auth(&headers_upper, &token));
    }

    #[test]
    fn test_auth_validation_bearer_prefix_required() {
        let token = Some("my-secret".to_string());
        // Raw token without "Bearer " prefix should fail
        let headers = vec![("Authorization".to_string(), "my-secret".to_string())];
        assert!(!WebhookChannel::validate_auth(&headers, &token));
    }

    // -----------------------------------------------------------------------
    // 5. HTTP request parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_http_request_valid_post() {
        let raw = b"POST /webhook HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 60\r\n\r\n{\"message\":\"hello\",\"sender\":\"svc\",\"chat_id\":\"c1\"}";

        let req = WebhookChannel::parse_http_request(raw).expect("should parse");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/webhook");
        assert!(req
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type")));
        assert!(!req.body.is_empty());
    }

    #[test]
    fn test_parse_http_request_get() {
        let raw = b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req = WebhookChannel::parse_http_request(raw).expect("should parse");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/health");
    }

    #[test]
    fn test_parse_http_request_empty() {
        let raw = b"";
        let result = WebhookChannel::parse_http_request(raw);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 6. JSON body parsing (via serde)
    // -----------------------------------------------------------------------

    #[test]
    fn test_webhook_payload_valid() {
        let json = r#"{"message":"hello","sender":"svc-a","chat_id":"chat-1"}"#;
        let payload: WebhookPayload = serde_json::from_str(json).expect("should parse");
        assert_eq!(payload.message, "hello");
        assert_eq!(payload.sender, "svc-a");
        assert_eq!(payload.chat_id, "chat-1");
    }

    #[test]
    fn test_webhook_payload_missing_fields() {
        let json = r#"{"message":"hello"}"#;
        let result: std::result::Result<WebhookPayload, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_webhook_payload_malformed_json() {
        let json = r#"{not valid json"#;
        let result: std::result::Result<WebhookPayload, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_webhook_payload_extra_fields_ignored() {
        let json = r#"{"message":"hi","sender":"s","chat_id":"c","extra":"ignored","priority":1}"#;
        let payload: WebhookPayload = serde_json::from_str(json).expect("should parse");
        assert_eq!(payload.message, "hi");
    }

    // -----------------------------------------------------------------------
    // 7. Start/stop lifecycle (AtomicBool)
    // -----------------------------------------------------------------------

    #[test]
    fn test_webhook_not_running_initially() {
        let channel = WebhookChannel::new(
            WebhookChannelConfig::default(),
            BaseChannelConfig::new("webhook"),
            test_bus(),
        );
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_webhook_stop_when_not_running() {
        let mut channel = WebhookChannel::new(
            WebhookChannelConfig::default(),
            BaseChannelConfig::new("webhook"),
            test_bus(),
        );

        let result = channel.stop().await;
        assert!(result.is_ok());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_webhook_send_when_not_running() {
        let channel = WebhookChannel::new(
            WebhookChannelConfig::default(),
            BaseChannelConfig::new("webhook"),
            test_bus(),
        );

        let msg = OutboundMessage::new("webhook", "chat-1", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_webhook_send_when_running() {
        let channel = WebhookChannel::new(
            WebhookChannelConfig::default(),
            BaseChannelConfig::new("webhook"),
            test_bus(),
        );

        // Simulate running state without actually binding a port
        channel.running.store(true, Ordering::SeqCst);

        let msg = OutboundMessage::new("webhook", "chat-1", "Hello from agent");
        let result = channel.send(msg).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 8. Content-Length extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_content_length_present() {
        let headers = vec![
            ("Host".to_string(), "localhost".to_string()),
            ("Content-Length".to_string(), "42".to_string()),
        ];
        assert_eq!(WebhookChannel::content_length(&headers), 42);
    }

    #[test]
    fn test_content_length_missing() {
        let headers = vec![("Host".to_string(), "localhost".to_string())];
        assert_eq!(WebhookChannel::content_length(&headers), 0);
    }

    #[test]
    fn test_content_length_invalid() {
        let headers = vec![("Content-Length".to_string(), "not-a-number".to_string())];
        assert_eq!(WebhookChannel::content_length(&headers), 0);
    }

    // -----------------------------------------------------------------------
    // 9. Header-end detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_header_end_found() {
        let data = b"GET / HTTP/1.1\r\nHost: x\r\n\r\nbody";
        assert!(WebhookChannel::find_header_end(data).is_some());
    }

    #[test]
    fn test_find_header_end_not_found() {
        let data = b"GET / HTTP/1.1\r\nHost: x\r\n";
        assert!(WebhookChannel::find_header_end(data).is_none());
    }

    // -----------------------------------------------------------------------
    // 10. Config accessor
    // -----------------------------------------------------------------------

    #[test]
    fn test_webhook_config_accessor() {
        let config = WebhookChannelConfig {
            bind_address: "10.0.0.1".to_string(),
            port: 3000,
            path: "/hooks/inbound".to_string(),
            auth_token: Some("abc".to_string()),
        };
        let channel = WebhookChannel::new(config, BaseChannelConfig::new("webhook"), test_bus());
        let cfg = channel.webhook_config();
        assert_eq!(cfg.bind_address, "10.0.0.1");
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.path, "/hooks/inbound");
        assert_eq!(cfg.auth_token, Some("abc".to_string()));
    }

    // -----------------------------------------------------------------------
    // 11. Full integration: start, send request, verify bus message
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_start_accept_and_publish() {
        let bus = test_bus();

        // Use port 0 to let the OS pick a free port
        let _config = WebhookChannelConfig {
            bind_address: "127.0.0.1".to_string(),
            port: 0,
            path: "/webhook".to_string(),
            auth_token: None,
        };

        // We need to bind ourselves first to discover the actual port, then
        // simulate. Instead, let's do a start() with a known free port.
        // For a simpler approach, we call start() with port 0 — but we can't
        // easily discover the port. Instead, use handle_connection directly.

        // Direct unit test of handle_connection via an in-memory TCP pair.
        let (client_stream, server_stream) = tokio::io::duplex(4096);

        let base = BaseChannelConfig::new("webhook");
        let cfg = WebhookChannelConfig::default();
        let bus_clone = Arc::clone(&bus);

        // Convert DuplexStream halves into a TcpStream-like pair is not
        // directly possible, so we test the parsing + auth pipeline instead.
        // The integration flow (bind, connect, POST) is tested via parse + validate.

        let raw_request = b"POST /webhook HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 62\r\n\r\n{\"message\":\"integration\",\"sender\":\"svc\",\"chat_id\":\"ch1\"}";

        let req = WebhookChannel::parse_http_request(raw_request).expect("should parse");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/webhook");

        // Validate auth (none required)
        assert!(WebhookChannel::validate_auth(&req.headers, &cfg.auth_token));

        // Parse body
        let payload: WebhookPayload = serde_json::from_str(&req.body).expect("should parse body");
        assert_eq!(payload.message, "integration");
        assert_eq!(payload.sender, "svc");
        assert_eq!(payload.chat_id, "ch1");

        // Verify allowlist check passes
        assert!(base.is_allowed(&payload.sender));

        // Publish
        let inbound = InboundMessage::new(
            "webhook",
            &payload.sender,
            &payload.chat_id,
            &payload.message,
        );
        bus_clone.publish_inbound(inbound).await.unwrap();

        // Consume and verify
        let received = bus.consume_inbound().await.expect("should receive message");
        assert_eq!(received.channel, "webhook");
        assert_eq!(received.sender_id, "svc");
        assert_eq!(received.chat_id, "ch1");
        assert_eq!(received.content, "integration");
        assert_eq!(received.session_key, "webhook:ch1");

        // Clean up unused duplex streams
        drop(client_stream);
        drop(server_stream);
    }

    // -----------------------------------------------------------------------
    // 12. Start and stop with real TCP binding
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_start_and_stop_lifecycle() {
        // We need to pick a real available port. Bind a listener to get one.
        let temp_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("should bind temp listener");
        let port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let config = WebhookChannelConfig {
            bind_address: "127.0.0.1".to_string(),
            port,
            path: "/webhook".to_string(),
            auth_token: None,
        };

        let mut channel =
            WebhookChannel::new(config, BaseChannelConfig::new("webhook"), test_bus());

        // Start
        let start_result = channel.start().await;
        assert!(start_result.is_ok());
        assert!(channel.is_running());

        // Give the spawned task a moment to start accepting
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Stop
        let stop_result = channel.stop().await;
        assert!(stop_result.is_ok());

        // Give the spawned task a moment to wind down
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // -----------------------------------------------------------------------
    // 13. Double-start is idempotent
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_double_start() {
        let temp_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("should bind temp listener");
        let port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let config = WebhookChannelConfig {
            bind_address: "127.0.0.1".to_string(),
            port,
            path: "/webhook".to_string(),
            auth_token: None,
        };

        let mut channel =
            WebhookChannel::new(config, BaseChannelConfig::new("webhook"), test_bus());

        channel.start().await.unwrap();
        assert!(channel.is_running());

        // Second start should be a no-op
        let result = channel.start().await;
        assert!(result.is_ok());
        assert!(channel.is_running());

        channel.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // 14. End-to-end: connect, POST, read response, verify bus
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_end_to_end_post() {
        let bus = test_bus();

        let temp_listener = TcpListener::bind("127.0.0.1:0").await.expect("should bind");
        let port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let config = WebhookChannelConfig {
            bind_address: "127.0.0.1".to_string(),
            port,
            path: "/webhook".to_string(),
            auth_token: Some("test-token".to_string()),
        };

        let mut channel =
            WebhookChannel::new(config, BaseChannelConfig::new("webhook"), Arc::clone(&bus));

        channel.start().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send a POST request using a raw TCP connection
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("should connect");

        let body = r#"{"message":"e2e test","sender":"test-client","chat_id":"e2e-chat"}"#;
        let request = format!(
            "POST /webhook HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAuthorization: Bearer test-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        stream.write_all(request.as_bytes()).await.unwrap();

        // Read the response
        let mut response_buf = vec![0u8; 4096];
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read(&mut response_buf),
        )
        .await
        .expect("should not timeout")
        .expect("should read");

        let response = std::str::from_utf8(&response_buf[..n]).expect("valid utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"));

        // Verify the message was published to the bus
        let received =
            tokio::time::timeout(std::time::Duration::from_secs(2), bus.consume_inbound())
                .await
                .expect("should not timeout")
                .expect("should receive message");

        assert_eq!(received.channel, "webhook");
        assert_eq!(received.sender_id, "test-client");
        assert_eq!(received.chat_id, "e2e-chat");
        assert_eq!(received.content, "e2e test");

        channel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_webhook_end_to_end_unauthorized() {
        let temp_listener = TcpListener::bind("127.0.0.1:0").await.expect("should bind");
        let port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let config = WebhookChannelConfig {
            bind_address: "127.0.0.1".to_string(),
            port,
            path: "/webhook".to_string(),
            auth_token: Some("correct-token".to_string()),
        };

        let mut channel =
            WebhookChannel::new(config, BaseChannelConfig::new("webhook"), test_bus());

        channel.start().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("should connect");

        let body = r#"{"message":"test","sender":"s","chat_id":"c"}"#;
        let request = format!(
            "POST /webhook HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer wrong-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response_buf = vec![0u8; 4096];
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read(&mut response_buf),
        )
        .await
        .expect("should not timeout")
        .expect("should read");

        let response = std::str::from_utf8(&response_buf[..n]).expect("valid utf8");
        assert!(response.starts_with("HTTP/1.1 401"));

        channel.stop().await.unwrap();
    }
}
