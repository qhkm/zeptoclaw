//! Webex channel implementation.
//!
//! # About Webex
//!
//! Webex (formerly Cisco Webex Teams) is an enterprise collaboration platform that provides
//! messaging, video conferencing, and file sharing. The Webex Messaging API allows bots to
//! send and receive messages in Webex spaces (rooms), enabling automated interactions and
//! integrations with external services.
//!
//! This implementation uses the official Webex REST API to enable zeptoclaw agents to
//! interact with users through Webex.
//!
//! # Implementation Modes
//!
//! This module implements both webhook and polling modes for receiving messages from Webex.
//!
//! Zeptoclaw supports **two different modes** for receiving messages from Webex:
//!
//! ## 1. Webhook Mode (Recommended for Production)
//!
//! Webex pushes messages to your server via HTTP webhooks.
//!
//! **How it works:**
//! - Starts an HTTP server listening for Webex webhook events
//! - Real-time message delivery when webhook fires
//! - Requires public HTTPS endpoint and webhook registration
//!
//! **Pros:**
//! - Real-time message delivery (immediate responses)
//! - Lower resource usage (event-driven)
//! - Official Webex recommendation
//!
//! **Cons:**
//! - Requires publicly accessible HTTPS endpoint
//! - Needs webhook registration
//! - More complex firewall configuration
//!
//! **Use when:** You have a public server or reverse proxy (e.g., nginx, ngrok)
//!
//! ## 2. Polling Mode
//!
//! Zeptoclaw periodically polls the Webex API for new messages.
//!
//! **TODO:** Implement real WebSocket mode using Webex Mercury protocol for true persistent connections
//! instead of polling. Mercury provides bidirectional real-time communication without requiring a public
//! endpoint, combining the benefits of both webhook and polling modes.
//!
//! **How it works:**
//! - Runs a background task that periodically fetches new messages
//! - Polls all rooms the bot is a member of
//! - Automatically filters mentioned messages in group spaces
//!
//! **Pros:**
//! - Works behind firewalls/NAT
//! - No public endpoint needed
//! - Easier for development/testing
//! - Automatic message deduplication
//!
//! **Cons:**
//! - Slight delay in responses (polling interval)
//! - Higher API usage
//! - More resource intensive
//!
//! **Use when:** You're behind a firewall or don't have a public IP
//!
//! ## Common Features (Both Modes)
//!
//! Both modes support:
//! - Message deduplication (prevents processing the same message twice)
//! - User allowlisting (restrict bot access to specific users)
//! - File attachments (downloads and processes files from messages)
//! - Markdown formatting in responses
//! - Room type detection (1:1 vs group spaces)
//!
//! Based on Cisco Webex Teams/Messaging API  
//! Reference: <https://developer.webex.com/docs/api/basics>
//!
//! # Configuration
//!
//! ## Webhook Mode Configuration
//! 
//! Add this to your `~/.zeptoclaw/config.json`:
//! 
//! ```json
//! {
//!   "channels": {
//!     "webex": {
//!       "enabled": true,
//!       "access_token": "YOUR_WEBEX_BOT_ACCESS_TOKEN",
//!       "webhook_url": "https://yourdomain.com/webhook",
//!       "webhook_secret": "optional-but-recommended-secret",
//!       "bind_address": "0.0.0.0",
//!       "port": 8084,
//!       "allow_from": [],
//!       "deny_by_default": false
//!     }
//!   }
//! }
//! ```
//!
//! ## Polling Mode Configuration
//!
//! For polling mode, omit `webhook_url` or set `polling_enabled` to true:
//!
//! ```json
//! {
//!   "channels": {
//!     "webex": {
//!       "enabled": true,
//!       "access_token": "YOUR_WEBEX_BOT_ACCESS_TOKEN",
//!       "polling_enabled": true,
//!       "polling_interval_secs": 15,
//!       "allow_from": [],
//!       "deny_by_default": false
//!     }
//!   }
//! }
//! ```
//!
//! **Configuration Options:**
//! - `polling_enabled`: Set to `true` to enable polling mode (default: auto-enabled if no webhook_url)
//! - `polling_interval_secs`: How often to check for new messages in seconds (default: 15)
//! - `mentioned_messages_only`: Only process messages that mention the bot (default: true)
//!
//! # Creating a Webex Bot
//! 
//! 1. Go to <https://developer.webex.com/>
//! 2. Sign in with your Webex account
//! 3. Navigate to "My Webex Apps" → "Create a New App"
//! 4. Select "Create a Bot"
//! 5. Fill in:
//!    - Bot name
//!    - Bot username
//!    - Icon (optional)
//!    - Description
//! 6. Click "Add Bot"
//! 7. Copy the **Bot Access Token** (this is your `access_token`)
//! 
//! # Webhook Mode Setup
//! 
//! ## Option 1: Public Webhook (Recommended for Production)
//! 
//! 1. You need a publicly accessible URL (e.g., `https://yourdomain.com/webhook`)
//! 2. Set `webhook_url` to your public URL
//! 3. Set a `webhook_secret` for signature verification
//! 4. Configure `bind_address` and `port` for the local server
//! 
//! ## Option 2: Development with ngrok
//! 
//! ```bash
//! # Terminal 1: Start zeptoclaw gateway
//! zeptoclaw gateway --config ~/.zeptoclaw/config.json
//! 
//! # Terminal 2: Expose with ngrok
//! ngrok http 8084
//! ```
//! 
//! Then update your config:
//! ```json
//! {
//!   "channels": {
//!     "webex": {
//!       "webhook_url": "https://YOUR-NGROK-ID.ngrok.io/webhook",
//!       "port": 8084
//!     }
//!   }
//! }
//! ```
//!
//! # Polling Mode Setup
//!
//! Polling mode is ideal when you cannot expose a public endpoint or are behind a firewall.
//!
//! ## Step-by-Step Setup
//!
//! 1. **Create your bot** (see "Creating a Webex Bot" section above)
//! 2. **Configure polling mode** in your `~/.zeptoclaw/config.json`:
//!
//! ```json
//! {
//!   "channels": {
//!     "webex": {
//!       "enabled": true,
//!       "access_token": "YOUR_WEBEX_BOT_ACCESS_TOKEN",
//!       "polling_enabled": true,
//!       "polling_interval_secs": 15,
//!       "mentioned_messages_only": true
//!     }
//!   }
//! }
//! ```
//!
//! 3. **Start zeptoclaw**:
//!
//! ```bash
//! zeptoclaw gateway --config ~/.zeptoclaw/config.json
//! ```
//!
//! 4. **Add bot to a Webex space** and start chatting!
//!
//! ## How It Works
//!
//! - Zeptoclaw polls the Webex API every `polling_interval_secs` seconds
//! - In group spaces, only messages that @mention the bot are processed (configurable)
//! - In direct messages, all messages are processed
//! - Message deduplication prevents processing the same message twice
//! - Historical messages (sent before bot startup) are automatically ignored
//!
//! ## Tuning Performance
//!
//! - **Lower interval (5-10s)**: Faster responses, higher API usage
//! - **Higher interval (15-30s)**: Slower responses, lower API usage
//! - **mentioned_messages_only**: Set to `false` to process all messages in group spaces
//!
//! ## When to Use Polling Mode
//!
//! ✅ **Use polling when:**
//! - You're behind a corporate firewall
//! - You don't have a public IP address
//! - You're testing/developing locally
//! - You can't configure webhooks
//!
//! ❌ **Avoid polling when:**
//! - You need instant responses (use webhook mode)
//! - You have many active rooms (high API usage)
//! - You have a public endpoint available
//! 
//! # Security Settings
//! 
//! ## Allow Specific Users Only
//! 
//! ```json
//! {
//!   "channels": {
//!     "webex": {
//!       "allow_from": [
//!         "user1@company.com-person-id",
//!         "user2@company.com-person-id"
//!       ],
//!       "deny_by_default": true
//!     }
//!   }
//! }
//! ```
//! 
//! To find a person ID:
//! - Go to <https://developer.webex.com/docs/api/v1/people/list-people>
//! - Use the API to search by email
//! 
//! ## Webhook Signature Verification
//! 
//! Always set a `webhook_secret` in production:
//! 
//! ```json
//! {
//!   "channels": {
//!     "webex": {
//!       "webhook_secret": "your-random-secret-string-here"
//!     }
//!   }
//! }
//! ```
//! 
//! # Testing Your Bot
//! 
//! 1. Add your bot to a Webex space
//! 2. Send a message: `@YourBot hello`
//! 3. Check zeptoclaw logs for incoming webhooks
//! 4. The bot should respond according to your agent configuration
//! 
//! # Troubleshooting
//! 
//! ## Bot doesn't receive messages
//! - Check that `webhook_url` is publicly accessible
//! - Verify the URL in Webex Developer portal (Webhooks section)
//! - Check zeptoclaw logs for webhook errors
//! - Ensure port is not blocked by firewall
//! 
//! ## Invalid signature errors
//! - Verify `webhook_secret` matches what you configured in Webex
//! - Check if the secret has special characters (URL-encode if needed)
//! 
//! ## Bot responds to its own messages
//! - This is prevented automatically by checking bot ID
//! - If you see loops, check the logs
//! 
//! # Features Supported
//! 
//! **Both Modes (Webhook & Polling):**
//! - ✅ Sending Messages - Text responses via REST API
//! - ✅ Receiving Messages - Via webhooks or polling  
//! - ✅ File Attachments - Download and process files
//! - ✅ Room Detection - Works in 1:1 and group spaces
//! - ✅ User Mentions - Detect when bot is mentioned
//! - ✅ Allowlist - Restrict access by person ID
//!
//! **Webhook Mode Only:**
//! - ✅ Signature Verification - HMAC-SHA1 webhook security
//! - ✅ Real-time delivery - Instant message processing
//!
//! **Polling Mode Only:**
//! - ✅ Message deduplication - Prevents processing same message multiple times
//! - ✅ Firewall friendly - Works behind NAT/firewalls
//! 
//! # Example Full Configuration
//! 
//! ## Webhook Mode (Production)
//!
//! ```json
//! {
//!   "agents": {
//!     "defaults": {
//!       "model": "gpt-4",
//!       "max_tokens": 16384
//!     }
//!   },
//!   "channels": {
//!     "webex": {
//!       "enabled": true,
//!       "access_token": "YzJh...your-long-token...M2Y",
//!       "webhook_url": "https://mybot.example.com/webhook",
//!       "webhook_secret": "super-secret-signature-key",
//!       "bind_address": "0.0.0.0",
//!       "port": 8084,
//!       "allow_from": [],
//!       "deny_by_default": false
//!     }
//!   },
//!   "providers": {
//!     "openai": {
//!       "api_key": "sk-..."
//!     }
//!   }
//! }
//! ```
//!
//! ## Polling Mode (Development/Behind Firewall)
//!
//! ```json
//! {
//!   "agents": {
//!     "defaults": {
//!       "model": "gpt-4",
//!       "max_tokens": 16384
//!     }
//!   },
//!   "channels": {
//!     "webex": {
//!       "enabled": true,
//!       "access_token": "YzJh...your-long-token...M2Y",
//!       "polling_enabled": true,
//!       "polling_interval_secs": 15,
//!       "mentioned_messages_only": true,
//!       "allow_from": [],
//!       "deny_by_default": false
//!     }
//!   },
//!   "providers": {
//!     "openai": {
//!       "api_key": "sk-..."
//!     }
//!   }
//! }
//! ```
//! 
//! # Next Steps
//! 
//! 1. Create your bot at <https://developer.webex.com/>
//! 2. Get the access token
//! 3. Update your config.json (choose webhook or polling mode)
//! 4. Start zeptoclaw: `zeptoclaw gateway --config ~/.zeptoclaw/config.json`
//! 5. Add bot to a Webex space and test by mentioning it: `@YourBot hello`
//! 
//! # Additional Resources
//! 
//! For more information, see:
//! - Webex Bot Documentation: <https://developer.webex.com/docs/bots>
//! - Webex Webhooks Guide: <https://developer.webex.com/docs/api/guides/webhooks>
//! - Webex Messages API: <https://developer.webex.com/docs/api/v1/messages>

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
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

