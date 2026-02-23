//! Lark / Feishu channel implementation.
//!
//! Receives events via the Lark WS long-connection (pbbp2 protobuf frames).
//! No public HTTPS endpoint is required — only an App ID and App Secret.
//!
//! # Endpoints
//!
//! | Region  | REST API base                           | WS endpoint base          |
//! |---------|-----------------------------------------|---------------------------|
//! | Lark    | `https://open.larksuite.com/open-apis`  | `https://open.larksuite.com` |
//! | Feishu  | `https://open.feishu.cn/open-apis`      | `https://open.feishu.cn`     |
//!
//! # WS frame format (pbbp2.proto)
//!
//! All binary frames are protobuf-encoded `PbFrame` messages:
//! - `method=0` → CONTROL (ping / pong)
//! - `method=1` → DATA (events)
//!
//! DATA frames carry a JSON event payload in `frame.payload`.
//!
//! # Reconnect strategy
//!
//! The `start()` loop reconnects with exponential back-off (2 s … 60 s) on
//! any connection failure or heartbeat timeout.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::LarkConfig;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";
const FEISHU_WS_BASE: &str = "https://open.feishu.cn";
const LARK_API_BASE: &str = "https://open.larksuite.com/open-apis";
const LARK_WS_BASE: &str = "https://open.larksuite.com";

/// If no binary frame arrives within this window the connection is considered dead.
const WS_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(300);

/// Refresh tenant token this many seconds before it expires.
const TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(120);

/// Fallback TTL when the token response does not include an expiry.
const DEFAULT_TOKEN_TTL: Duration = Duration::from_secs(7200);

/// Lark business error code for an expired/invalid tenant access token.
const LARK_INVALID_ACCESS_TOKEN_CODE: i64 = 99_991_663;

/// Base reconnect delay in seconds.
const BASE_RECONNECT_DELAY_SECS: u64 = 2;

/// Maximum reconnect delay in seconds.
const MAX_RECONNECT_DELAY_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// pbbp2.proto frame types
// ---------------------------------------------------------------------------

/// A key-value header carried inside a `PbFrame`.
#[derive(Clone, PartialEq, prost::Message)]
struct PbHeader {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

/// Top-level Lark / Feishu WS frame (pbbp2.proto).
///
/// `method=0` → CONTROL (ping / pong)  
/// `method=1` → DATA (events)
#[derive(Clone, PartialEq, prost::Message)]
struct PbFrame {
    #[prost(uint64, tag = "1")]
    pub seq_id: u64,
    #[prost(uint64, tag = "2")]
    pub log_id: u64,
    #[prost(int32, tag = "3")]
    pub service: i32,
    #[prost(int32, tag = "4")]
    pub method: i32,
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<PbHeader>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub payload: Option<Vec<u8>>,
}

impl PbFrame {
    /// Returns the value of the first header whose key matches `key`, or `""`.
    fn header_value<'a>(&'a self, key: &str) -> &'a str {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
            .unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// REST response types
// ---------------------------------------------------------------------------

/// Server-sent client configuration (from pong payload).
#[derive(Debug, serde::Deserialize, Default, Clone)]
struct WsClientConfig {
    #[serde(rename = "PingInterval")]
    ping_interval: Option<u64>,
}

/// POST /callback/ws/endpoint → this envelope.
#[derive(Debug, serde::Deserialize)]
struct WsEndpointResp {
    code: i32,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<WsEndpoint>,
}

#[derive(Debug, serde::Deserialize)]
struct WsEndpoint {
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "ClientConfig")]
    client_config: Option<WsClientConfig>,
}

// ---------------------------------------------------------------------------
// Cached token state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    refresh_after: Instant,
}

// ---------------------------------------------------------------------------
// Helper free functions
// ---------------------------------------------------------------------------

fn extract_token_ttl(body: &serde_json::Value) -> u64 {
    body.get("expire")
        .or_else(|| body.get("expires_in"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
        })
        .unwrap_or(DEFAULT_TOKEN_TTL.as_secs())
        .max(1)
}

fn token_refresh_deadline(now: Instant, ttl_secs: u64) -> Instant {
    let ttl = Duration::from_secs(ttl_secs.max(1));
    let refresh_in = ttl
        .checked_sub(TOKEN_REFRESH_SKEW)
        .unwrap_or(Duration::from_secs(1));
    now + refresh_in
}

fn is_invalid_token_response(body: &serde_json::Value) -> bool {
    body.get("code").and_then(|c| c.as_i64()) == Some(LARK_INVALID_ACCESS_TOKEN_CODE)
}

