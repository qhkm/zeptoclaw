//! Background sub-task tool.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task;

use crate::agent::AgentLoop;
use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::error::{PicoError, Result};

use super::{Tool, ToolContext};

/// Tool to spawn a background delegated task.
pub struct SpawnTool {
    agent: Weak<AgentLoop>,
    bus: Arc<MessageBus>,
}

impl SpawnTool {
    /// Create a new spawn tool.
    pub fn new(agent: Weak<AgentLoop>, bus: Arc<MessageBus>) -> Self {
        Self { agent, bus }
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a delegated background task and notify the user when it completes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description for delegated execution"
                },
                "label": {
                    "type": "string",
                    "description": "Optional short task label"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let task_text = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PicoError::Tool("Missing 'task' argument".into()))?
            .to_string();

        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if task_text.len() > 30 {
                    format!("{}...", &task_text[..30])
                } else {
                    task_text.clone()
                }
            });

        let channel = ctx
            .channel
            .clone()
            .ok_or_else(|| PicoError::Tool("No channel available in tool context".into()))?;
        let chat_id = ctx
            .chat_id
            .clone()
            .ok_or_else(|| PicoError::Tool("No chat_id available in tool context".into()))?;

        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let worker_task_id = task_id.clone();
        let agent = self.agent.clone();
        let bus = Arc::clone(&self.bus);
        let reply_channel = channel.clone();
        let reply_chat_id = chat_id.clone();
        let reply_label = label.clone();

        task::spawn(async move {
            let completion_text = if let Some(agent) = agent.upgrade() {
                let inbound =
                    InboundMessage::new("subagent", "subagent", &worker_task_id, &task_text);
                match agent.process_message(&inbound).await {
                    Ok(result) => format!(
                        "[Background task '{}' completed]\n\n{}",
                        reply_label, result
                    ),
                    Err(e) => format!("[Background task '{}' failed]\n\n{}", reply_label, e),
                }
            } else {
                format!(
                    "[Background task '{}' failed]\n\nAgent is no longer available",
                    reply_label
                )
            };

            let outbound = OutboundMessage::new(&reply_channel, &reply_chat_id, &completion_text);
            if let Err(e) = bus.publish_outbound(outbound).await {
                tracing::error!("Failed to publish spawn completion message: {}", e);
            }
        });

        Ok(format!(
            "Spawned background task '{}' (id: {}). I will notify you when it completes.",
            label, task_id
        ))
    }
}
