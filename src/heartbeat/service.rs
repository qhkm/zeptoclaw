//! Heartbeat service implementation.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::bus::{InboundMessage, MessageBus};
use crate::error::Result;

/// Prompt sent to the agent when heartbeat is triggered.
pub const HEARTBEAT_PROMPT: &str = r#"Read HEARTBEAT.md in your workspace (if it exists).
Follow any actionable items listed there.
If nothing needs attention, reply with: HEARTBEAT_OK"#;

/// Structured result from a heartbeat tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResult {
    /// Unix timestamp of the tick.
    pub timestamp: u64,
    /// Whether the heartbeat file was found.
    pub file_found: bool,
    /// Whether actionable content was present.
    pub actionable: bool,
    /// Whether the message was successfully published.
    pub delivered: bool,
    /// Error message if the tick failed.
    pub error: Option<String>,
}

impl HeartbeatResult {
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Construct a successful result.
    pub fn ok(file_found: bool, actionable: bool, delivered: bool) -> Self {
        Self {
            timestamp: Self::now(),
            file_found,
            actionable,
            delivered,
            error: None,
        }
    }

    /// Construct an error result.
    pub fn err(msg: &str) -> Self {
        Self {
            timestamp: Self::now(),
            file_found: false,
            actionable: false,
            delivered: false,
            error: Some(msg.to_string()),
        }
    }
}

/// Background service that periodically enqueues heartbeat prompts.
pub struct HeartbeatService {
    file_path: PathBuf,
    interval: Duration,
    bus: Arc<MessageBus>,
    running: Arc<RwLock<bool>>,
    chat_id: String,
    channel: String,
    /// Count of consecutive failed ticks.
    pub(crate) consecutive_failures: Arc<AtomicU32>,
    /// Threshold before warning about missed heartbeats.
    failure_alert_threshold: u32,
}

impl HeartbeatService {
    /// Create a new heartbeat service.
    pub fn new(
        file_path: PathBuf,
        interval_secs: u64,
        bus: Arc<MessageBus>,
        channel: &str,
        chat_id: &str,
    ) -> Self {
        Self {
            file_path,
            interval: Duration::from_secs(interval_secs.max(30)),
            bus,
            running: Arc::new(RwLock::new(false)),
            chat_id: chat_id.to_string(),
            channel: channel.to_string(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_alert_threshold: 3,
        }
    }

    /// Start heartbeat loop in the background.
    pub async fn start(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            if *running {
                warn!("Heartbeat service already running");
                return Ok(());
            }
            *running = true;
        }

        let file_path = self.file_path.clone();
        let interval_duration = self.interval;
        let bus = Arc::clone(&self.bus);
        let running = Arc::clone(&self.running);
        let chat_id = self.chat_id.clone();
        let channel = self.channel.clone();
        let consecutive_failures = Arc::clone(&self.consecutive_failures);
        let failure_threshold = self.failure_alert_threshold;

        info!(
            "Heartbeat service started (interval={}s, file={:?})",
            interval_duration.as_secs(),
            file_path
        );

        let running_clone = Arc::clone(&running);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval_duration);
            ticker.tick().await;