fn should_refresh_token(status: reqwest::StatusCode, body: &serde_json::Value) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || is_invalid_token_response(body)
}

/// Returns `true` for binary / ping / pong frames that reset the heartbeat watchdog.
fn is_live_ws_frame(msg: &WsMsg) -> bool {
    matches!(msg, WsMsg::Binary(_) | WsMsg::Ping(_) | WsMsg::Pong(_))
}

/// Flatten a Feishu `post` rich-text message to plain text.
fn parse_post_content(content: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let locale = parsed
        .get("zh_cn")
        .or_else(|| parsed.get("en_us"))
        .or_else(|| {
            parsed
                .as_object()
                .and_then(|m| m.values().find(|v| v.is_object()))
        })?;

    let mut text = String::new();

    if let Some(title) = locale
        .get("title")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
    {
        text.push_str(title);
        text.push_str("\n\n");
    }

    if let Some(paragraphs) = locale.get("content").and_then(|c| c.as_array()) {
        for para in paragraphs {
            if let Some(elements) = para.as_array() {
                for el in elements {
                    match el.get("tag").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            if let Some(t) = el.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                            }
                        }
                        "a" => {
                            text.push_str(
                                el.get("text")
                                    .and_then(|t| t.as_str())
                                    .filter(|s| !s.is_empty())
                                    .or_else(|| el.get("href").and_then(|h| h.as_str()))
                                    .unwrap_or(""),
                            );
                        }
                        "at" => {
                            let n = el
                                .get("user_name")
                                .and_then(|n| n.as_str())
                                .or_else(|| el.get("user_id").and_then(|i| i.as_str()))
                                .unwrap_or("user");
                            text.push('@');
                            text.push_str(n);
                        }
                        _ => {}
                    }
                }
                text.push('\n');
            }
        }
    }

    let result = text.trim().to_string();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Remove `@_user_N` placeholder tokens injected by Feishu in group chats.
