//! Discord channel implementation.
//!
//! Connects to Discord via the Gateway WebSocket API (v10) for inbound messages
//! and uses the REST API for outbound messaging. Uses raw `tokio-tungstenite`
//! and `reqwest` -- no third-party Discord SDK crate required.
//!
//! # Gateway flow
//!
//! 1. GET `https://discord.com/api/v10/gateway` to obtain the WebSocket URL.
//! 2. Connect via `tokio-tungstenite`.
//! 3. Receive opcode 10 (HELLO) -- extract `heartbeat_interval`.
//! 4. Send opcode 2 (IDENTIFY) with bot token and intents.
//! 5. Start a periodic heartbeat task (opcode 1).
//! 6. Listen for opcode 0 (DISPATCH) events, specifically `MESSAGE_CREATE`.
//! 7. Reconnect with exponential backoff on disconnection.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::DiscordConfig;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DISCORD_GATEWAY_URL: &str = "https://discord.com/api/v10/gateway";

/// Maximum reconnect delay (in seconds) for exponential backoff.
const MAX_RECONNECT_DELAY_SECS: u64 = 120;
/// Base reconnect delay (in seconds).
const BASE_RECONNECT_DELAY_SECS: u64 = 2;
/// Maximum number of consecutive reconnect attempts before resetting backoff.
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Discord Gateway intents bitmask.
/// GUILDS (1 << 0) | GUILD_MESSAGES (1 << 9) | DIRECT_MESSAGES (1 << 12) | MESSAGE_CONTENT (1 << 15)
const GATEWAY_INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 12) | (1 << 15);

/// Discord message content length limit.
const DISCORD_MAX_MESSAGE_LENGTH: usize = 2000;
const DISCORD_CHANNEL_TYPE_GUILD_FORUM: u8 = 15;
const DISCORD_CHANNEL_TYPE_GUILD_MEDIA: u8 = 16;

// ---------------------------------------------------------------------------
// Gateway payload types (deserialization)
// ---------------------------------------------------------------------------

/// Top-level Discord Gateway payload.
#[derive(Debug, Deserialize)]
struct GatewayPayload {
    /// Gateway opcode.
    op: u8,
    /// Event data (shape depends on opcode / event name).
    #[serde(default)]
    d: Option<Value>,
    /// Sequence number (used for heartbeat and resume).
    #[serde(default)]
    s: Option<u64>,
    /// Event name (only present for opcode 0 / DISPATCH).
    #[serde(default)]
    t: Option<String>,
}

/// The `d` field of a HELLO (opcode 10) payload.
#[derive(Debug, Deserialize)]
struct HelloData {
    heartbeat_interval: u64,
}

/// The `d` field of a MESSAGE_CREATE dispatch event.
#[derive(Debug, Deserialize)]
struct MessageCreateData {
    /// The message text content.
    #[serde(default)]
    content: String,
    /// The Discord channel ID this message was sent in.
    channel_id: String,
    /// The message author.
    author: MessageAuthor,
    /// The unique message ID.
    id: String,
}

/// Author of a Discord message.
#[derive(Debug, Deserialize)]
struct MessageAuthor {
    /// The user's snowflake ID.
    id: String,
    /// Whether the user is a bot.
    #[serde(default)]
    bot: Option<bool>,
}

/// Response from GET /gateway.
#[derive(Debug, Deserialize)]
struct GatewayResponse {
    url: String,
}

