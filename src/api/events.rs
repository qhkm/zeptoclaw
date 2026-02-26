//! Panel event bus — bridges agent loop events to WebSocket clients.

use serde::Serialize;
use tokio::sync::broadcast;

/// Events emitted by the agent loop and consumed by WebSocket clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelEvent {
    /// A tool execution has started.
    ToolStarted { tool: String },
    /// A tool execution completed successfully.
    ToolDone { tool: String, duration_ms: u64 },
    /// A tool execution failed.
    ToolFailed { tool: String, error: String },
    /// An inbound message was received on a channel.
    MessageReceived { channel: String, chat_id: String },
    /// An agent run has started for a session.
    AgentStarted { session_key: String },
    /// An agent run has completed.
    AgentDone { session_key: String, tokens: u64 },
    /// Context compaction occurred.
    Compaction { from_tokens: u64, to_tokens: u64 },
    /// A channel's status changed.
    ChannelStatus { channel: String, status: String },
    /// A cron job fired.
    CronFired { job_id: String, status: String },
}

/// Broadcast-based event bus for panel real-time events.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<PanelEvent>,
}

impl EventBus {
    /// Create a new event bus with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Send an event to all subscribers. Silently drops if no subscribers.
    pub fn send(&self, event: PanelEvent) {
        let _ = self.tx.send(event);
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<PanelEvent> {
        self.tx.subscribe()
    }

    /// Get the current number of active subscribers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_bus_send_receive() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.send(PanelEvent::ChannelStatus {
            channel: "telegram".into(),
            status: "up".into(),
        });
        let event = rx.recv().await.unwrap();
        match event {
            PanelEvent::ChannelStatus { channel, status } => {
                assert_eq!(channel, "telegram");
                assert_eq!(status, "up");
            }
            _ => panic!("unexpected event type"),
        }
    }

    #[tokio::test]
    async fn test_event_bus_multiple_subscribers() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.send(PanelEvent::AgentStarted {
            session_key: "test:123".into(),
        });
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn test_event_bus_no_subscribers_no_panic() {
        let bus = EventBus::new(16);
        // Should not panic even with no subscribers
        bus.send(PanelEvent::ToolStarted {
            tool: "echo".into(),
        });
    }

    #[tokio::test]
    async fn test_event_bus_receiver_count() {
        let bus = EventBus::new(16);
        assert_eq!(bus.receiver_count(), 0);
        let _rx1 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);
        let _rx2 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 2);
    }

    #[test]
    fn test_panel_event_serialization() {
        let event = PanelEvent::ToolDone {
            tool: "web_search".into(),
            duration_ms: 230,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_done""#));
        assert!(json.contains(r#""tool":"web_search""#));
        assert!(json.contains(r#""duration_ms":230"#));
    }
}