fn strip_at_placeholders(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '@' {
            let rest: String = chars.clone().map(|(_, c)| c).collect();
            if let Some(after) = rest.strip_prefix("_user_") {
                let skip =
                    "_user_".len() + after.chars().take_while(|c| c.is_ascii_digit()).count();
                for _ in 0..skip {
                    chars.next();
                }
                if chars.peek().map(|(_, c)| *c == ' ').unwrap_or(false) {
                    chars.next();
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

/// In group chats only respond when the bot is @-mentioned.
fn should_respond_in_group(mentions: &[serde_json::Value]) -> bool {
    !mentions.is_empty()
}

/// Fetch and cache a Lark tenant access token.
///
/// Checks the cache first; only hits the network when the token is missing
/// or within `TOKEN_REFRESH_SKEW` of expiry.  Both `get_tenant_token` and
/// the fire-and-forget reaction task share this helper so the token-fetch
/// logic lives in exactly one place.
async fn fetch_tenant_token_cached(
    api_base: &'static str,
    app_id: &str,
    app_secret: &str,
    cache: &Arc<RwLock<Option<CachedToken>>>,
) -> anyhow::Result<String> {
    // Fast path: cached and still fresh
    {
        let guard = cache.read().await;
        if let Some(ref tok) = *guard {
            if Instant::now() < tok.refresh_after {
                return Ok(tok.value.clone());
            }
        }
    }

    // Fetch a new token
    let url = format!("{}/auth/v3/tenant_access_token/internal", api_base);
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default()
        .post(&url)
        .json(&serde_json::json!({
            "app_id": app_id,
            "app_secret": app_secret,
        }))
        .send()
        .await?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await?;

    if !status.is_success() {
        anyhow::bail!("Lark token request failed: status={status}, body={body}");
    }

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("Lark tenant_access_token error: {msg}");
    }

    let token = body
        .get("tenant_access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing tenant_access_token in response"))?
        .to_string();

    let ttl = extract_token_ttl(&body);
    let refresh_after = token_refresh_deadline(Instant::now(), ttl);

    {
        let mut guard = cache.write().await;
        *guard = Some(CachedToken {
            value: token.clone(),
            refresh_after,
        });
    }

    Ok(token)
}

// ---------------------------------------------------------------------------
// LarkChannel
// ---------------------------------------------------------------------------

/// Lark / Feishu messaging channel.
///
/// Uses the Lark WS long-connection protocol (pbbp2 protobuf over WSS) to
/// receive events without requiring a publicly accessible HTTPS endpoint.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use zeptoclaw::bus::MessageBus;
/// use zeptoclaw::config::LarkConfig;
/// use zeptoclaw::channels::LarkChannel;
///
/// let config = LarkConfig {
///     enabled: true,
///     app_id: "cli_app123".into(),
///     app_secret: "secret".into(),
///     feishu: false,
///     ..Default::default()
/// };
/// let bus = Arc::new(MessageBus::new());
/// let channel = LarkChannel::new(config, bus);
/// ```
pub struct LarkChannel {
    config: LarkConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    /// Cached tenant access token with proactive refresh metadata.
    tenant_token: Arc<RwLock<Option<CachedToken>>>,
    /// Dedup set: WS message_ids seen in the last ~30 min.
    ws_seen_ids: Arc<RwLock<HashMap<String, Instant>>>,
}

impl LarkChannel {
    /// Creates a new `LarkChannel` from the given config and message bus.
    pub fn new(config: LarkConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: if config.feishu {
                "feishu".to_string()
            } else {
                "lark".to_string()
            },
            allowlist: config.allowed_senders.clone(),
            deny_by_default: config.deny_by_default,
        };
        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            tenant_token: Arc::new(RwLock::new(None)),
            ws_seen_ids: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // ------------------------------------------------------------------
    // Region helpers
    // ------------------------------------------------------------------

    /// REST API base URL based on the `feishu` flag.
    pub fn api_base(&self) -> &'static str {
        if self.config.feishu {
            FEISHU_API_BASE
        } else {
            LARK_API_BASE
        }
    }

    /// WS endpoint base URL based on the `feishu` flag.
    pub fn ws_base(&self) -> &'static str {
        if self.config.feishu {
            FEISHU_WS_BASE
        } else {
            LARK_WS_BASE
        }
    }

    // ------------------------------------------------------------------
    // Access control
    // ------------------------------------------------------------------

    /// Returns `true` if `sender_open_id` is allowed to send messages.
    ///
    /// Always drops the bot's own messages (when `bot_open_id` is set).
    /// Then checks the allowlist (empty + `deny_by_default=false` → allow all).
    pub fn is_sender_allowed(&self, sender_open_id: &str) -> bool {
        // Drop bot self-messages
        if let Some(ref bot_id) = self.config.bot_open_id {
            if sender_open_id == bot_id {
                return false;
            }
        }
        self.base_config.is_allowed(sender_open_id)
    }

    // ------------------------------------------------------------------
    // Tenant token
    // ------------------------------------------------------------------

    /// Returns a valid tenant access token, refreshing from the API if needed.
    async fn get_tenant_token(&self) -> anyhow::Result<String> {
        fetch_tenant_token_cached(
            self.api_base(),
            &self.config.app_id,
            &self.config.app_secret,
            &self.tenant_token,
        )
        .await
    }

    /// Invalidate the cached tenant token (called on 401 / business code 99991663).
    async fn invalidate_token(&self) {
        let mut guard = self.tenant_token.write().await;
        *guard = None;
    }

    // ------------------------------------------------------------------
    // Outbound send helper
    // ------------------------------------------------------------------

    async fn send_text_once(
        &self,
        url: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::json!({ "raw": raw }));
        Ok((status, parsed))
    }

    // ------------------------------------------------------------------
    // WS long-connection
    // ------------------------------------------------------------------

    /// Obtain the WSS URL from Lark's `/callback/ws/endpoint` API.
    async fn get_ws_endpoint(&self) -> anyhow::Result<(String, WsClientConfig)> {
        let url = format!("{}/callback/ws/endpoint", self.ws_base());
        let resp = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
            .post(&url)
            .header("locale", if self.config.feishu { "zh" } else { "en" })
            .json(&serde_json::json!({
                "AppID":     self.config.app_id,
                "AppSecret": self.config.app_secret,
            }))
            .send()
            .await?
            .json::<WsEndpointResp>()
            .await?;

        if resp.code != 0 {
            anyhow::bail!(
                "Lark WS endpoint error: code={} msg={}",
                resp.code,
                resp.msg.as_deref().unwrap_or("(none)")
            );
        }

        let ep = resp
            .data
            .ok_or_else(|| anyhow::anyhow!("Lark WS endpoint: empty data field"))?;

        Ok((ep.url, ep.client_config.unwrap_or_default()))
    }

    /// Connect to the Lark WS endpoint and process events until the connection closes.
    ///
    /// Returns `Ok(())` when the connection drops (the caller reconnects).
    #[allow(clippy::too_many_lines)]
    async fn listen_ws_once(&self) -> anyhow::Result<()> {
        let (wss_url, client_config) = self.get_ws_endpoint().await?;

        // Extract service_id from the URL query string for use in pings
        let service_id: i32 = wss_url
            .split('?')
            .nth(1)
            .and_then(|qs| {
                qs.split('&')
                    .find(|kv| kv.starts_with("service_id="))
                    .and_then(|kv| kv.split('=').nth(1))
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0);

        info!("Lark: connecting to {wss_url}");
        let (ws_stream, _) = connect_async(&wss_url).await?;
        let (mut write, mut read) = ws_stream.split();
        info!("Lark: WS connected (service_id={service_id})");

        let mut ping_secs = client_config.ping_interval.unwrap_or(120).max(10);
        let mut hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
        let mut timeout_check = tokio::time::interval(Duration::from_secs(10));
        hb_interval.tick().await; // discard the immediate first tick

        let mut seq: u64 = 0;
        let mut last_recv = Instant::now();

        // Send an initial ping so the server starts sending pongs immediately
        seq = seq.wrapping_add(1);
        let initial_ping = PbFrame {
            seq_id: seq,
            log_id: 0,
            service: service_id,
            method: 0,
            headers: vec![PbHeader {
                key: "type".into(),
                value: "ping".into(),
            }],
            payload: None,
        };
        if write
            .send(WsMsg::Binary(initial_ping.encode_to_vec()))
            .await
            .is_err()
        {
            anyhow::bail!("Lark: initial ping failed");
        }

        // Fragment reassembly cache: message_id → (slots, created_at)
        type FragEntry = (Vec<Option<Vec<u8>>>, Instant);
        let mut frag_cache: HashMap<String, FragEntry> = HashMap::new();

        loop {
            tokio::select! {
                biased;

                _ = hb_interval.tick() => {
                    seq = seq.wrapping_add(1);
                    let ping = PbFrame {
                        seq_id: seq, log_id: 0, service: service_id, method: 0,
                        headers: vec![PbHeader { key: "type".into(), value: "ping".into() }],
                        payload: None,
                    };
                    if write.send(WsMsg::Binary(ping.encode_to_vec())).await.is_err() {
                        warn!("Lark: ping send failed — reconnecting");
                        break;
                    }
                    // GC stale fragments older than 5 minutes
                    let cutoff = Instant::now()
                        .checked_sub(Duration::from_secs(300))
                        .unwrap_or_else(Instant::now);
                    frag_cache.retain(|_, (_, ts)| *ts > cutoff);
                }

                _ = timeout_check.tick() => {
                    if last_recv.elapsed() > WS_HEARTBEAT_TIMEOUT {
                        warn!("Lark: heartbeat timeout — reconnecting");
                        break;
                    }
                }

                msg = read.next() => {
                    let raw = match msg {
                        Some(Ok(ws_msg)) => {
                            if is_live_ws_frame(&ws_msg) {
                                last_recv = Instant::now();
                            }
                            match ws_msg {
                                WsMsg::Binary(b) => b,
                                WsMsg::Ping(d) => {
                                    let _ = write.send(WsMsg::Pong(d)).await;
                                    continue;
                                }
                                WsMsg::Close(_) => {
                                    info!("Lark: server closed connection — reconnecting");
                                    break;
                                }
                                _ => continue,
                            }
                        }
                        None => { info!("Lark: WS stream ended — reconnecting"); break; }
                        Some(Err(e)) => { error!("Lark: WS read error: {e}"); break; }
                    };

                    // Decode the pbbp2 frame
                    let frame = match PbFrame::decode(&raw[..]) {
                        Ok(f) => f,
                        Err(e) => { error!("Lark: protobuf decode error: {e}"); continue; }
                    };

                    // ---- CONTROL frame (method=0) ----
                    if frame.method == 0 {
                        if frame.header_value("type") == "pong" {
                            if let Some(payload) = &frame.payload {
                                if let Ok(cfg) = serde_json::from_slice::<WsClientConfig>(payload) {
                                    if let Some(secs) = cfg.ping_interval {
                                        let secs = secs.max(10);
                                        if secs != ping_secs {
                                            ping_secs = secs;
                                            hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
                                            info!("Lark: ping_interval updated to {ping_secs}s");
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    // ---- DATA frame (method=1) ----
                    let msg_type = frame.header_value("type").to_string();
                    let msg_id   = frame.header_value("message_id").to_string();
                    let sum      = frame.header_value("sum").parse::<usize>().unwrap_or(1);
                    let seq_num  = frame.header_value("seq").parse::<usize>().unwrap_or(0);

                    // ACK immediately — Lark requires a reply within 3 s
                    {
                        let mut ack = frame.clone();
                        ack.payload = Some(br#"{"code":200,"headers":{},"data":[]}"#.to_vec());
                        ack.headers.push(PbHeader { key: "biz_rt".into(), value: "0".into() });
                        let _ = write.send(WsMsg::Binary(ack.encode_to_vec())).await;
                    }

                    // Reassemble multi-part frames
                    let sum = if sum == 0 { 1 } else { sum };
                    let payload: Vec<u8> = if sum == 1 || msg_id.is_empty() || seq_num >= sum {
                        frame.payload.clone().unwrap_or_default()
                    } else {
                        let entry = frag_cache
                            .entry(msg_id.clone())
                            .or_insert_with(|| (vec![None; sum], Instant::now()));
                        if entry.0.len() != sum {
                            *entry = (vec![None; sum], Instant::now());
                        }
                        entry.0[seq_num] = frame.payload.clone();
                        if entry.0.iter().all(|s| s.is_some()) {
                            let full: Vec<u8> = entry
                                .0
                                .iter()
                                .flat_map(|s| s.as_deref().unwrap_or(&[]))
                                .copied()
                                .collect();
                            frag_cache.remove(&msg_id);
                            full
                        } else {
                            continue;
                        }
                    };

                    if msg_type != "event" {
                        continue;
                    }

                    // Parse the event envelope
                    let event: serde_json::Value = match serde_json::from_slice(&payload) {
                        Ok(e) => e,
                        Err(e) => { error!("Lark: event JSON parse error: {e}"); continue; }
                    };

                    let event_type = event["header"]["event_type"].as_str().unwrap_or("");
                    if event_type != "im.message.receive_v1" {
                        continue;
                    }

                    // Extract sender info
                    let sender_type = event["event"]["sender"]["sender_type"]
                        .as_str()
                        .unwrap_or("");
                    if sender_type == "app" || sender_type == "bot" {
                        continue;
                    }

                    let sender_open_id = event["event"]["sender"]["sender_id"]["open_id"]
                        .as_str()
                        .unwrap_or("");

                    if !self.is_sender_allowed(sender_open_id) {
                        warn!("Lark WS: ignoring sender {sender_open_id} (not in allowed list)");
                        continue;
                    }

                    // Extract message fields
                    let message_id = event["event"]["message"]["message_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let chat_id = event["event"]["message"]["chat_id"]
                        .as_str()
                        .unwrap_or(sender_open_id)
                        .to_string();
                    let chat_type = event["event"]["message"]["chat_type"]
                        .as_str()
                        .unwrap_or("p2p");
                    let message_type = event["event"]["message"]["message_type"]
                        .as_str()
                        .unwrap_or("");
                    let content_str = event["event"]["message"]["content"]
                        .as_str()
                        .unwrap_or("");
                    let mentions: Vec<serde_json::Value> = event["event"]["message"]["mentions"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();

                    // Dedup
                    if !message_id.is_empty() {
                        let now = Instant::now();
                        let mut seen = self.ws_seen_ids.write().await;
                        seen.retain(|_, t| now.duration_since(*t) < Duration::from_secs(30 * 60));
                        if seen.contains_key(&message_id) {
                            debug!("Lark WS: duplicate message_id {message_id}, skipping");
                            continue;
                        }
                        seen.insert(message_id.clone(), now);
                    }

                    // Decode message text
                    let text = match message_type {
                        "text" => {
                            let v: serde_json::Value =
                                match serde_json::from_str(content_str) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };
                            match v
                                .get("text")
                                .and_then(|t| t.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                Some(t) => t.to_string(),
                                None => continue,
                            }
                        }
                        "post" => match parse_post_content(content_str) {
                            Some(t) => t,
                            None => continue,
                        },
                        _ => {
                            debug!(
                                "Lark WS: skipping unsupported message type '{message_type}'"
                            );
                            continue;
                        }
                    };

                    // Strip @_user_N placeholders, trim
                    let text = strip_at_placeholders(&text);
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        continue;
                    }

                    // Group chat: only reply when @-mentioned
                    if chat_type == "group" && !should_respond_in_group(&mentions) {
                        continue;
                    }

                    let inbound = InboundMessage::new(
                        self.base_config.name.as_str(),
                        sender_open_id,
                        &chat_id,
                        &text,
                    );

                    debug!("Lark WS: dispatching message from {sender_open_id} in {chat_id}");
                    if self.bus.publish_inbound(inbound).await.is_err() {
                        warn!("Lark: message bus closed — stopping WS loop");
                        break;
                    }

                    // Fire-and-forget emoji reaction ack (spec requirement)
                    if !message_id.is_empty() {
                        let api_base_str: &'static str = self.api_base();
                        let token_cache = Arc::clone(&self.tenant_token);
                        let app_id = self.config.app_id.clone();
                        let app_secret = self.config.app_secret.clone();
                        let msg_id = message_id.clone();
                        let _reaction = tokio::spawn(async move {
                            match fetch_tenant_token_cached(
                                api_base_str,
                                &app_id,
                                &app_secret,
                                &token_cache,
                            )
                            .await
                            {
                                Ok(token) => {
                                    let url = format!(
                                        "{}/im/v1/messages/{}/reactions",
                                        api_base_str, msg_id
                                    );
                                    match reqwest::Client::builder()
                                        .timeout(Duration::from_secs(30))
                                        .build()
                                        .unwrap_or_default()
                                        .post(&url)
                                        .bearer_auth(&token)
                                        .json(&serde_json::json!({
                                            "reaction_type": { "emoji_type": "OK" }
                                        }))
                                        .send()
                                        .await
                                    {
                                        Ok(resp) if resp.status().is_success() => {
                                            debug!("Lark: reaction ack sent for {msg_id}");
                                        }
                                        Ok(resp) => {
                                            let body = resp.text().await.unwrap_or_default();
                                            warn!("Failed to send Lark reaction: {body}");
                                        }
                                        Err(e) => {
                                            warn!("Failed to send Lark reaction: {e}");
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to send Lark reaction (token error): {e}");
                                }
                            }
                        });
                    }
                }
            }
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Public event parsing (used by tests and potential webhook mode)
    // ------------------------------------------------------------------

    /// Parse a Lark event v2 JSON payload and return `(sender_open_id, text)`.
    ///
    /// Returns `None` for non-message events, unrecognised message types,
    /// empty text, or senders blocked by the allowlist.
    pub fn parse_inbound_event(event_json: &serde_json::Value) -> Option<(String, String)> {
        let event_type = event_json["header"]["event_type"].as_str()?;
        if event_type != "im.message.receive_v1" {
            return None;
        }
        let sender_open_id = event_json["event"]["sender"]["sender_id"]["open_id"]
            .as_str()?
            .to_string();
        let content_str = event_json["event"]["message"]["content"].as_str()?;
        let content: serde_json::Value = serde_json::from_str(content_str).ok()?;
        let text = content["text"].as_str()?.trim().to_string();
        if text.is_empty() {
            return None;
        }
        Some((sender_open_id, text))
    }
}

// ---------------------------------------------------------------------------
// Channel trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Channel for LarkChannel {
    fn name(&self) -> &str {
        &self.base_config.name
    }

    async fn start(&mut self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        info!(
            "Lark channel starting (WS long-connection, feishu={})",
            self.config.feishu
        );

        let running = Arc::clone(&self.running);
        let tenant_token = Arc::clone(&self.tenant_token);
        let ws_seen_ids = Arc::clone(&self.ws_seen_ids);
        let config = self.config.clone();
        let bus = Arc::clone(&self.bus);

        // Spawn the reconnect loop as a background task
        let running_clone = Arc::clone(&running);
        tokio::spawn(async move {
            // Build a temporary channel clone for the async task
            let base_config = BaseChannelConfig {
                name: if config.feishu {
                    "feishu".to_string()
                } else {
                    "lark".to_string()
                },
                allowlist: config.allowed_senders.clone(),
                deny_by_default: config.deny_by_default,
            };
            let ch = LarkChannel {
                config,
                base_config,
                bus,
                running: Arc::clone(&running),
                tenant_token,
                ws_seen_ids,
            };

            let mut delay_secs = BASE_RECONNECT_DELAY_SECS;

            while running.load(Ordering::SeqCst) {
                match ch.listen_ws_once().await {
                    Ok(()) => {
                        // clean disconnect — reset backoff
                        delay_secs = BASE_RECONNECT_DELAY_SECS;
                    }
                    Err(e) => {
                        error!("Lark WS error: {e}");
                        warn!("Lark: reconnecting in {delay_secs}s");
                        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                        delay_secs = (delay_secs * 2).min(MAX_RECONNECT_DELAY_SECS);
                    }
                }
            }
            running_clone.store(false, Ordering::SeqCst);
            info!("Lark channel stopped");
        });

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Lark channel stopping");
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        let mut token = self
            .get_tenant_token()
            .await
            .map_err(|e| ZeptoError::Channel(format!("Lark: failed to get tenant token: {e}")))?;

        let url = format!("{}/im/v1/messages?receive_id_type=open_id", self.api_base());
        let content = serde_json::json!({ "text": msg.content }).to_string();
        let body = serde_json::json!({
            "receive_id": msg.chat_id,
            "msg_type":   "text",
            "content":    content,
        });

        let (status, response) = self
            .send_text_once(&url, &token, &body)
            .await
            .map_err(|e| ZeptoError::Channel(format!("Lark: send request failed: {e}")))?;

        if should_refresh_token(status, &response) {
            // Token expired — invalidate and retry once
            self.invalidate_token().await;
            token = self
                .get_tenant_token()
                .await
                .map_err(|e| ZeptoError::Channel(format!("Lark: token refresh failed: {e}")))?;
            let (retry_status, retry_body) = self
                .send_text_once(&url, &token, &body)
                .await
                .map_err(|e| ZeptoError::Channel(format!("Lark: retry send failed: {e}")))?;

            if !retry_status.is_success() {
                return Err(ZeptoError::Channel(format!(
                    "Lark send failed after token refresh: status={retry_status}, body={retry_body}"
                )));
            }
            let code = retry_body
                .get("code")
                .and_then(|c| c.as_i64())
                .unwrap_or(-1);
            if code != 0 {
                return Err(ZeptoError::Channel(format!(
                    "Lark send error after refresh: code={code}, body={retry_body}"
                )));
            }
            return Ok(());
        }

        if !status.is_success() {
            return Err(ZeptoError::Channel(format!(
                "Lark send failed: status={status}, body={response}"
            )));
        }
        let code = response.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(ZeptoError::Channel(format!(
                "Lark send error: code={code}, body={response}"
            )));
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.is_sender_allowed(user_id)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LarkConfig;

    fn lark_config() -> LarkConfig {
        LarkConfig {
            enabled: true,
            app_id: "app123".into(),
            app_secret: "secret456".into(),
            feishu: false,
            allowed_senders: vec![],
            bot_open_id: None,
            deny_by_default: false,
        }
    }

    fn make_channel() -> LarkChannel {
        let bus = Arc::new(MessageBus::new());
        LarkChannel::new(lark_config(), bus)
    }

    // ---- api_base / ws_base ----

    #[test]
    fn test_api_base_lark() {
        let ch = make_channel();
        assert_eq!(ch.api_base(), "https://open.larksuite.com/open-apis");
    }

    #[test]
    fn test_api_base_feishu() {
        let bus = Arc::new(MessageBus::new());
        let ch = LarkChannel::new(
            LarkConfig {
                feishu: true,
                ..lark_config()
            },
            bus,
        );
        assert_eq!(ch.api_base(), "https://open.feishu.cn/open-apis");
    }

    #[test]
    fn test_ws_base_lark() {
        let ch = make_channel();
        assert_eq!(ch.ws_base(), "https://open.larksuite.com");
    }

    #[test]
    fn test_ws_base_feishu() {
        let bus = Arc::new(MessageBus::new());
        let ch = LarkChannel::new(
            LarkConfig {
                feishu: true,
                ..lark_config()
            },
            bus,
        );
        assert_eq!(ch.ws_base(), "https://open.feishu.cn");
    }

    // ---- is_sender_allowed ----

    #[test]
    fn test_sender_allowed_empty_allowlist() {
        let ch = make_channel();
        // Empty allowlist + deny_by_default=false → allow all
        assert!(ch.is_sender_allowed("user_abc"));
    }

    #[test]
    fn test_sender_blocked_by_allowlist() {
        let bus = Arc::new(MessageBus::new());
        let ch = LarkChannel::new(
            LarkConfig {
                allowed_senders: vec!["user_allowed".into()],
                ..lark_config()
            },
            bus,
        );
        assert!(!ch.is_sender_allowed("user_blocked"));
        assert!(ch.is_sender_allowed("user_allowed"));
    }

    #[test]
    fn test_bot_self_filter() {
        let bus = Arc::new(MessageBus::new());
        let ch = LarkChannel::new(
            LarkConfig {
                bot_open_id: Some("bot_self_id".into()),
                ..lark_config()
            },
            bus,
        );
        assert!(!ch.is_sender_allowed("bot_self_id"));
        assert!(ch.is_sender_allowed("user_abc"));
    }

    #[test]
    fn test_deny_by_default_empty_allowlist() {
        let bus = Arc::new(MessageBus::new());
        let ch = LarkChannel::new(
            LarkConfig {
                deny_by_default: true,
                ..lark_config()
            },
            bus,
        );
        // Strict mode: empty allowlist rejects all
        assert!(!ch.is_sender_allowed("anyone"));
    }

    // ---- parse_inbound_event ----

    #[test]
    fn test_parse_inbound_event_valid() {
        let event = serde_json::json!({
            "schema": "2.0",
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_abc123" } },
                "message": {
                    "content": r#"{"text": "Hello from Lark"}"#
                }
            }
        });
        let result = LarkChannel::parse_inbound_event(&event);
        assert_eq!(result, Some(("ou_abc123".into(), "Hello from Lark".into())));
    }

    #[test]
    fn test_parse_inbound_event_non_message() {
        let event = serde_json::json!({
            "schema": "2.0",
            "header": { "event_type": "contact.user.created_v3" },
            "event": {}
        });
        assert!(LarkChannel::parse_inbound_event(&event).is_none());
    }

    #[test]
    fn test_parse_inbound_event_empty_text() {
        let event = serde_json::json!({
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_id": { "open_id": "ou_abc" } },
                "message": {
                    "content": r#"{"text": "   "}"#
                }
            }
        });
        // Whitespace-only text → None after trim
        assert!(LarkChannel::parse_inbound_event(&event).is_none());
    }

    // ---- channel name ----

    #[test]
    fn test_channel_name_lark() {
        let ch = make_channel();
        assert_eq!(ch.name(), "lark");
    }

    #[test]
    fn test_channel_name_feishu() {
        let bus = Arc::new(MessageBus::new());
        let ch = LarkChannel::new(
            LarkConfig {
                feishu: true,
                ..lark_config()
            },
            bus,
        );
        assert_eq!(ch.name(), "feishu");
    }

    // ---- helper functions ----

    #[test]
    fn test_extract_token_ttl_expire_field() {
        let body = serde_json::json!({ "expire": 7200 });
        assert_eq!(extract_token_ttl(&body), 7200);
    }

    #[test]
    fn test_extract_token_ttl_expires_in_field() {
        let body = serde_json::json!({ "expires_in": 3600 });
        assert_eq!(extract_token_ttl(&body), 3600);
    }

    #[test]
    fn test_extract_token_ttl_missing_uses_default() {
        let body = serde_json::json!({});
        assert_eq!(extract_token_ttl(&body), DEFAULT_TOKEN_TTL.as_secs());
    }

    #[test]
    fn test_should_refresh_token_on_401() {
        let body = serde_json::json!({ "code": 0 });
        assert!(should_refresh_token(
            reqwest::StatusCode::UNAUTHORIZED,
            &body
        ));
    }

    #[test]
    fn test_should_refresh_token_on_invalid_code() {
        let body = serde_json::json!({ "code": LARK_INVALID_ACCESS_TOKEN_CODE });
        assert!(should_refresh_token(reqwest::StatusCode::OK, &body));
    }

    #[test]
    fn test_should_not_refresh_token_on_success() {
        let body = serde_json::json!({ "code": 0 });
        assert!(!should_refresh_token(reqwest::StatusCode::OK, &body));
    }

    #[test]
    fn test_is_live_ws_frame_binary() {
        assert!(is_live_ws_frame(&WsMsg::Binary(vec![1, 2, 3].into())));
        assert!(is_live_ws_frame(&WsMsg::Ping(vec![].into())));
        assert!(is_live_ws_frame(&WsMsg::Pong(vec![].into())));
    }

    #[test]
    fn test_is_live_ws_frame_non_live() {
        assert!(!is_live_ws_frame(&WsMsg::Text("hello".into())));
        assert!(!is_live_ws_frame(&WsMsg::Close(None)));
    }

    #[test]
    fn test_strip_at_placeholders() {
        let input = "Hello @_user_1 how are you?";
        let result = strip_at_placeholders(input);
        assert_eq!(result, "Hello how are you?");
    }

    #[test]
    fn test_parse_post_content() {
        let content = serde_json::json!({
            "zh_cn": {
                "title": "Title",
                "content": [[{"tag": "text", "text": "Hello"}]]
            }
        });
        let result = parse_post_content(&content.to_string());
        assert!(result.is_some());
        assert!(result.unwrap().contains("Hello"));
    }

    #[test]
    fn test_lark_config_serde_roundtrip() {
        let cfg = lark_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: LarkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_id, "app123");
        assert_eq!(parsed.feishu, false);
        assert!(parsed.allowed_senders.is_empty());
    }
}