            loop {
                ticker.tick().await;

                if !*running.read().await {
                    info!("Heartbeat service stopped");
                    break;
                }

                let result = Self::tick(&file_path, &bus, &channel, &chat_id).await;

                if result.error.is_some() {
                    let count = consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
                    if count >= failure_threshold {
                        warn!(
                            consecutive_failures = count,
                            "Heartbeat: {} consecutive failures, service may be degraded", count
                        );
                    }
                } else {
                    consecutive_failures.store(0, Ordering::Relaxed);
                }
            }
            let mut r = running_clone.write().await;
            *r = false;
        });

        Ok(())
    }

    /// Stop heartbeat loop.
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    /// Trigger heartbeat immediately, returning a structured result.
    pub async fn trigger_now(&self) -> HeartbeatResult {
        Self::tick(&self.file_path, &self.bus, &self.channel, &self.chat_id).await
    }

    /// Returns whether service is running.
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }

    /// Returns the current count of consecutive failed ticks.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    /// Returns true if the service is healthy (fewer failures than the alert threshold).
    pub fn is_healthy(&self) -> bool {
        self.consecutive_failures() < self.failure_alert_threshold
    }

    /// Whether heartbeat content is actionable.
    pub fn is_empty(content: &str) -> bool {
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("<!--") {
                continue;
            }
            if line == "- [ ]" || line == "* [ ]" {
                continue;
            }
            return false;
        }
        true
    }

    async fn tick(
        file_path: &PathBuf,
        bus: &MessageBus,
        channel: &str,
        chat_id: &str,
    ) -> HeartbeatResult {
        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Heartbeat file missing at {:?}, skipping tick", file_path);
                return HeartbeatResult::ok(false, false, false);
            }
            Err(e) => {
                warn!("Failed to read heartbeat file {:?}: {}", file_path, e);
                return HeartbeatResult::err(&format!("Failed to read file: {e}"));
            }
        };

        if Self::is_empty(&content) {
            debug!("Heartbeat file has no actionable content");
            return HeartbeatResult::ok(true, false, false);
        }

        let message = InboundMessage::new(channel, "system", chat_id, HEARTBEAT_PROMPT);
        match bus.publish_inbound(message).await {
            Ok(_) => {
                info!("Heartbeat delivered to bus");
                HeartbeatResult::ok(true, true, true)
            }
            Err(e) => {
                error!("Failed to publish heartbeat: {}", e);
                HeartbeatResult::err(&format!("Delivery failed: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_empty_true() {
        assert!(HeartbeatService::is_empty(""));
        assert!(HeartbeatService::is_empty("# Header\n## Tasks"));
        assert!(HeartbeatService::is_empty("<!-- comment -->\n\n- [ ]"));
    }

    #[test]
    fn test_is_empty_false() {
        assert!(!HeartbeatService::is_empty("Check orders"));
        assert!(!HeartbeatService::is_empty("- [x] Done"));
        assert!(!HeartbeatService::is_empty("# Header\n- Send alert"));
    }

    #[test]
    fn test_heartbeat_result_ok() {
        let result = HeartbeatResult::ok(true, true, true);
        assert!(result.file_found);
        assert!(result.actionable);
        assert!(result.delivered);
        assert!(result.error.is_none());
        assert!(result.timestamp > 0);
    }

    #[test]
    fn test_heartbeat_result_err() {
        let result = HeartbeatResult::err("test error");
        assert!(!result.file_found);
        assert!(!result.delivered);
        assert_eq!(result.error, Some("test error".to_string()));
    }

    #[tokio::test]
    async fn test_heartbeat_tick_missing_file() {
        let bus = Arc::new(MessageBus::new());
        let result = HeartbeatService::tick(
            &PathBuf::from("/nonexistent/heartbeat.md"),
            &bus,
            "heartbeat",
            "test-chat",
        )
        .await;
        assert!(!result.file_found);
        assert!(!result.actionable);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_heartbeat_tick_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("HEARTBEAT.md");
        tokio::fs::write(&file, "# Tasks\n\n").await.unwrap();

        let bus = Arc::new(MessageBus::new());
        let result = HeartbeatService::tick(&file, &bus, "heartbeat", "test-chat").await;
        assert!(result.file_found);
        assert!(!result.actionable);
        assert!(!result.delivered);
    }

    #[tokio::test]
    async fn test_heartbeat_tick_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("HEARTBEAT.md");
        tokio::fs::write(&file, "# Tasks\n- Check orders\n")
            .await
            .unwrap();

        // MessageBus holds the inbound_rx internally, so publish_inbound succeeds
        // as long as the bus is alive (MPSC sender succeeds when receiver exists).
        let bus = Arc::new(MessageBus::new());
        let result = HeartbeatService::tick(&file, &bus, "heartbeat", "test-chat").await;
        assert!(result.file_found);
        assert!(result.actionable);
        assert!(result.delivered);
    }

    #[test]
    fn test_heartbeat_health_tracking() {
        let bus = Arc::new(MessageBus::new());
        let service =
            HeartbeatService::new(PathBuf::from("/tmp/hb.md"), 60, bus, "heartbeat", "test");
        assert_eq!(service.consecutive_failures(), 0);
        assert!(service.is_healthy());

        // Simulate accumulated failures (threshold is 3)
        service.consecutive_failures.store(3, Ordering::Relaxed);
        assert!(!service.is_healthy());
    }

    #[test]
    fn test_heartbeat_result_json_serialization() {
        let result = HeartbeatResult::ok(true, true, true);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"delivered\":true"));
        let parsed: HeartbeatResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.delivered);
    }

    #[test]
    fn test_heartbeat_service_stores_channel() {
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(crate::bus::MessageBus::new());
        let svc = HeartbeatService::new(dir.path().join("hb.md"), 60, bus, "telegram", "chat_99");
        assert_eq!(svc.channel, "telegram");
        assert_eq!(svc.chat_id, "chat_99");
    }
}
