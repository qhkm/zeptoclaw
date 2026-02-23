//! Message tool for proactive outbound messages.
//!
//! Supports multiple action types:
//! - `send` (default): Plain text message
//! - `react`: Add emoji reaction (Discord only)
//! - `rich_message`: Send Slack Block Kit message (Slack only)
//! - `inline_keyboard`: Send inline keyboard buttons (Telegram only)

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::bus::{MessageBus, OutboundMessage};
use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Channels that the message tool is allowed to target.
const ALLOWED_CHANNELS: &[&str] = &[
    "telegram",
    "slack",
    "discord",
    "webhook",
    "whatsapp",
    "whatsapp_cloud",
];

/// Tool for sending outbound messages to channels.
///
/// Supports plain text sends as well as channel-specific rich actions
/// like reactions (Discord), Block Kit messages (Slack), and inline
/// keyboards (Telegram).
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
        "Send a proactive message or perform a channel action. \
         Supports actions: 'send' (default, all channels), \
         'react' (Discord: add emoji reaction), \
         'rich_message' (Slack: Block Kit blocks), \
         'inline_keyboard' (Telegram: inline keyboard buttons)."
    }

    fn compact_description(&self) -> &str {
        "Send message or channel action"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Messaging
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
                    "description": "Destination channel name (telegram, discord, slack, whatsapp, webhook). Omit when replying — the originating channel is used automatically."
                },
                "chat_id": {
                    "type": "string",
                    "description": "Destination chat ID. Optional when context already has chat_id."
                },
                "reply_to": {
                    "type": "string",
                    "description": "Optional message ID to reply to (send action only)."
                },
                "discord_thread_name": {
                    "type": "string",
                    "description": "Discord only: create a thread in this channel with this name (send action only)."
                },
                "discord_thread_message_id": {
                    "type": "string",
                    "description": "Discord only: message ID to create a thread from (send action only)."
                },
                "discord_thread_auto_archive_minutes": {
                    "type": "integer",
                    "description": "Discord only: auto archive duration in minutes for new thread (send action only)."
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform. Default: 'send'. Options: 'send', 'react', 'rich_message', 'inline_keyboard'",
                    "default": "send"
                },
                "payload": {
                    "type": "object",
                    "description": "Action-specific payload (e.g., emoji for react, blocks for rich_message, buttons for inline_keyboard)"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
        let reply_to = args
            .get("reply_to")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let discord_thread_name = args
            .get("discord_thread_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let discord_thread_message_id = args
            .get("discord_thread_message_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let discord_thread_auto_archive_minutes = args
            .get("discord_thread_auto_archive_minutes")
            .and_then(|v| match v {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.parse::<u64>().ok(),
                _ => None,
            })
            .map(|n| n.to_string());

        // Validate channel name: only allow known channel types to prevent
        // the LLM from targeting arbitrary/unexpected channels.
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

        // Determine action — default to "send" when absent.
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("send");

        let payload = args.get("payload");
        let has_discord_thread_options = discord_thread_name.is_some()
            || discord_thread_message_id.is_some()
            || discord_thread_auto_archive_minutes.is_some();
        if has_discord_thread_options && !channel.eq_ignore_ascii_case("discord") {
            return Err(ZeptoError::Tool(
                "Discord thread options require channel='discord'".to_string(),
            ));
        }
        if has_discord_thread_options && discord_thread_name.is_none() {
            return Err(ZeptoError::Tool(
                "Missing 'discord_thread_name' for Discord thread creation".to_string(),
            ));
        }
        if (reply_to.is_some() || has_discord_thread_options) && action != "send" {
            return Err(ZeptoError::Tool(
                "reply_to and Discord thread options are only supported with action='send'"
                    .to_string(),
            ));
        }

        match action {
            "send" => {
                let mut outbound = OutboundMessage::new(&channel, &chat_id, content);
                if let Some(reply_id) = reply_to.as_deref() {
                    outbound = outbound.with_reply(reply_id);
                }
                if let Some(name) = discord_thread_name.as_deref() {
                    outbound = outbound.with_metadata("discord_thread_name", name);
                }
                if let Some(message_id) = discord_thread_message_id.as_deref() {
                    outbound = outbound.with_metadata("discord_thread_message_id", message_id);
                }
                if let Some(auto_archive_minutes) =
                    discord_thread_auto_archive_minutes.as_deref()
                {
                    outbound = outbound.with_metadata(
                        "discord_thread_auto_archive_minutes",
                        auto_archive_minutes,
                    );
                }

                self.bus
                    .publish_outbound(outbound)
                    .await
                    .map_err(|e| {
                        ZeptoError::Tool(format!("Failed to publish message: {}", e))
                    })?;
                Ok(ToolOutput::llm_only(format!("Message sent to {}:{}", channel, chat_id)))
            }

            "react" => {
                if !channel.eq_ignore_ascii_case("discord") {
                    return Err(ZeptoError::Tool(format!(
                        "Action 'react' is not supported on channel '{}'. Only supported on: discord",
                        channel
                    )));
                }
                let emoji = payload
                    .and_then(|p| p.get("emoji"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Action 'react' requires payload.emoji (string)".to_string(),
                        )
                    })?;
                let message_id = payload
                    .and_then(|p| p.get("message_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let rich_content = json!({
                    "action": "react",
                    "emoji": emoji,
                    "message_id": message_id,
                })
                .to_string();

                self.bus
                    .publish_outbound(OutboundMessage::new(&channel, &chat_id, &rich_content))
                    .await
                    .map_err(|e| {
                        ZeptoError::Tool(format!("Failed to publish react: {}", e))
                    })?;
                Ok(ToolOutput::llm_only(format!(
                    "Reaction '{}' sent to {}:{}",
                    emoji, channel, chat_id
                )))
            }

            "rich_message" => {
                if !channel.eq_ignore_ascii_case("slack") {
                    return Err(ZeptoError::Tool(format!(
                        "Action 'rich_message' is not supported on channel '{}'. Only supported on: slack",
                        channel
                    )));
                }
                let blocks = payload
                    .and_then(|p| p.get("blocks"))
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Action 'rich_message' requires payload.blocks (array)".to_string(),
                        )
                    })?;

                let rich_content = json!({
                    "action": "rich_message",
                    "blocks": blocks,
                })
                .to_string();

                self.bus
                    .publish_outbound(OutboundMessage::new(&channel, &chat_id, &rich_content))
                    .await
                    .map_err(|e| {
                        ZeptoError::Tool(format!("Failed to publish rich message: {}", e))
                    })?;
                Ok(ToolOutput::llm_only(format!("Rich message sent to {}:{}", channel, chat_id)))
            }

            "inline_keyboard" => {
                if !channel.eq_ignore_ascii_case("telegram") {
                    return Err(ZeptoError::Tool(format!(
                        "Action 'inline_keyboard' is not supported on channel '{}'. Only supported on: telegram",
                        channel
                    )));
                }
                let buttons = payload
                    .and_then(|p| p.get("buttons"))
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Action 'inline_keyboard' requires payload.buttons (array of arrays)"
                                .to_string(),
                        )
                    })?;

                let rich_content = json!({
                    "action": "inline_keyboard",
                    "text": content,
                    "buttons": buttons,
                })
                .to_string();

                self.bus
                    .publish_outbound(OutboundMessage::new(&channel, &chat_id, &rich_content))
                    .await
                    .map_err(|e| {
                        ZeptoError::Tool(format!("Failed to publish inline keyboard: {}", e))
                    })?;
                Ok(ToolOutput::llm_only(format!(
                    "Inline keyboard sent to {}:{}",
                    channel, chat_id
                )))
            }

            unknown => Err(ZeptoError::Tool(format!(
                "Unknown action '{}'. Supported actions: send, react, rich_message, inline_keyboard",
                unknown
            ))),
        }
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
        assert!(tool.description().contains("react"));
        assert!(tool.description().contains("rich_message"));
        assert!(tool.description().contains("inline_keyboard"));
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
        for channel in &[
            "telegram",
            "slack",
            "discord",
            "webhook",
            "whatsapp",
            "whatsapp_cloud",
        ] {
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

    // ====================================================================
    // WhatsApp channel tests
    // ====================================================================

    #[tokio::test]
    async fn test_message_tool_allows_whatsapp_channels() {
        for channel in &["whatsapp", "whatsapp_cloud"] {
            let bus = Arc::new(MessageBus::new());
            let tool = MessageTool::new(bus.clone());

            let result = tool
                .execute(
                    json!({"content": "Hi from WhatsApp", "channel": channel, "chat_id": "123"}),
                    &ToolContext::new(),
                )
                .await;

            assert!(result.is_ok(), "Channel '{}' should be allowed", channel);
            let outbound = bus.consume_outbound().await.expect("outbound message");
            assert_eq!(outbound.channel, *channel);
            assert_eq!(outbound.content, "Hi from WhatsApp");
        }
    }

    // ====================================================================
    // Default action tests
    // ====================================================================

    #[tokio::test]
    async fn test_message_tool_default_action_is_send() {
        // When no action field is provided, the tool should behave as a plain send.
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let result = tool
            .execute(
                json!({"content": "No action field", "channel": "telegram", "chat_id": "999"}),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let msg = result.unwrap().for_llm;
        assert!(msg.contains("Message sent"));
        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.content, "No action field");
    }

    #[tokio::test]
    async fn test_message_tool_explicit_send_action() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let result = tool
            .execute(
                json!({
                    "content": "Explicit send",
                    "channel": "slack",
                    "chat_id": "C01",
                    "action": "send"
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.content, "Explicit send");
    }

    #[tokio::test]
    async fn test_message_tool_with_reply_to() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let result = tool
            .execute(
                json!({"content": "Reply", "channel": "discord", "chat_id": "c1", "reply_to": "m123"}),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.reply_to.as_deref(), Some("m123"));
    }

    #[tokio::test]
    async fn test_message_tool_with_discord_thread_metadata() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let result = tool
            .execute(
                json!({
                    "content": "thread starter",
                    "channel": "discord",
                    "chat_id": "c1",
                    "discord_thread_name": "Daily Ops",
                    "discord_thread_auto_archive_minutes": 60
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(
            outbound
                .metadata
                .get("discord_thread_name")
                .map(String::as_str),
            Some("Daily Ops")
        );
        assert_eq!(
            outbound
                .metadata
                .get("discord_thread_auto_archive_minutes")
                .map(String::as_str),
            Some("60")
        );
    }

    #[tokio::test]
    async fn test_message_tool_discord_thread_rejects_non_discord_channel() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "bad",
                    "channel": "telegram",
                    "chat_id": "123",
                    "discord_thread_name": "Nope"
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Discord thread options require channel='discord'"));
    }

    #[tokio::test]
    async fn test_message_tool_discord_thread_requires_send_action() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "bad",
                    "channel": "discord",
                    "chat_id": "123",
                    "action": "react",
                    "discord_thread_name": "Nope",
                    "payload": {"emoji": "heart"}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("only supported with action='send'"));
    }

    // ====================================================================
    // React action tests
    // ====================================================================

    #[tokio::test]
    async fn test_message_tool_react_discord() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let result = tool
            .execute(
                json!({
                    "content": "reacting",
                    "channel": "discord",
                    "chat_id": "ch1",
                    "action": "react",
                    "payload": {
                        "emoji": "thumbsup",
                        "message_id": "msg_42"
                    }
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let msg = result.unwrap().for_llm;
        assert!(msg.contains("Reaction"));
        assert!(msg.contains("thumbsup"));

        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.channel, "discord");

        let parsed: Value = serde_json::from_str(&outbound.content).unwrap();
        assert_eq!(parsed["action"], "react");
        assert_eq!(parsed["emoji"], "thumbsup");
        assert_eq!(parsed["message_id"], "msg_42");
    }

    #[tokio::test]
    async fn test_message_tool_react_wrong_channel() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "react on telegram",
                    "channel": "telegram",
                    "chat_id": "123",
                    "action": "react",
                    "payload": {"emoji": "heart"}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("react"));
        assert!(err.contains("not supported"));
        assert!(err.contains("telegram"));
        assert!(err.contains("discord"));
    }

    #[tokio::test]
    async fn test_message_tool_react_missing_emoji() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "react without emoji",
                    "channel": "discord",
                    "chat_id": "ch1",
                    "action": "react",
                    "payload": {}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("emoji"));
    }

    // ====================================================================
    // Rich message action tests
    // ====================================================================

    #[tokio::test]
    async fn test_message_tool_rich_message_slack() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let blocks = json!([
            {
                "type": "section",
                "text": {"type": "mrkdwn", "text": "Hello *world*"}
            }
        ]);

        let result = tool
            .execute(
                json!({
                    "content": "rich msg",
                    "channel": "slack",
                    "chat_id": "C01",
                    "action": "rich_message",
                    "payload": {"blocks": blocks}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let msg = result.unwrap().for_llm;
        assert!(msg.contains("Rich message sent"));

        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.channel, "slack");

        let parsed: Value = serde_json::from_str(&outbound.content).unwrap();
        assert_eq!(parsed["action"], "rich_message");
        assert!(parsed["blocks"].is_array());
        assert_eq!(parsed["blocks"][0]["type"], "section");
    }

    #[tokio::test]
    async fn test_message_tool_rich_message_wrong_channel() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "rich on discord",
                    "channel": "discord",
                    "chat_id": "ch1",
                    "action": "rich_message",
                    "payload": {"blocks": []}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("rich_message"));
        assert!(err.contains("not supported"));
        assert!(err.contains("discord"));
        assert!(err.contains("slack"));
    }

    #[tokio::test]
    async fn test_message_tool_rich_message_missing_blocks() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "no blocks",
                    "channel": "slack",
                    "chat_id": "C01",
                    "action": "rich_message",
                    "payload": {}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("blocks"));
    }

    // ====================================================================
    // Inline keyboard action tests
    // ====================================================================

    #[tokio::test]
    async fn test_message_tool_inline_keyboard_telegram() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus.clone());

        let buttons = json!([
            [
                {"text": "Yes", "callback_data": "yes"},
                {"text": "No", "callback_data": "no"}
            ]
        ]);

        let result = tool
            .execute(
                json!({
                    "content": "Do you agree?",
                    "channel": "telegram",
                    "chat_id": "12345",
                    "action": "inline_keyboard",
                    "payload": {"buttons": buttons}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_ok());
        let msg = result.unwrap().for_llm;
        assert!(msg.contains("Inline keyboard sent"));

        let outbound = bus.consume_outbound().await.expect("outbound message");
        assert_eq!(outbound.channel, "telegram");

        let parsed: Value = serde_json::from_str(&outbound.content).unwrap();
        assert_eq!(parsed["action"], "inline_keyboard");
        assert_eq!(parsed["text"], "Do you agree?");
        assert!(parsed["buttons"].is_array());
        assert_eq!(parsed["buttons"][0][0]["text"], "Yes");
        assert_eq!(parsed["buttons"][0][1]["callback_data"], "no");
    }

    #[tokio::test]
    async fn test_message_tool_inline_keyboard_wrong_channel() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "keyboard on slack",
                    "channel": "slack",
                    "chat_id": "C01",
                    "action": "inline_keyboard",
                    "payload": {"buttons": []}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("inline_keyboard"));
        assert!(err.contains("not supported"));
        assert!(err.contains("slack"));
        assert!(err.contains("telegram"));
    }

    #[tokio::test]
    async fn test_message_tool_inline_keyboard_missing_buttons() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "no buttons",
                    "channel": "telegram",
                    "chat_id": "12345",
                    "action": "inline_keyboard",
                    "payload": {}
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("buttons"));
    }

    // ====================================================================
    // Unknown action test
    // ====================================================================

    #[tokio::test]
    async fn test_message_tool_unknown_action() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(bus);

        let result = tool
            .execute(
                json!({
                    "content": "test",
                    "channel": "telegram",
                    "chat_id": "123",
                    "action": "foobar"
                }),
                &ToolContext::new(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown action"));
        assert!(err.contains("foobar"));
        assert!(err.contains("send"));
        assert!(err.contains("react"));
        assert!(err.contains("rich_message"));
        assert!(err.contains("inline_keyboard"));
    }
}
