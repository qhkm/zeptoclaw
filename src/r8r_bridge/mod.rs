//! r8r bridge integration for ZeptoClaw.
//!
//! This module handles the bidirectional event stream between ZeptoClaw and
//! [r8r](https://github.com/qhkm/r8r), the workflow-automation engine.
//!
//! # Submodules
//!
//! * [`events`] — Mirrored event types matching r8r's wire format.
//! * [`dedup`]  — Deduplicator for at-least-once delivery.
//! * [`approval`] — Approval routing and response parsing (Task 8).
//! * [`health`] — Health ping loop and CLI status (Task 9).

pub mod approval;
pub mod dedup;
pub mod events;
pub mod health;

pub use dedup::Deduplicator;
pub use events::{Ack, BridgeEvent, BridgeEventEnvelope};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{info, warn};
use url::Url;

/// Maximum length for event IDs to prevent memory abuse.
const MAX_EVENT_ID_LEN: usize = 256;

/// Maximum WebSocket message size (1 MiB).
const MAX_WS_MESSAGE_SIZE: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// R8rBridge — WebSocket client for r8r event stream
// ---------------------------------------------------------------------------

/// WebSocket client that connects to r8r's `/api/ws/events` endpoint.
///
/// Provides:
/// - Automatic reconnection with exponential backoff.
/// - At-least-once deduplication via [`Deduplicator`].
/// - Event dispatch by type (health, approval, execution).
/// - Outbound message sending via an internal mpsc channel.
pub struct R8rBridge {
    /// WebSocket endpoint URL (e.g. `ws://localhost:8080/api/ws/events`).
    endpoint: String,
    /// Optional bearer token for authentication.
    token: Option<String>,
    /// Sender half of the outbound message channel (feeds the WS writer task).
    sender: Arc<Mutex<Option<mpsc::Sender<String>>>>,
    /// Deduplicator for at-least-once delivery.
    dedup: Arc<Mutex<Deduplicator>>,
    /// Last received health status event.
    health_status: Arc<Mutex<Option<BridgeEvent>>>,
    /// Whether the bridge is currently connected.
    connected: Arc<AtomicBool>,
}