#[derive(Debug, Deserialize)]
struct WebexMessageList {
    items: Vec<WebexMessageData>,
}

#[derive(Debug, Deserialize)]
struct WebexRoomList {
    items: Vec<WebexRoom>,
}

#[derive(Debug, Deserialize, Clone)]
struct WebexRoom {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "type")]
    #[serde(default)]
    room_type: String,  // "direct" or "group"
}

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
    html: Option<String>,
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
    processed_message_ids: Arc<Mutex<HashSet<String>>>,
    startup_time: Arc<Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
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
            processed_message_ids: Arc::new(Mutex::new(HashSet::new())),
            startup_time: Arc::new(Mutex::new(None)),
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

    /// Fetch all rooms the bot is in
    async fn get_rooms(&self) -> Result<Vec<WebexRoom>> {
        let url = format!("{}/rooms", WEBEX_API_BASE);
        
        let response = self.client
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .query(&[("max", "100")])
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to fetch rooms: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Channel(format!(
                "Webex API error fetching rooms ({}): {}",
                status, error_text
            )));
        }

        let room_list: WebexRoomList = response
            .json()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to parse rooms: {}", e)))?;

        Ok(room_list.items)
    }

    /// Poll for new messages in a specific room
    async fn poll_room_messages(&self, room_id: &str, is_direct: bool) -> Result<Vec<WebexMessageData>> {
        let url = format!("{}{}", WEBEX_API_BASE, WEBEX_MESSAGES_ENDPOINT);
        
        let bot_id = self.bot_id.as_ref()
            .ok_or_else(|| ZeptoError::Channel("Bot ID not set".to_string()))?;
        
        let mut request = self.client
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .query(&[("roomId", room_id)]);
        
        // For group spaces, only get messages that mention the bot
        // For direct messages, get all messages
        if !is_direct {
            request = request.query(&[("mentionedPeople", bot_id.as_str())]);
        }
        
        let response = request
            .query(&[("max", "50")])
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to poll messages: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            
            // Handle rate limiting
            if status.as_u16() == 429 {
                let retry_after = response.headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(60);
                warn!("Rate limited, should retry after {} seconds", retry_after);
            }
            
            let error_text = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Channel(format!(
                "Webex API error ({}): {}",
                status, error_text
            )));
        }

        let message_list: WebexMessageList = response
            .json()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to parse messages: {}", e)))?;

        Ok(message_list.items)
    }

    /// Poll for new messages across all rooms
    async fn poll_messages(&self) -> Result<Vec<WebexMessageData>> {
        // Get all rooms the bot is in
        let rooms = self.get_rooms().await?;
        
        let mut all_messages = Vec::new();
        
        // Poll each room for messages
        for room in rooms {
            let is_direct = room.room_type == "direct";
            match self.poll_room_messages(&room.id, is_direct).await {
                Ok(mut messages) => all_messages.append(&mut messages),
                Err(e) => {
                    // Don't spam logs for rate limiting - already logged in poll_room_messages
                    if !e.to_string().contains("429") {
                        warn!("Failed to poll room {}: {}", room.title, e);
                    }
                    // Continue with other rooms even if one fails
                }
            }
        }
        
        Ok(all_messages)
    }

    /// Process a polled message
    async fn process_message(&self, data: WebexMessageData) -> Result<()> {
        // Ignore messages from bot itself
        if let Some(ref bot_id) = self.bot_id {
            if data.person_id == *bot_id {
                debug!("Skipping bot's own message: {}", data.id);
                return Ok(());
            }
        }

        // Skip messages created before bot startup (historical messages)
        if let Some(startup_time) = *self.startup_time.lock().await {
            if let Ok(msg_time) = chrono::DateTime::parse_from_rfc3339(&data.created) {
                if msg_time.with_timezone(&Utc) < startup_time {
                    debug!("Skipping historical message: {} from {} (created before startup)", 
                           data.id, data.person_email);
                    return Ok(());
                }
            }
        }

        // Check if we've already processed this message
        let mut processed_ids = self.processed_message_ids.lock().await;
        if processed_ids.contains(&data.id) {
            debug!("Skipping already-processed message: {} from {}", data.id, data.person_email);
            return Ok(());
        }

        // Mark message as processed
        info!("Processing NEW message: {} from {}", data.id, data.person_email);
        processed_ids.insert(data.id.clone());
        
        // Keep the set from growing indefinitely - keep last 1000 message IDs
        if processed_ids.len() > 1000 {
            // Remove oldest half (crude but effective)
            let to_remove: Vec<String> = processed_ids.iter().take(500).cloned().collect();
            for id in to_remove {
                processed_ids.remove(&id);
            }
        }
        drop(processed_ids);

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
            return Ok(());
        }

        info!("Webex message from {} (room_type={}): {}", data.person_email, data.room_type, content);

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
            inbound_msg.metadata.insert("raw_message_data".to_string(), raw);
        }

        self.bus.publish_inbound(inbound_msg).await;
        Ok(())
    }

    /// Start message polling loop
    async fn start_polling(&mut self) -> Result<mpsc::Sender<()>> {
        info!("Webex channel using polling mode (interval: {}s)", self.config.poll_interval_secs);

        // Record startup time - only process messages created after this
        let startup_time = Utc::now();
        *self.startup_time.lock().await = Some(startup_time);
        info!("Webex polling will ignore messages before {}", startup_time.to_rfc3339());

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Clone data for async task
        let running = Arc::clone(&self.running);
        let bus = Arc::clone(&self.bus);
        let config = self.config.clone();
        let base_config = self.base_config.clone();
        let client = self.client.clone();
        let bot_id = self.bot_id.clone();
        let processed_message_ids = Arc::clone(&self.processed_message_ids);
        let startup_time = Arc::clone(&self.startup_time);
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);

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
                processed_message_ids: processed_message_ids.clone(),
                startup_time: startup_time.clone(),
            };

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Webex polling shutting down");
                        break;
                    }
                    _ = sleep(poll_interval) => {
                        match channel.poll_messages().await {
                            Ok(messages) => {
                                if !messages.is_empty() {
                                    debug!("Polled {} messages from Webex", messages.len());
                                }
                                // Process messages in reverse order (oldest first)
                                for msg in messages.into_iter().rev() {
                                    if let Err(e) = channel.process_message(msg).await {
                                        error!("Failed to process Webex message: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Failed to poll Webex messages: {}", e);
                            }
                        }
                    }
                }
            }
        });

        Ok(shutdown_tx)
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
                processed_message_ids: Arc::new(Mutex::new(HashSet::new())),
                startup_time: Arc::new(Mutex::new(None)),
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
            processed_message_ids: Arc::clone(&self.processed_message_ids),
            startup_time: Arc::clone(&self.startup_time),
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

        // Choose between webhook and polling mode
        let shutdown_tx = if self.config.webhook_url.is_some() {
            // Webhook mode
            self.start_webhook_server().await?
        } else {
            // Polling mode
            self.start_polling().await?
        };
        
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

        // Check if this is a group space and we should mention the user
        let room_type = msg.metadata.get("room_type");
        let sender_id = msg.metadata.get("sender_id");
        let sender_email = msg.metadata.get("sender_email");
        
        debug!("Sending to room_type={:?}, sender_id={:?}", room_type, sender_id);
        
        let (text, html) = if room_type.map(|s| s.as_str()) == Some("group") {
            // For group spaces, mention the user in the response
            if let (Some(person_id), Some(email)) = (sender_id, sender_email)  {
                let display_name = email.split('@').next().unwrap_or(email);
                let html_content = format!(
                    "<spark-mention data-object-type=\"person\" data-object-id=\"{}\">{}</spark-mention> {}",
                    person_id, display_name, msg.content
                );
                info!("Sending group message with mention to {}", display_name);
                (None, Some(html_content))
            } else {
                (Some(msg.content.clone()), None)
            }
        } else {
            // For direct messages, just send the text
            info!("Sending direct message (room_type={:?})", room_type);
            (Some(msg.content.clone()), None)
        };

        let outbound = WebexOutboundMessage {
            room_id: msg.chat_id.clone(),
            text,
            markdown: None,
            html,
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