#[derive(Debug, Deserialize)]
struct DiscordChannelInfo {
    #[serde(rename = "type")]
    channel_type: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscordThreadRequest {
    name: String,
    message_id: Option<String>,
    auto_archive_minutes: Option<u16>,
}

// ---------------------------------------------------------------------------
// DiscordChannel
// ---------------------------------------------------------------------------

/// Discord channel implementation backed by the Discord Gateway WebSocket API
/// (inbound) and REST API (outbound).
pub struct DiscordChannel {
    config: DiscordConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    shutdown_tx: Option<watch::Sender<bool>>,
    http_client: reqwest::Client,
}

impl DiscordChannel {
    /// Creates a new Discord channel.
    pub fn new(config: DiscordConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "discord".to_string(),
            allowlist: config.allow_from.clone(),
            deny_by_default: config.deny_by_default,
        };

        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
            http_client: reqwest::Client::new(),
        }
    }

    /// Returns a reference to the Discord configuration.
    pub fn discord_config(&self) -> &DiscordConfig {
        &self.config
    }

    /// Returns whether the channel is enabled in configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    // -----------------------------------------------------------------------
    // Gateway URL acquisition
    // -----------------------------------------------------------------------

    /// Fetches the Gateway WebSocket URL from the Discord REST API.
    async fn fetch_gateway_url(client: &reqwest::Client, token: &str) -> Result<String> {
        let response = client
            .get(DISCORD_GATEWAY_URL)
            .header("Authorization", format!("Bot {}", token))
            .send()
            .await
            .map_err(|e| {
                ZeptoError::Channel(format!("Failed to fetch Discord Gateway URL: {}", e))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ZeptoError::Channel(format!("Failed to read Discord Gateway response: {}", e))
        })?;

        if !status.is_success() {
            return Err(ZeptoError::Channel(format!(
                "Discord Gateway HTTP {}: {}",
                status, body
            )));
        }

        let parsed: GatewayResponse = serde_json::from_str(&body).map_err(|e| {
            ZeptoError::Channel(format!("Invalid Discord Gateway response JSON: {}", e))
        })?;

        let url = parsed.url.trim().to_string();
        if url.is_empty() {
            return Err(ZeptoError::Channel(
                "Discord Gateway response missing URL".to_string(),
            ));
        }

        // Append gateway version and encoding query params.
        Ok(format!("{}/?v=10&encoding=json", url))
    }

    // -----------------------------------------------------------------------
    // Gateway payload helpers
    // -----------------------------------------------------------------------

    /// Builds the IDENTIFY payload (opcode 2).
    fn build_identify_payload(token: &str) -> String {
        json!({
            "op": 2,
            "d": {
                "token": token,
                "intents": GATEWAY_INTENTS,
                "properties": {
                    "os": std::env::consts::OS,
                    "browser": "zeptoclaw",
                    "device": "zeptoclaw"
                }
            }
        })
        .to_string()
    }

    /// Builds a heartbeat payload (opcode 1).
    fn build_heartbeat_payload(sequence: Option<u64>) -> String {
        json!({
            "op": 1,
            "d": sequence
        })
        .to_string()
    }

    /// Extracts the heartbeat interval (ms) from a HELLO payload.
    fn extract_heartbeat_interval(data: &Value) -> Result<u64> {
        let hello: HelloData = serde_json::from_value(data.clone())
            .map_err(|e| ZeptoError::Channel(format!("Invalid Discord HELLO payload: {}", e)))?;
        Ok(hello.heartbeat_interval)
    }

    /// Parses a MESSAGE_CREATE dispatch event into an `InboundMessage`,
    /// returning `None` if the message should be ignored (bot author, empty
    /// content, disallowed user, etc.).
    fn parse_message_create(
        data: &Value,
        allowlist: &[String],
        deny_by_default: bool,
    ) -> Option<InboundMessage> {
        let msg: MessageCreateData = serde_json::from_value(data.clone()).ok()?;

        // Ignore bot messages.
        if msg.author.bot.unwrap_or(false) {
            return None;
        }

        let content = msg.content.trim().to_string();
        if content.is_empty() {
            return None;
        }

        let sender_id = msg.author.id.trim().to_string();
        if sender_id.is_empty() {
            return None;
        }

        // Allowlist check with deny_by_default support.
        let allowed = if allowlist.is_empty() {
            !deny_by_default
        } else {
            allowlist.contains(&sender_id)
        };
        if !allowed {
            info!(
                "Discord: user {} not in allowlist, ignoring message",
                sender_id
            );
            return None;
        }

        let channel_id = msg.channel_id.trim().to_string();
        if channel_id.is_empty() {
            return None;
        }

        let inbound = InboundMessage::new("discord", &sender_id, &channel_id, &content)
            .with_metadata("discord_message_id", &msg.id);

        Some(inbound)
    }

    /// Calculates the exponential backoff delay for a given attempt number.
    fn backoff_delay(attempt: u32) -> Duration {
        let delay_secs = BASE_RECONNECT_DELAY_SECS
            .saturating_mul(2u64.saturating_pow(attempt))
            .min(MAX_RECONNECT_DELAY_SECS);
        Duration::from_secs(delay_secs)
    }

    // -----------------------------------------------------------------------
    // Outbound payload construction
    // -----------------------------------------------------------------------

    /// Builds the JSON body for a channel message POST request.
    fn build_send_payload(msg: &OutboundMessage) -> Result<Value> {
        let channel_id = msg.chat_id.trim();
        if channel_id.is_empty() {
            return Err(ZeptoError::Channel(
                "Discord channel ID cannot be empty".to_string(),
            ));
        }

        // Truncate content to Discord's max length using UTF-8-safe preview.
        // Append an ellipsis only when the original content actually exceeds
        // `DISCORD_MAX_MESSAGE_LENGTH` characters. This preserves messages
        // that are exactly at the limit (they should not end with "...").
        let content = if msg.content.chars().count() > DISCORD_MAX_MESSAGE_LENGTH {
            crate::utils::string::preview(
                &msg.content,
                DISCORD_MAX_MESSAGE_LENGTH.saturating_sub(3),
            )
        } else {
            msg.content.to_string()
        };

        let mut payload = json!({ "content": content });

        // If replying to a specific message, attach a message_reference.
        if let Some(ref reply_id) = msg.reply_to {
            if let Some(map) = payload.as_object_mut() {
                map.insert(
                    "message_reference".to_string(),
                    json!({ "message_id": reply_id }),
                );
            }
        }

        Ok(payload)
    }

    fn parse_discord_thread_request(msg: &OutboundMessage) -> Result<Option<DiscordThreadRequest>> {
        let thread_name = msg
            .metadata
            .get("discord_thread_name")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let message_id = msg
            .metadata
            .get("discord_thread_message_id")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let auto_archive_minutes = match msg
            .metadata
            .get("discord_thread_auto_archive_minutes")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Some(raw.parse::<u16>().map_err(|_| {
                ZeptoError::Channel(format!(
                    "Invalid discord_thread_auto_archive_minutes '{}'",
                    raw
                ))
            })?),
            None => None,
        };

        if thread_name.is_none() && message_id.is_none() && auto_archive_minutes.is_none() {
            return Ok(None);
        }

        let name = thread_name.ok_or_else(|| {
            ZeptoError::Channel("Missing discord_thread_name for Discord thread create".to_string())
        })?;
        Ok(Some(DiscordThreadRequest {
            name,
            message_id,
            auto_archive_minutes,
        }))
    }

    fn build_create_thread_payload(
        req: &DiscordThreadRequest,
        content: &str,
        forum_like: bool,
    ) -> Value {
        let mut payload = json!({ "name": req.name });
        if let Some(minutes) = req.auto_archive_minutes {
            if let Some(map) = payload.as_object_mut() {
                map.insert("auto_archive_duration".to_string(), json!(minutes));
            }
        }
        if forum_like {
            let starter = content.trim();
            let starter_content = if starter.is_empty() {
                &req.name
            } else {
                starter
            };
            if let Some(map) = payload.as_object_mut() {
                map.insert("message".to_string(), json!({ "content": starter_content }));
            }
        }
        payload
    }

    async fn fetch_channel_type(&self, token: &str, channel_id: &str) -> Result<Option<u8>> {
        let url = format!("{}/channels/{}", DISCORD_API_BASE, channel_id);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bot {}", token))
            .send()
            .await
            .map_err(|e| {
                ZeptoError::Channel(format!(
                    "Failed to fetch Discord channel metadata for thread create: {}",
                    e
                ))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ZeptoError::Channel(format!("Failed to read Discord channel response: {}", e))
        })?;

        if !status.is_success() {
            warn!(
                "Discord channel lookup returned HTTP {} while creating thread: {}",
                status, body
            );
            return Ok(None);
        }

        let parsed: DiscordChannelInfo = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(e) => {
                warn!("Discord channel lookup JSON parse failed: {}", e);
                return Ok(None);
            }
        };
        Ok(Some(parsed.channel_type))
    }

    async fn create_thread(
        &self,
        token: &str,
        channel_id: &str,
        req: &DiscordThreadRequest,
        content: &str,
    ) -> Result<()> {
        let is_forum_like = if req.message_id.is_none() {
            matches!(
                self.fetch_channel_type(token, channel_id).await?,
                Some(channel_type)
                    if channel_type == DISCORD_CHANNEL_TYPE_GUILD_FORUM
                        || channel_type == DISCORD_CHANNEL_TYPE_GUILD_MEDIA
            )
        } else {
            false
        };

        let payload = Self::build_create_thread_payload(req, content, is_forum_like);
        let url = if let Some(message_id) = req.message_id.as_deref() {
            format!(
                "{}/channels/{}/messages/{}/threads",
                DISCORD_API_BASE, channel_id, message_id
            )
        } else {
            format!("{}/channels/{}/threads", DISCORD_API_BASE, channel_id)
        };

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bot {}", token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ZeptoError::Channel(format!("Failed to call Discord thread create API: {}", e))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ZeptoError::Channel(format!(
                "Failed to read Discord thread create response: {}",
                e
            ))
        })?;
        if !status.is_success() {
            return Err(ZeptoError::Channel(format!(
                "Discord thread create API returned HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Gateway event loop
    // -----------------------------------------------------------------------

    /// Main gateway loop: connects, identifies, heartbeats, and dispatches.
    /// Reconnects with exponential backoff on any disconnect.
    async fn run_gateway_loop(
        client: reqwest::Client,
        token: String,
        bus: Arc<MessageBus>,
        allowlist: Vec<String>,
        deny_by_default: bool,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let mut reconnect_attempt: u32 = 0;

        loop {
            // Check shutdown before each connection attempt.
            if *shutdown_rx.borrow() {
                info!("Discord gateway shutdown requested");
                return;
            }

            // --- Fetch gateway URL ---
            let ws_url = tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Discord gateway shutdown requested");
                    return;
                }
                result = Self::fetch_gateway_url(&client, &token) => {
                    match result {
                        Ok(url) => url,
                        Err(e) => {
                            warn!("Discord: failed to fetch gateway URL: {}", e);
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

            // --- WebSocket connect ---
            let ws_stream = tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Discord gateway shutdown requested");
                    return;
                }
                result = connect_async(&ws_url) => {
                    match result {
                        Ok((stream, _)) => stream,
                        Err(e) => {
                            warn!("Discord: WebSocket connect failed: {}", e);
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

            info!("Discord gateway WebSocket connected");
            reconnect_attempt = 0;

            let (mut ws_writer, mut ws_reader) = ws_stream.split();

            // --- Wait for HELLO (opcode 10) ---
            let heartbeat_interval = loop {
                let next = tokio::select! {
                    _ = shutdown_rx.changed() => {
                        info!("Discord gateway shutdown requested");
                        return;
                    }
                    msg = ws_reader.next() => msg,
                };

                match next {
                    Some(Ok(WsMessage::Text(raw))) => {
                        match serde_json::from_str::<GatewayPayload>(&raw) {
                            Ok(payload) if payload.op == 10 => {
                                if let Some(ref data) = payload.d {
                                    match Self::extract_heartbeat_interval(data) {
                                        Ok(interval) => {
                                            debug!(
                                                "Discord HELLO: heartbeat_interval = {}ms",
                                                interval
                                            );
                                            break interval;
                                        }
                                        Err(e) => {
                                            warn!("Discord: invalid HELLO data: {}", e);
                                            break 41250; // fallback default
                                        }
                                    }
                                } else {
                                    warn!("Discord: HELLO without data, using default interval");
                                    break 41250;
                                }
                            }
                            Ok(_) => {
                                debug!("Discord: ignoring pre-HELLO payload");
                            }
                            Err(e) => {
                                debug!("Discord: failed to parse pre-HELLO payload: {}", e);
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        warn!("Discord: WebSocket error waiting for HELLO: {}", e);
                        break 0; // will trigger reconnect below
                    }
                    None => {
                        warn!("Discord: WebSocket closed before HELLO");
                        break 0;
                    }
                }
            };

            // If heartbeat_interval is 0, something went wrong -- reconnect.
            if heartbeat_interval == 0 {
                let delay = Self::backoff_delay(reconnect_attempt);
                reconnect_attempt = (reconnect_attempt + 1).min(MAX_RECONNECT_ATTEMPTS);
                tokio::select! {
                    _ = shutdown_rx.changed() => return,
                    _ = tokio::time::sleep(delay) => continue,
                }
            }

            // --- Send IDENTIFY (opcode 2) ---
            let identify = Self::build_identify_payload(&token);
            if let Err(e) = ws_writer.send(WsMessage::Text(identify)).await {
                warn!("Discord: failed to send IDENTIFY: {}", e);
                let delay = Self::backoff_delay(reconnect_attempt);
                reconnect_attempt = (reconnect_attempt + 1).min(MAX_RECONNECT_ATTEMPTS);
                tokio::select! {
                    _ = shutdown_rx.changed() => return,
                    _ = tokio::time::sleep(delay) => continue,
                }
            }

            // --- Spawn heartbeat task ---
            let sequence = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let sequence_valid = Arc::new(AtomicBool::new(false));
            let heartbeat_shutdown = shutdown_rx.clone();

            let seq_clone = Arc::clone(&sequence);
            let seq_valid_clone = Arc::clone(&sequence_valid);
            let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<String>(16);

            tokio::spawn({
                let mut shutdown = heartbeat_shutdown;
                async move {
                    let interval = Duration::from_millis(heartbeat_interval);
                    loop {
                        tokio::select! {
                            _ = shutdown.changed() => {
                                debug!("Discord heartbeat task shutting down");
                                return;
                            }
                            _ = tokio::time::sleep(interval) => {
                                let s = if seq_valid_clone.load(Ordering::SeqCst) {
                                    Some(seq_clone.load(Ordering::SeqCst))
                                } else {
                                    None
                                };
                                let payload = Self::build_heartbeat_payload(s);
                                if heartbeat_tx.send(payload).await.is_err() {
                                    debug!("Discord heartbeat channel closed");
                                    return;
                                }
                            }
                        }
                    }
                }
            });

            // --- Main dispatch loop ---
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        info!("Discord gateway shutdown requested");
                        return;
                    }

                    // Forward heartbeat payloads to the WebSocket writer.
                    hb = heartbeat_rx.recv() => {
                        match hb {
                            Some(payload) => {
                                if let Err(e) = ws_writer.send(WsMessage::Text(payload)).await {
                                    warn!("Discord: heartbeat send failed: {}", e);
                                    break;
                                }
                            }
                            None => {
                                debug!("Discord heartbeat channel closed");
                                break;
                            }
                        }
                    }

                    // Process incoming gateway events.
                    msg = ws_reader.next() => {
                        match msg {
                            Some(Ok(WsMessage::Text(raw))) => {
                                match serde_json::from_str::<GatewayPayload>(&raw) {
                                    Ok(payload) => {
                                        // Track sequence number for heartbeats.
                                        if let Some(s) = payload.s {
                                            sequence.store(s, Ordering::SeqCst);
                                            sequence_valid.store(true, Ordering::SeqCst);
                                        }

                                        match payload.op {
                                            // DISPATCH
                                            0 => {
                                                if let Some(event_name) = payload.t.as_deref() {
                                                    if event_name == "MESSAGE_CREATE" {
                                                        if let Some(ref data) = payload.d {
                                                            if let Some(inbound) =
                                                                Self::parse_message_create(data, &allowlist, deny_by_default)
                                                            {
                                                                if let Err(e) =
                                                                    bus.publish_inbound(inbound).await
                                                                {
                                                                    error!(
                                                                        "Failed to publish Discord inbound message: {}",
                                                                        e
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    } else if event_name == "READY" {
                                                        info!("Discord gateway READY");
                                                    } else {
                                                        debug!("Discord: ignoring event {}", event_name);
                                                    }
                                                }
                                            }
                                            // HEARTBEAT request from server
                                            1 => {
                                                let s = if sequence_valid.load(Ordering::SeqCst) {
                                                    Some(sequence.load(Ordering::SeqCst))
                                                } else {
                                                    None
                                                };
                                                let hb = Self::build_heartbeat_payload(s);
                                                if let Err(e) = ws_writer.send(WsMessage::Text(hb)).await {
                                                    warn!("Discord: heartbeat response send failed: {}", e);
                                                    break;
                                                }
                                            }
                                            // RECONNECT
                                            7 => {
                                                info!("Discord: server requested reconnect");
                                                break;
                                            }
                                            // INVALID SESSION
                                            9 => {
                                                warn!("Discord: invalid session, reconnecting");
                                                break;
                                            }
                                            // HEARTBEAT ACK
                                            11 => {
                                                debug!("Discord: heartbeat ACK received");
                                            }
                                            _ => {
                                                debug!("Discord: unhandled opcode {}", payload.op);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        debug!("Discord: failed to parse gateway payload: {}", e);
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Ping(payload))) => {
                                if let Err(e) = ws_writer.send(WsMessage::Pong(payload)).await {
                                    warn!("Discord: pong send failed: {}", e);
                                    break;
                                }
                            }
                            Some(Ok(WsMessage::Close(frame))) => {
                                info!("Discord: WebSocket closed by server: {:?}", frame);
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                warn!("Discord: WebSocket stream error: {}", e);
                                break;
                            }
                            None => {
                                warn!("Discord: WebSocket stream ended");
                                break;
                            }
                        }
                    }
                }
            }

            // --- Wait before reconnecting ---
            let delay = Self::backoff_delay(reconnect_attempt);
            reconnect_attempt = (reconnect_attempt + 1).min(MAX_RECONNECT_ATTEMPTS);
            info!("Discord: reconnecting in {} seconds", delay.as_secs());
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
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("Discord channel already running");
            return Ok(());
        }

        if !self.config.enabled {
            warn!("Discord channel is disabled in configuration");
            self.running.store(false, Ordering::SeqCst);
            return Ok(());
        }

        let token = self.config.token.trim().to_string();
        if token.is_empty() {
            self.running.store(false, Ordering::SeqCst);
            return Err(ZeptoError::Config("Discord bot token is empty".to_string()));
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        info!("Starting Discord channel with Gateway WebSocket");
        let running_clone = Arc::clone(&self.running);
        let http_client = self.http_client.clone();
        let bus = Arc::clone(&self.bus);
        let allow_from = self.config.allow_from.clone();
        let deny_by_default = self.config.deny_by_default;
        tokio::spawn(async move {
            Self::run_gateway_loop(
                http_client,
                token,
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
            info!("Discord channel already stopped");
            return Ok(());
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }

        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ZeptoError::Channel(
                "Discord channel not running".to_string(),
            ));
        }

        let token = self.config.token.trim();
        if token.is_empty() {
            return Err(ZeptoError::Config("Discord bot token is empty".to_string()));
        }

        let channel_id = msg.chat_id.trim();
        if channel_id.is_empty() {
            return Err(ZeptoError::Channel(
                "Discord channel ID cannot be empty".to_string(),
            ));
        }

        if let Some(thread_req) = Self::parse_discord_thread_request(&msg)? {
            self.create_thread(token, channel_id, &thread_req, &msg.content)
                .await?;
            info!("Discord: thread created successfully");
            return Ok(());
        }

        let payload = Self::build_send_payload(&msg)?;
        let url = format!("{}/channels/{}/messages", DISCORD_API_BASE, channel_id);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bot {}", token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Failed to call Discord API: {}", e)))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ZeptoError::Channel(format!("Failed to read Discord API response: {}", e))
        })?;

        if !status.is_success() {
            return Err(ZeptoError::Channel(format!(
                "Discord API returned HTTP {}: {}",
                status, body
            )));
        }

        info!("Discord: message sent successfully");
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

    fn test_config() -> DiscordConfig {
        DiscordConfig {
            enabled: true,
            token: "test-bot-token".to_string(),
            allow_from: vec!["123456789".to_string()],
            ..Default::default()
        }
    }

    // -----------------------------------------------------------------------
    // 1. Channel name
    // -----------------------------------------------------------------------
    #[test]
    fn test_channel_name() {
        let channel = DiscordChannel::new(test_config(), test_bus());
        assert_eq!(channel.name(), "discord");
    }

    // -----------------------------------------------------------------------
    // 2. Config initialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_config_initialization() {
        let config = DiscordConfig {
            enabled: true,
            token: "my-token".to_string(),
            allow_from: vec!["U1".to_string(), "U2".to_string()],
            ..Default::default()
        };
        let channel = DiscordChannel::new(config, test_bus());

        assert!(channel.is_enabled());
        assert_eq!(channel.discord_config().token, "my-token");
        assert_eq!(channel.discord_config().allow_from.len(), 2);
        assert!(!channel.is_running());
    }

    // -----------------------------------------------------------------------
    // 3. is_allowed delegation
    // -----------------------------------------------------------------------
    #[test]
    fn test_is_allowed_delegation() {
        let channel = DiscordChannel::new(test_config(), test_bus());

        assert!(channel.is_allowed("123456789"));
        assert!(!channel.is_allowed("999999999"));
    }

    #[test]
    fn test_is_allowed_empty_allowlist() {
        let config = DiscordConfig {
            enabled: true,
            token: "tok".to_string(),
            allow_from: vec![],
            ..Default::default()
        };
        let channel = DiscordChannel::new(config, test_bus());

        assert!(channel.is_allowed("anyone"));
        assert!(channel.is_allowed("literally_anyone"));
    }

    // -----------------------------------------------------------------------
    // 4. Gateway URL parsing
    // -----------------------------------------------------------------------
    #[test]
    fn test_gateway_url_formatting() {
        // Verify the URL we build appends query params correctly.
        let base = "wss://gateway.discord.gg";
        let formatted = format!("{}/?v=10&encoding=json", base);
        assert_eq!(formatted, "wss://gateway.discord.gg/?v=10&encoding=json");
    }

    #[test]
    fn test_gateway_response_deserialization() {
        let json = r#"{"url": "wss://gateway.discord.gg"}"#;
        let resp: GatewayResponse = serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.url, "wss://gateway.discord.gg");
    }

    // -----------------------------------------------------------------------
    // 5. MESSAGE_CREATE event deserialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_message_create_deserialization() {
        let data = json!({
            "id": "msg-001",
            "content": "Hello from Discord!",
            "channel_id": "ch-100",
            "author": {
                "id": "user-42",
                "bot": false
            }
        });

        let inbound = DiscordChannel::parse_message_create(&data, &[], false);
        assert!(inbound.is_some());
        let msg = inbound.unwrap();
        assert_eq!(msg.channel, "discord");
        assert_eq!(msg.sender_id, "user-42");
        assert_eq!(msg.chat_id, "ch-100");
        assert_eq!(msg.content, "Hello from Discord!");
        assert_eq!(
            msg.metadata.get("discord_message_id"),
            Some(&"msg-001".to_string())
        );
    }

    #[test]
    fn test_message_create_with_allowlist() {
        let data = json!({
            "id": "msg-002",
            "content": "test",
            "channel_id": "ch-200",
            "author": { "id": "allowed-user", "bot": false }
        });

        let allowed =
            DiscordChannel::parse_message_create(&data, &["allowed-user".to_string()], false);
        assert!(allowed.is_some());

        let denied =
            DiscordChannel::parse_message_create(&data, &["someone-else".to_string()], false);
        assert!(denied.is_none());
    }

    // -----------------------------------------------------------------------
    // 6. Heartbeat interval extraction from HELLO payload
    // -----------------------------------------------------------------------
    #[test]
    fn test_heartbeat_interval_extraction() {
        let data = json!({ "heartbeat_interval": 41250 });
        let interval = DiscordChannel::extract_heartbeat_interval(&data).expect("should extract");
        assert_eq!(interval, 41250);
    }

    #[test]
    fn test_heartbeat_interval_extraction_invalid() {
        let data = json!({ "something_else": 123 });
        // serde will deserialize missing field as 0 for u64, but the struct
        // has no default so this should fail.
        // Actually, since serde will error on missing required fields:
        let result = DiscordChannel::extract_heartbeat_interval(&data);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 7. Bot message filtering logic
    // -----------------------------------------------------------------------
    #[test]
    fn test_bot_message_ignored() {
        let data = json!({
            "id": "msg-003",
            "content": "I am a bot",
            "channel_id": "ch-300",
            "author": { "id": "bot-user", "bot": true }
        });

        let result = DiscordChannel::parse_message_create(&data, &[], false);
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_content_ignored() {
        let data = json!({
            "id": "msg-004",
            "content": "   ",
            "channel_id": "ch-400",
            "author": { "id": "user-1", "bot": false }
        });

        let result = DiscordChannel::parse_message_create(&data, &[], false);
        assert!(result.is_none());
    }

    #[test]
    fn test_missing_bot_field_treated_as_human() {
        let data = json!({
            "id": "msg-005",
            "content": "No bot field",
            "channel_id": "ch-500",
            "author": { "id": "user-2" }
        });

        let result = DiscordChannel::parse_message_create(&data, &[], false);
        assert!(result.is_some());
    }

    // -----------------------------------------------------------------------
    // 8. Outbound message formatting
    // -----------------------------------------------------------------------
    #[test]
    fn test_outbound_message_payload() {
        let msg = OutboundMessage::new("discord", "ch-100", "Hello back!");
        let payload = DiscordChannel::build_send_payload(&msg).expect("should build payload");

        assert_eq!(payload["content"], "Hello back!");
        assert!(payload.get("message_reference").is_none());
    }

    #[test]
    fn test_outbound_message_with_reply() {
        let msg =
            OutboundMessage::new("discord", "ch-100", "reply text").with_reply("original-msg-id");
        let payload = DiscordChannel::build_send_payload(&msg).expect("should build payload");

        assert_eq!(payload["content"], "reply text");
        assert_eq!(
            payload["message_reference"]["message_id"],
            "original-msg-id"
        );
    }

    #[test]
    fn test_outbound_empty_channel_id() {
        let msg = OutboundMessage::new("discord", "  ", "test");
        let result = DiscordChannel::build_send_payload(&msg);
        assert!(result.is_err());
    }

    #[test]
    fn test_outbound_message_truncation() {
        let long_content = "x".repeat(2500);
        let msg = OutboundMessage::new("discord", "ch-100", &long_content);
        let payload = DiscordChannel::build_send_payload(&msg).expect("should build payload");

        let content = payload["content"].as_str().unwrap();
        assert!(content.len() <= DISCORD_MAX_MESSAGE_LENGTH);
        assert!(content.ends_with("..."));
    }

    #[test]
    fn test_parse_discord_thread_request_none() {
        let msg = OutboundMessage::new("discord", "ch-100", "hello");
        let req = DiscordChannel::parse_discord_thread_request(&msg).expect("parse should succeed");
        assert!(req.is_none());
    }

    #[test]
    fn test_parse_discord_thread_request_success() {
        let msg = OutboundMessage::new("discord", "ch-100", "hello")
            .with_metadata("discord_thread_name", "Ops")
            .with_metadata("discord_thread_auto_archive_minutes", "60");
        let req = DiscordChannel::parse_discord_thread_request(&msg)
            .expect("parse should succeed")
            .expect("thread request should exist");
        assert_eq!(req.name, "Ops");
        assert_eq!(req.auto_archive_minutes, Some(60));
        assert_eq!(req.message_id, None);
    }

    #[test]
    fn test_parse_discord_thread_request_requires_name() {
        let msg = OutboundMessage::new("discord", "ch-100", "hello")
            .with_metadata("discord_thread_message_id", "m1");
        let err = DiscordChannel::parse_discord_thread_request(&msg).unwrap_err();
        assert!(err
            .to_string()
            .contains("Missing discord_thread_name for Discord thread create"));
    }

    #[test]
    fn test_build_create_thread_payload_forum_like_uses_starter_message() {
        let req = DiscordThreadRequest {
            name: "Daily".to_string(),
            message_id: None,
            auto_archive_minutes: Some(1440),
        };
        let payload = DiscordChannel::build_create_thread_payload(&req, "  ", true);
        assert_eq!(payload["name"], "Daily");
        assert_eq!(payload["auto_archive_duration"], 1440);
        assert_eq!(payload["message"]["content"], "Daily");
    }

    // -----------------------------------------------------------------------
    // 9. Running state management
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_running_state_default() {
        let channel = DiscordChannel::new(test_config(), test_bus());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_start_without_token() {
        let config = DiscordConfig {
            enabled: true,
            token: String::new(),
            allow_from: vec![],
            ..Default::default()
        };
        let mut channel = DiscordChannel::new(config, test_bus());

        let result = channel.start().await;
        assert!(result.is_err());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_start_disabled() {
        let config = DiscordConfig {
            enabled: false,
            token: "some-token".to_string(),
            allow_from: vec![],
            ..Default::default()
        };
        let mut channel = DiscordChannel::new(config, test_bus());

        let result = channel.start().await;
        assert!(result.is_ok());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_stop_not_running() {
        let mut channel = DiscordChannel::new(test_config(), test_bus());
        let result = channel.stop().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_not_running() {
        let channel = DiscordChannel::new(test_config(), test_bus());
        let msg = OutboundMessage::new("discord", "ch-100", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 10. Reconnect backoff calculation
    // -----------------------------------------------------------------------
    #[test]
    fn test_backoff_delay_increases_exponentially() {
        let d0 = DiscordChannel::backoff_delay(0);
        let d1 = DiscordChannel::backoff_delay(1);
        let d2 = DiscordChannel::backoff_delay(2);
        let d3 = DiscordChannel::backoff_delay(3);

        assert_eq!(d0, Duration::from_secs(2)); // 2 * 2^0 = 2
        assert_eq!(d1, Duration::from_secs(4)); // 2 * 2^1 = 4
        assert_eq!(d2, Duration::from_secs(8)); // 2 * 2^2 = 8
        assert_eq!(d3, Duration::from_secs(16)); // 2 * 2^3 = 16
    }

    #[test]
    fn test_backoff_delay_caps_at_max() {
        let d_high = DiscordChannel::backoff_delay(20);
        assert_eq!(d_high, Duration::from_secs(MAX_RECONNECT_DELAY_SECS));
    }

    #[test]
    fn test_backoff_delay_does_not_overflow() {
        // u64::MAX exponent should not panic.
        let d = DiscordChannel::backoff_delay(u32::MAX);
        assert_eq!(d, Duration::from_secs(MAX_RECONNECT_DELAY_SECS));
    }

    // -----------------------------------------------------------------------
    // Extra: Identify and heartbeat payload construction
    // -----------------------------------------------------------------------
    #[test]
    fn test_identify_payload_structure() {
        let payload_str = DiscordChannel::build_identify_payload("my-token");
        let payload: Value = serde_json::from_str(&payload_str).expect("valid JSON");

        assert_eq!(payload["op"], 2);
        assert_eq!(payload["d"]["token"], "my-token");
        assert_eq!(payload["d"]["intents"], GATEWAY_INTENTS);
        assert_eq!(payload["d"]["properties"]["browser"], "zeptoclaw");
    }

    #[test]
    fn test_heartbeat_payload_with_sequence() {
        let payload_str = DiscordChannel::build_heartbeat_payload(Some(42));
        let payload: Value = serde_json::from_str(&payload_str).expect("valid JSON");

        assert_eq!(payload["op"], 1);
        assert_eq!(payload["d"], 42);
    }

    #[test]
    fn test_heartbeat_payload_without_sequence() {
        let payload_str = DiscordChannel::build_heartbeat_payload(None);
        let payload: Value = serde_json::from_str(&payload_str).expect("valid JSON");

        assert_eq!(payload["op"], 1);
        assert!(payload["d"].is_null());
    }

    #[test]
    fn test_gateway_payload_deserialization_dispatch() {
        let raw = r#"{
            "op": 0,
            "s": 5,
            "t": "MESSAGE_CREATE",
            "d": {
                "id": "msg-100",
                "content": "test message",
                "channel_id": "ch-999",
                "author": { "id": "u-1", "bot": false }
            }
        }"#;

        let payload: GatewayPayload = serde_json::from_str(raw).expect("should deserialize");
        assert_eq!(payload.op, 0);
        assert_eq!(payload.s, Some(5));
        assert_eq!(payload.t, Some("MESSAGE_CREATE".to_string()));
        assert!(payload.d.is_some());
    }

    #[test]
    fn test_gateway_payload_deserialization_hello() {
        let raw = r#"{
            "op": 10,
            "d": { "heartbeat_interval": 41250 }
        }"#;

        let payload: GatewayPayload = serde_json::from_str(raw).expect("should deserialize");
        assert_eq!(payload.op, 10);
        assert!(payload.s.is_none());
        assert!(payload.t.is_none());

        let interval = DiscordChannel::extract_heartbeat_interval(payload.d.as_ref().unwrap())
            .expect("should extract");
        assert_eq!(interval, 41250);
    }

    // -----------------------------------------------------------------------
    // 11. DiscordConfig serde deserialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_discord_config_deserialize_defaults() {
        // Only provide `token`; `enabled` and `allow_from` should use serde defaults.
        let json = r#"{ "token": "bot-abc-123" }"#;
        let config: DiscordConfig = serde_json::from_str(json).expect("should parse");

        assert!(!config.enabled); // #[serde(default)] -> bool defaults to false
        assert_eq!(config.token, "bot-abc-123");
        assert!(config.allow_from.is_empty());
    }

    #[test]
    fn test_discord_config_deserialize_full() {
        let json = r#"{
            "enabled": true,
            "token": "tok-full",
            "allow_from": ["111", "222", "333"]
        }"#;
        let config: DiscordConfig = serde_json::from_str(json).expect("should parse");

        assert!(config.enabled);
        assert_eq!(config.token, "tok-full");
        assert_eq!(config.allow_from, vec!["111", "222", "333"]);
    }

    #[test]
    fn test_discord_config_default_trait() {
        let config = DiscordConfig::default();
        assert!(!config.enabled);
        assert!(config.token.is_empty());
        assert!(config.allow_from.is_empty());
    }

    // -----------------------------------------------------------------------
    // 12. Gateway payload edge cases
    // -----------------------------------------------------------------------
    #[test]
    fn test_gateway_payload_minimal_fields() {
        // Only `op` is required; `d`, `s`, `t` all have serde defaults.
        let raw = r#"{ "op": 11 }"#;
        let payload: GatewayPayload = serde_json::from_str(raw).expect("should parse");

        assert_eq!(payload.op, 11);
        assert!(payload.d.is_none());
        assert!(payload.s.is_none());
        assert!(payload.t.is_none());
    }

    #[test]
    fn test_gateway_payload_reconnect_opcode() {
        let raw = r#"{ "op": 7, "d": null }"#;
        let payload: GatewayPayload = serde_json::from_str(raw).expect("should parse");
        assert_eq!(payload.op, 7);
    }

    #[test]
    fn test_gateway_payload_invalid_session_opcode() {
        let raw = r#"{ "op": 9, "d": false }"#;
        let payload: GatewayPayload = serde_json::from_str(raw).expect("should parse");
        assert_eq!(payload.op, 9);
        assert_eq!(payload.d, Some(json!(false)));
    }

    // -----------------------------------------------------------------------
    // 13. MESSAGE_CREATE edge cases
    // -----------------------------------------------------------------------
    #[test]
    fn test_message_create_empty_author_id() {
        let data = json!({
            "id": "msg-edge-1",
            "content": "valid content",
            "channel_id": "ch-100",
            "author": { "id": "  ", "bot": false }
        });

        let result = DiscordChannel::parse_message_create(&data, &[], false);
        // Empty (whitespace-only) sender_id should be rejected.
        assert!(result.is_none());
    }

    #[test]
    fn test_message_create_missing_content_field() {
        // `content` has `#[serde(default)]`, so omitting it yields "".
        let data = json!({
            "id": "msg-edge-2",
            "channel_id": "ch-200",
            "author": { "id": "user-42", "bot": false }
        });

        let result = DiscordChannel::parse_message_create(&data, &[], false);
        // Empty content should be filtered out.
        assert!(result.is_none());
    }

    #[test]
    fn test_message_create_empty_channel_id() {
        let data = json!({
            "id": "msg-edge-3",
            "content": "hello",
            "channel_id": "  ",
            "author": { "id": "user-42", "bot": false }
        });

        let result = DiscordChannel::parse_message_create(&data, &[], false);
        // Empty (whitespace-only) channel_id should be rejected.
        assert!(result.is_none());
    }

    #[test]
    fn test_message_create_content_trimmed() {
        let data = json!({
            "id": "msg-trim",
            "content": "  padded message  ",
            "channel_id": "ch-100",
            "author": { "id": "user-1" }
        });

        let inbound = DiscordChannel::parse_message_create(&data, &[], false).unwrap();
        assert_eq!(inbound.content, "padded message");
    }

    // -----------------------------------------------------------------------
    // 14. HelloData direct deserialization
    // -----------------------------------------------------------------------
    #[test]
    fn test_hello_data_deserialization() {
        let data: HelloData = serde_json::from_value(json!({
            "heartbeat_interval": 45000
        }))
        .expect("should parse");
        assert_eq!(data.heartbeat_interval, 45000);
    }

    #[test]
    fn test_hello_data_extra_fields_ignored() {
        // Serde should ignore unknown fields by default (deny_unknown_fields not set).
        let data: HelloData = serde_json::from_value(json!({
            "heartbeat_interval": 30000,
            "_trace": ["gateway-1"]
        }))
        .expect("should parse with extra fields");
        assert_eq!(data.heartbeat_interval, 30000);
    }

    // -----------------------------------------------------------------------
    // 15. Outbound truncation boundary
    // -----------------------------------------------------------------------
    #[test]
    fn test_outbound_message_exactly_at_limit() {
        // A message of exactly DISCORD_MAX_MESSAGE_LENGTH should NOT be truncated.
        let exact_content = "a".repeat(DISCORD_MAX_MESSAGE_LENGTH);
        let msg = OutboundMessage::new("discord", "ch-100", &exact_content);
        let payload = DiscordChannel::build_send_payload(&msg).expect("should build");

        let content = payload["content"].as_str().unwrap();
        assert_eq!(content.len(), DISCORD_MAX_MESSAGE_LENGTH);
        assert!(!content.ends_with("..."));
    }

    #[test]
    fn test_outbound_message_one_over_limit() {
        // A message of DISCORD_MAX_MESSAGE_LENGTH + 1 SHOULD be truncated.
        let over_content = "b".repeat(DISCORD_MAX_MESSAGE_LENGTH + 1);
        let msg = OutboundMessage::new("discord", "ch-100", &over_content);
        let payload = DiscordChannel::build_send_payload(&msg).expect("should build");

        let content = payload["content"].as_str().unwrap();
        assert!(content.len() <= DISCORD_MAX_MESSAGE_LENGTH);
        assert!(content.ends_with("..."));
    }

    // -----------------------------------------------------------------------
    // 16. Gateway intents bitmask sanity
    // -----------------------------------------------------------------------
    #[test]
    fn test_gateway_intents_bitmask() {
        // GUILDS (1 << 0) = 1
        // GUILD_MESSAGES (1 << 9) = 512
        // DIRECT_MESSAGES (1 << 12) = 4096
        // MESSAGE_CONTENT (1 << 15) = 32768
        // Total = 37377
        assert_eq!(GATEWAY_INTENTS, 1 + 512 + 4096 + 32768);
        assert_eq!(GATEWAY_INTENTS, 37377);
    }

    // -----------------------------------------------------------------------
    // 17. Identify payload includes OS
    // -----------------------------------------------------------------------
    #[test]
    fn test_identify_payload_has_os() {
        let payload_str = DiscordChannel::build_identify_payload("tok");
        let payload: Value = serde_json::from_str(&payload_str).expect("valid JSON");

        let os_val = payload["d"]["properties"]["os"]
            .as_str()
            .expect("os field should be a string");
        // The OS field should be populated with the compile-time constant.
        assert_eq!(os_val, std::env::consts::OS);
        assert!(!os_val.is_empty());
    }
}
