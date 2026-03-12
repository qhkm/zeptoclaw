//! Webex channel implementation.
//!
//! Supports:
//! - Outbound messaging via Webex REST API (`POST /messages`)
//! - Inbound messaging via Webex Webhooks
//! 
//! Based on Cisco Webex Teams/Messaging API
//! Reference: https://developer.webex.com/docs/api/basics

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MediaAttachment, MediaType, MessageBus, OutboundMessage};
use crate::config::WebexConfig;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

const WEBEX_API_BASE: &str = "https://webexapis.com/v1";
const WEBEX_MESSAGES_ENDPOINT: &str = "/messages";
const WEBEX_PEOPLE_ENDPOINT: &str = "/people/me";
const WEBEX_WEBHOOKS_ENDPOINT: &str = "/webhooks";
const SHA1_BLOCK_SIZE: usize = 64;

/// Parsed HTTP request structure
#[derive(Debug)]
struct ParsedHttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

#[derive(Debug, Deserialize)]
struct WebexWebhookPayload {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    resource: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    data: Option<WebexMessageData>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct WebexMessageData {
    id: String,
    #[serde(rename = "roomId")]
    room_id: String,
    #[serde(rename = "roomType")]
    #[serde(default)]
    room_type: String,
    #[serde(rename = "personId")]
    person_id: String,
    #[serde(rename = "personEmail")]
    person_email: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    created: String,
}

#[derive(Debug, Serialize)]
struct WebexOutboundMessage {
    #[serde(rename = "roomId")]
    room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentId")]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebexPerson {
    id: String,
    #[serde(default)]
    emails: Vec<String>,
    #[serde(rename = "displayName")]
    #[serde(default)]
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct WebexWebhook {
    id: String,
    name: String,
    #[serde(rename = "targetUrl")]
    target_url: String,
    resource: String,
    event: String,
}

/// HMAC-SHA1 implementation for Webex signature verification
fn hmac_sha1_hex(key: &[u8], message: &[u8]) -> String {
    let mut k = [0u8; SHA1_BLOCK_SIZE];
    if key.len() > SHA1_BLOCK_SIZE {
        let hashed = Sha1::digest(key);
        k[..20].copy_from_slice(&hashed);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut k_ipad = [0u8; SHA1_BLOCK_SIZE];
    let mut k_opad = [0u8; SHA1_BLOCK_SIZE];
    for i in 0..SHA1_BLOCK_SIZE {
        k_ipad[i] = k[i] ^ 0x36;
        k_opad[i] = k[i] ^ 0x5c;
    }

    let mut inner = Sha1::new();
    inner.update(k_ipad);
    inner.update(message);
    let inner_result = inner.finalize();

    let mut outer = Sha1::new();
    outer.update(k_opad);
    outer.update(inner_result);
    hex::encode(outer.finalize())
}

/// Parse raw HTTP request
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
    let path = parts
        .next()
        .ok_or_else(|| ZeptoError::Channel("Missing HTTP path".to_string()))?
        .to_string();

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

/// Extract header value by name (case-insensitive)
fn get_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Webex channel implementation
pub struct WebexChannel {
    config: WebexConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    client: reqwest::Client,
    shutdown_tx: Option<mpsc::Sender<()>>,
    bot_id: Option<String>,
    webhook_id: Option<String>,
}

impl WebexChannel {
    /// Creates a new Webex channel.
    pub fn new(config: WebexConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "webex".to_string(),
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
            bot_id: None,
            webhook_id: None,
        }
    }

    /// Get bot's own user ID
    async fn get_bot_id(&self) -> Result<String> {
        let url = format!("{}{}", WEBEX_API_BASE, WEBEX_PEOPLE_ENDPOINT);
        
        let response = self.client
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to get bot info: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Channel(format!(
                "Webex API error ({}): {}",
                status, error_text
            )));
        }

        let person: WebexPerson = response
            .json()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to parse bot info: {}", e)))?;

        info!("Webex bot: {} ({})", person.display_name, person.id);
        Ok(person.id)
    }