fn build_ws_request(endpoint: &str, token: Option<&str>) -> Result<Request<()>, String> {
    let parsed = Url::parse(endpoint).map_err(|e| format!("Invalid endpoint URL: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "Endpoint URL has no host".to_string())?;
    let host_with_port = if let Some(port) = parsed.port() {
        format!("{host}:{port}")
    } else {
        host.to_string()
    };

    let mut request = endpoint
        .into_client_request()
        .map_err(|e| format!("Failed to build WS request: {e}"))?;
    let host_header =
        HeaderValue::from_str(&host_with_port).map_err(|e| format!("Invalid Host header: {e}"))?;
    request.headers_mut().insert("Host", host_header);

    if let Some(token) = token {
        let auth_header = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| format!("Invalid Authorization header: {e}"))?;
        request.headers_mut().insert("Authorization", auth_header);
    }

    Ok(request)
}

/// Sanitize an endpoint URL for logging by stripping any embedded credentials.
fn sanitize_endpoint(endpoint: &str) -> String {
    Url::parse(endpoint)
        .ok()
        .map(|mut u| {
            let _ = u.set_password(None);
            let _ = u.set_username("");
            u.to_string()
        })
        .unwrap_or_else(|| "[invalid url]".to_string())
}

impl R8rBridge {
    /// Create a new bridge client.
    ///
    /// * `endpoint` — WebSocket URL (e.g. `ws://localhost:8080/api/ws/events`).
    /// * `token` — Optional bearer token for the `Authorization` header.
    pub fn new(endpoint: String, token: Option<String>) -> Self {
        Self {
            endpoint,
            token,
            sender: Arc::new(Mutex::new(None)),
            dedup: Arc::new(Mutex::new(Deduplicator::default())),
            health_status: Arc::new(Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns `true` if the bridge is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Establish a one-shot WebSocket connection.
    ///
    /// 1. Builds an HTTP upgrade request with `Authorization` and `Host` headers.
    /// 2. Connects via `tokio-tungstenite`.
    /// 3. Splits the stream into sender/receiver halves.
    /// 4. Spawns a writer task that reads from an internal mpsc channel.
    /// 5. Spawns a reader task that:
    ///    - Parses incoming text messages as [`BridgeEventEnvelope`].
    ///    - Deduplicates events via [`Deduplicator`].
    ///    - Sends acknowledgments back.
    ///    - Dispatches events by type.
    pub async fn connect(&self) -> Result<(), String> {
        let request = build_ws_request(&self.endpoint, self.token.as_deref())?;

        // Connect with message size limits to prevent memory exhaustion.
        let mut ws_config = WebSocketConfig::default();
        ws_config.max_message_size = Some(MAX_WS_MESSAGE_SIZE);
        ws_config.max_frame_size = Some(MAX_WS_MESSAGE_SIZE);
        let (ws_stream, _response) =
            tokio_tungstenite::connect_async_with_config(request, Some(ws_config), false)
                .await
                .map_err(|e| format!("WebSocket connection failed: {e}"))?;

        let (ws_write, ws_read) = ws_stream.split();

        // Create internal mpsc channel for outbound messages.
        let (tx, mut rx) = mpsc::channel::<String>(256);
        {
            let mut sender_guard = self.sender.lock().await;
            *sender_guard = Some(tx);
        }

        self.connected.store(true, Ordering::Relaxed);
        info!(
            "r8r bridge connected to {}",
            sanitize_endpoint(&self.endpoint)
        );

        // Spawn writer task: reads from mpsc, writes to WS.
        let ws_write = Arc::new(Mutex::new(ws_write));
        let ws_write_clone = Arc::clone(&ws_write);
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let mut writer = ws_write_clone.lock().await;
                if let Err(e) = writer.send(WsMessage::Text(msg.into())).await {
                    warn!("r8r bridge send error: {e}");
                    break;
                }
            }
        });

        // Spawn reader task: reads from WS, dispatches events.
        let dedup = Arc::clone(&self.dedup);
        let health_status = Arc::clone(&self.health_status);
        let connected = Arc::clone(&self.connected);
        let sender = Arc::clone(&self.sender);

        tokio::spawn(async move {
            let mut ws_read = ws_read;

            while let Some(msg_result) = ws_read.next().await {
                let msg = match msg_result {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("r8r bridge receive error: {e}");
                        break;
                    }
                };

                let text = match msg {
                    WsMessage::Text(t) => t.to_string(),
                    WsMessage::Close(_) => {
                        info!("r8r bridge received close frame");
                        break;
                    }
                    _ => continue,
                };

                // Parse envelope.
                let envelope: BridgeEventEnvelope = match serde_json::from_str(&text) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("r8r bridge: failed to parse envelope: {e}");
                        continue;
                    }
                };

                // Reject oversized event IDs to prevent memory abuse.
                if envelope.id.len() > MAX_EVENT_ID_LEN {
                    warn!(
                        "r8r bridge: event ID exceeds {} chars, dropping",
                        MAX_EVENT_ID_LEN
                    );
                    continue;
                }

                // Ack every well-formed envelope, including duplicates, so replayed
                // deliveries are drained cleanly on reconnect.
                let ack = Ack {
                    event_id: envelope.id.clone(),
                };
                if let Ok(ack_json) = serde_json::to_string(&ack) {
                    let mut writer = ws_write.lock().await;
                    if let Err(e) = writer.send(WsMessage::Text(ack_json.into())).await {
                        warn!("r8r bridge: failed to send ack: {e}");
                    }
                }

                // Dedup check.
                {
                    let mut dd = dedup.lock().await;
                    if !dd.is_new(&envelope.id) {
                        info!("r8r bridge: acknowledged duplicate event {}", envelope.id);
                        continue;
                    }
                }

                // Dispatch by event type.
                match BridgeEvent::from_type_and_data(&envelope.event_type, &envelope.data) {
                    Ok(event) => match &event {
                        BridgeEvent::HealthStatus { .. } => {
                            let mut hs = health_status.lock().await;
                            *hs = Some(event);
                        }
                        BridgeEvent::ApprovalRequested {
                            workflow,
                            execution_id,
                            ..
                        } => {
                            info!(
                                "r8r bridge: approval requested for {} ({})",
                                workflow, execution_id
                            );
                        }
                        BridgeEvent::ApprovalTimeout {
                            workflow,
                            execution_id,
                            ..
                        } => {
                            info!(
                                "r8r bridge: approval timeout for {} ({})",
                                workflow, execution_id
                            );
                        }
                        BridgeEvent::ExecutionCompleted {
                            workflow,
                            execution_id,
                            ..
                        } => {
                            info!(
                                "r8r bridge: execution completed for {} ({})",
                                workflow, execution_id
                            );
                        }
                        BridgeEvent::ExecutionFailed {
                            workflow,
                            execution_id,
                            ..
                        } => {
                            info!(
                                "r8r bridge: execution failed for {} ({})",
                                workflow, execution_id
                            );
                        }
                        _ => {
                            info!("r8r bridge: received event type {}", envelope.event_type);
                        }
                    },
                    Err(e) => {
                        warn!("r8r bridge: failed to parse event: {e}");
                    }
                }
            }

            // Disconnected — clean up.
            connected.store(false, Ordering::Relaxed);
            let mut sender_guard = sender.lock().await;
            *sender_guard = None;
            info!("r8r bridge disconnected");
        });

        Ok(())
    }

    /// Disconnect from the bridge, clearing the sender and setting connected to false.
    pub async fn disconnect(&self) {
        let mut sender_guard = self.sender.lock().await;
        *sender_guard = None;
        self.connected.store(false, Ordering::Relaxed);
        info!("r8r bridge disconnected (manual)");
    }

    /// Send a serialized envelope through the WebSocket.
    ///
    /// Returns an error if not connected or the channel is full.
    pub async fn send(&self, envelope: BridgeEventEnvelope) -> Result<(), String> {
        let json = serde_json::to_string(&envelope)
            .map_err(|e| format!("Failed to serialize envelope: {e}"))?;

        let sender_guard = self.sender.lock().await;
        match sender_guard.as_ref() {
            Some(tx) => tx
                .send(json)
                .await
                .map_err(|e| format!("Failed to send message: {e}")),
            None => Err("Not connected".to_string()),
        }
    }

    /// Send a health ping to the r8r server.
    pub async fn send_health_ping(&self) -> Result<(), String> {
        let envelope = BridgeEventEnvelope::new(BridgeEvent::HealthPing, None);
        self.send(envelope).await
    }

    /// Return the last received health status event, if any.
    pub async fn last_health_status(&self) -> Option<BridgeEvent> {
        let hs = self.health_status.lock().await;
        hs.clone()
    }

    /// Run a reconnection loop with exponential backoff.
    ///
    /// * `max_interval_secs` — Maximum backoff interval in seconds.
    ///
    /// This method runs forever, reconnecting on disconnection.
    pub async fn run(&self, max_interval_secs: u64) {
        let mut backoff_secs: u64 = 1;

        loop {
            match self.connect().await {
                Ok(()) => {
                    backoff_secs = 1;
                    // Wait until disconnected.
                    while self.is_connected() {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
                Err(e) => {
                    warn!("r8r bridge connection failed: {e}");
                }
            }

            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(max_interval_secs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ws_request_sets_host_and_authorization_headers() {
        let request =
            build_ws_request("ws://localhost:8080/api/ws/events", Some("secret-token")).unwrap();

        assert_eq!(request.headers()["Host"], "localhost:8080");
        assert_eq!(request.headers()["Authorization"], "Bearer secret-token");
    }

    #[test]
    fn test_build_ws_request_rejects_invalid_authorization_header() {
        let err = build_ws_request("ws://localhost:8080/api/ws/events", Some("bad\nvalue"))
            .expect_err("invalid header should return an error");
        assert!(err.contains("Authorization"));
    }

    #[test]
    fn test_sanitize_endpoint_strips_credentials() {
        let sanitized = sanitize_endpoint("ws://user:secret@host:8080/path");
        assert!(!sanitized.contains("secret"), "password should be stripped");
        assert!(!sanitized.contains("user"), "username should be stripped");
        assert!(sanitized.contains("host:8080/path"));
    }

    #[test]
    fn test_sanitize_endpoint_passes_clean_url() {
        let sanitized = sanitize_endpoint("ws://localhost:8080/api/ws/events");
        assert_eq!(sanitized, "ws://localhost:8080/api/ws/events");
    }

    #[test]
    fn test_sanitize_endpoint_handles_invalid_url() {
        assert_eq!(sanitize_endpoint("not a url"), "[invalid url]");
    }

    #[test]
    fn test_max_event_id_len_constant() {
        const { assert!(MAX_EVENT_ID_LEN >= 64, "must allow standard evt_<uuid> IDs") };
        const { assert!(MAX_EVENT_ID_LEN <= 1024, "must not allow absurdly long IDs") };
    }
}
