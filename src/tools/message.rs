//! Message tool for proactive outbound messages.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::bus::{MessageBus, OutboundMessage};
use crate::error::{Result, ZeptoError};

use super::{Tool, ToolContext};

/// Tool for sending outbound messages to channels.
pub struct MessageTool {
    bus: Arc<MessageBus>,
}

impl MessageTool {
    /// Create a new message tool.
    pub fn new(bus: Arc<MessageBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a proactive message to a chat."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Message text to send"
                },
                "channel": {
                    "type": "string",
                    "description": "Destination channel. Optional when context already has channel."
                },
                "chat_id": {
                    "type": "string",
                    "description": "Destination chat ID. Optional when context already has chat_id."
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'content' parameter".to_string()))?;

        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.channel.clone())
            .ok_or_else(|| ZeptoError::Tool("No target channel specified".to_string()))?;

        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.chat_id.clone())
            .ok_or_else(|| ZeptoError::Tool("No target chat_id specified".to_string()))?;

        // Validate channel name: only allow known channel types to prevent
        // the LLM from targeting arbitrary/unexpected channels.
        const ALLOWED_CHANNELS: &[&str] = &["telegram", "slack", "discord", "webhook"];
        if !ALLOWED_CHANNELS
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&channel))
        {
            return Err(ZeptoError::Tool(format!(
                "Unknown channel '{}'. Allowed: {}",
                channel,
                ALLOWED_CHANNELS.join(", ")
            )));
        }

        self.bus
            .publish_outbound(OutboundMessage::new(&channel, &chat_id, content))
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to publish message: {}", e)))?;

        Ok(format!("Message sent to {}:{}", channel, chat_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_tool_properties() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        assert_eq!(tool.name(), "message");
        assert!(tool.description().contains("proactive"));
    }

    #[tokio::test]
    async fn test_message_tool_with_context_target() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let ctx = ToolContext::new().with_channel("telegram", "12345");
        let result = tool.execute(json!({"content": "Hello"}), &ctx).await;

        assert!(result.is_ok());
        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.channel, "telegram");
        assert_eq!(outbound.chat_id, "12345");
        assert_eq!(outbound.content, "Hello");
    }

    #[tokio::test]
    async fn test_message_tool_missing_content() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({"channel": "telegram", "chat_id": "12345"}),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_message_tool_missing_target() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(json!({"content": "Hello"}), &ToolContext::new())
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_message_tool_rejects_unknown_channel() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({"content": "Hello", "channel": "evil-channel", "chat_id": "123"}),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown channel"));
    }

    #[tokio::test]
    async fn test_message_tool_allows_known_channels() {
        for channel in &["telegram", "slack", "discord", "webhook"] {
            let bus = Arc::new(MessageBus::new());
            let tool = MessageTool::new(bus.clone());

            let result = tool
                .execute(
                    json!({"content": "Hi", "channel": channel, "chat_id": "123"}),
                    &ToolContext::new(),
                )
                .await;

            assert!(result.is_ok(), "Channel '{}' should be allowed", channel);
        }
    }
}