    /// Register webhook with Webex
    async fn register_webhook(&self, webhook_url: &str) -> Result<String> {
        let url = format!("{}{}", WEBEX_API_BASE, WEBEX_WEBHOOKS_ENDPOINT);
        
        let mut webhook = serde_json::json!({
            "name": format!("Zeptoclaw-{}", uuid::Uuid::new_v4()),
            "targetUrl": webhook_url,
            "resource": "messages",
            "event": "created",
        });

        if let Some(ref secret) = self.config.webhook_secret {
            webhook["secret"] = serde_json::json!(secret);
        }

        let response = self.client
            .post(&url)
            .bearer_auth(&self.config.access_token)
            .json(&webhook)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to register webhook: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Channel(format!(
                "Failed to register webhook ({}): {}",
                status, error_text
            )));
        }

        let created: WebexWebhook = response
            .json()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to parse webhook response: {}", e)))?;

        info!("Registered Webex webhook: {} -> {}", created.id, webhook_url);
        Ok(created.id)
    }

    /// Delete webhook
    async fn delete_webhook(&self, webhook_id: &str) -> Result<()> {
        let url = format!("{}{}/{}", WEBEX_API_BASE, WEBEX_WEBHOOKS_ENDPOINT, webhook_id);
        
        let response = self.client
            .delete(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to delete webhook: {}", e)))?;

        if !response.status().is_success() {
            warn!("Failed to delete Webex webhook {}: HTTP {}", webhook_id, response.status());
        } else {
            info!("Deleted Webex webhook: {}", webhook_id);
        }

        Ok(())
    }

    /// Verify webhook signature
    fn verify_signature(&self, signature: &str, body: &str) -> bool {
        let Some(ref secret) = self.config.webhook_secret else {
            return true; // No secret configured, skip verification
        };

        let expected = hmac_sha1_hex(secret.as_bytes(), body.as_bytes());
        signature == expected
    }

    /// Process incoming webhook
    async fn process_webhook(&self, payload: WebexWebhookPayload) -> Result<()> {
        // Only handle message creation events
        if payload.resource != "messages" || payload.event != "created" {
            debug!("Ignoring non-message webhook: {} {}", payload.resource, payload.event);
            return Ok(());
        }

        let Some(data) = payload.data else {
            debug!("Webhook missing data field");
            return Ok(());
        };

        // Ignore messages from bot itself
        if let Some(ref bot_id) = self.bot_id {
            if data.person_id == *bot_id {
                debug!("Ignoring message from bot itself");
                return Ok(());
            }
        }

        // Check allowlist
        if !self.base_config.is_allowed(&data.person_id) {
            debug!("User {} not in allowlist, ignoring", data.person_id);
            return Ok(());
        }

        // Extract message text (prefer text over markdown)
        let content = data.text.clone()
            .or_else(|| data.markdown.clone())
            .unwrap_or_default();

        if content.is_empty() && data.files.is_empty() {
            debug!("Empty message, ignoring");
            return Ok(());
        }

        // Process file attachments
        let mut attachments = Vec::new();
        for file_url in &data.files {
            match self.download_file(file_url).await {
                Ok(attachment) => attachments.push(attachment),
                Err(e) => warn!("Failed to download Webex file: {}", e),
            }
        }

        let mut inbound_msg = InboundMessage::new(
            "webex",
            &data.person_id,
            &data.room_id,
            &content,
        );
        inbound_msg.media = attachments;
        inbound_msg.metadata.insert("person_email".to_string(), data.person_email.clone());
        inbound_msg.metadata.insert("room_type".to_string(), data.room_type.clone());
        if let Ok(raw) = serde_json::to_string(&data) {
            inbound_msg.metadata.insert("raw_webhook_data".to_string(), raw);
        }

        self.bus.publish_inbound(inbound_msg).await;
        Ok(())
    }

    /// Download file from Webex
    async fn download_file(&self, file_url: &str) -> Result<MediaAttachment> {
        let response = self.client
            .get(file_url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to download file: {}", e)))?;

        if !response.status().is_success() {
            return Err(ZeptoError::Channel(format!(
                "Failed to download file: HTTP {}",
                response.status()
            )));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let data = response
            .bytes()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to read file data: {}", e)))?
            .to_vec();

        let media_type = Self::parse_media_type(&content_type);

        Ok(MediaAttachment {
            media_type,
            url: None,
            data: Some(data),
            filename: None,
            mime_type: Some(content_type),
        })
    }

    fn parse_media_type(content_type: &str) -> MediaType {
        let ct = content_type.to_lowercase();
        if ct.starts_with("image/") {
            MediaType::Image
        } else if ct.starts_with("audio/") {
            MediaType::Audio
        } else if ct.starts_with("video/") {
            MediaType::Video
        } else {
            MediaType::Document
        }
    }

    /// Start webhook HTTP server
    async fn start_webhook_server(&mut self) -> Result<mpsc::Sender<()>> {
        let webhook_url = self.config.webhook_url.as_ref()
            .ok_or_else(|| ZeptoError::Channel("webhook_url is required".to_string()))?
            .clone();

        // Register webhook with Webex
        let webhook_id = self.register_webhook(&webhook_url).await?;
        self.webhook_id = Some(webhook_id);

        let addr = format!("{}:{}", self.config.bind_address, self.config.port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to bind {}: {}", addr, e)))?;

        info!("Webex webhook server listening on {}", addr);

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Clone data for async task
        let running = Arc::clone(&self.running);
        let bus = Arc::clone(&self.bus);
        let config = self.config.clone();
        let base_config = self.base_config.clone();
        let client = self.client.clone();
        let bot_id = self.bot_id.clone();

        tokio::spawn(async move {
            let channel = WebexChannel {
                config,
                base_config,
                bus,
                running: running.clone(),
                client,
                shutdown_tx: None,
                bot_id,
                webhook_id: None,
            };

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Webex webhook server shutting down");
                        break;
                    }
                    result = listener.accept() => {
                        match result {
                            Ok((mut socket, addr)) => {
                                debug!("Webex webhook connection from {}", addr);
                                let ch = channel.clone_for_handler();
                                tokio::spawn(async move {
                                    if let Err(e) = ch.handle_connection(&mut socket).await {
                                        error!("Webex webhook handler error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Failed to accept connection: {}", e);
                            }
                        }
                    }
                }
            }
        });

        Ok(shutdown_tx)
    }

    fn clone_for_handler(&self) -> Self {
        WebexChannel {
            config: self.config.clone(),
            base_config: self.base_config.clone(),
            bus: Arc::clone(&self.bus),
            running: Arc::clone(&self.running),
            client: self.client.clone(),
            shutdown_tx: None,
            bot_id: self.bot_id.clone(),
            webhook_id: None,
        }
    }

    async fn handle_connection(&self, socket: &mut tokio::net::TcpStream) -> Result<()> {
        let mut buffer = vec![0u8; 8192];
        let n = socket.read(&mut buffer).await
            .map_err(|e| ZeptoError::Channel(format!("Failed to read request: {}", e)))?;

        if n == 0 {
            return Ok(());
        }

        let request = parse_http_request(&buffer[..n])?;

        // Only handle POST requests
        if request.method != "POST" {
            let response = "HTTP/1.1 405 Method Not Allowed\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
            return Ok(());
        }

        // Verify signature if configured
        if let Some(signature) = get_header(&request.headers, "X-Spark-Signature") {
            if !self.verify_signature(signature, &request.body) {
                warn!("Invalid Webex webhook signature");
                let response = "HTTP/1.1 401 Unauthorized\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                return Ok(());
            }
        }

        // Parse payload
        let payload: WebexWebhookPayload = match serde_json::from_str(&request.body) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to parse Webex webhook payload: {}", e);
                let response = "HTTP/1.1 400 Bad Request\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                return Ok(());
            }
        };

        // Send 200 OK response immediately
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
        let _ = socket.write_all(response.as_bytes()).await;

        // Process webhook asynchronously
        if let Err(e) = self.process_webhook(payload).await {
            error!("Failed to process Webex webhook: {}", e);
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for WebexChannel {
    fn name(&self) -> &str {
        &self.base_config.name
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Starting Webex channel...");

        // Get bot ID
        self.bot_id = Some(self.get_bot_id().await?);

        // Start webhook server
        if self.config.webhook_url.is_none() {
            return Err(ZeptoError::Channel(
                "Webex requires webhook_url to be configured".to_string(),
            ));
        }

        let shutdown_tx = self.start_webhook_server().await?;
        self.shutdown_tx = Some(shutdown_tx);

        self.running.store(true, Ordering::SeqCst);
        info!("Webex channel started");

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Stopping Webex channel...");

        // Delete webhook
        if let Some(ref webhook_id) = self.webhook_id {
            let _ = self.delete_webhook(webhook_id).await;
            self.webhook_id = None;
        }

        // Stop server
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        self.running.store(false, Ordering::SeqCst);
        info!("Webex channel stopped");

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        let url = format!("{}{}", WEBEX_API_BASE, WEBEX_MESSAGES_ENDPOINT);

        let outbound = WebexOutboundMessage {
            room_id: msg.chat_id.clone(),
            text: Some(msg.content.clone()),
            markdown: None,
            parent_id: msg.reply_to.clone(),
        };

        let response = self.client
            .post(&url)
            .bearer_auth(&self.config.access_token)
            .json(&outbound)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to send Webex message: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Channel(format!(
                "Webex API error ({}): {}",
                status, error_text
            )));
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
