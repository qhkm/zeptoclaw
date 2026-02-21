//! Background sub-task tool.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task;

use crate::agent::AgentLoop;
use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

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

    fn compact_description(&self) -> &str {
        "Background task"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
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

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // Prevent recursive spawning (fork bomb protection)
        if ctx.channel.as_deref() == Some("subagent") {
            return Err(ZeptoError::Tool(
                "Cannot spawn from within a spawned task (recursion limit)".to_string(),
            ));
        }

        let task_text = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'task' argument".into()))?
            .to_string();

        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| crate::utils::string::preview(&task_text, 30));

        let channel = ctx
            .channel
            .clone()
            .ok_or_else(|| ZeptoError::Tool("No channel available in tool context".into()))?;
        let chat_id = ctx
            .chat_id
            .clone()
            .ok_or_else(|| ZeptoError::Tool("No chat_id available in tool context".into()))?;

        let task_id = uuid::Uuid::new_v4()
            .to_string()
            .chars()
            .take(8)
            .collect::<String>();
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

        Ok(ToolOutput::async_task(format!(
            "Spawned background task '{}' (id: {}). I will notify you when it completes.",
            label, task_id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;

    /// Helper: create a SpawnTool with a null agent (Weak::new()) and real MessageBus.
    fn make_spawn_tool() -> SpawnTool {
        let bus = Arc::new(MessageBus::new());
        SpawnTool::new(Weak::new(), bus)
    }

    /// Helper: ToolContext with channel + chat_id set.
    fn ctx_with_channel() -> ToolContext {
        ToolContext::new().with_channel("telegram", "chat_99")
    }

    // ---- metadata tests ----

    #[test]
    fn test_spawn_tool_name() {
        let tool = make_spawn_tool();
        assert_eq!(tool.name(), "spawn");
    }

    #[test]
    fn test_spawn_tool_description() {
        let tool = make_spawn_tool();
        let desc = tool.description();
        assert!(desc.contains("background"));
        assert!(desc.contains("task"));
    }

    #[test]
    fn test_spawn_tool_parameters_schema() {
        let tool = make_spawn_tool();
        let params = tool.parameters();

        assert_eq!(params["type"], "object");
        assert!(params["properties"]["task"].is_object());
        assert_eq!(params["properties"]["task"]["type"], "string");
        assert!(params["properties"]["label"].is_object());
        assert_eq!(params["required"], json!(["task"]));
    }

    // ---- execute tests ----

    #[tokio::test]
    async fn test_execute_missing_task() {
        let tool = make_spawn_tool();
        let ctx = ctx_with_channel();

        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Missing 'task'"));
    }

    #[tokio::test]
    async fn test_execute_rejects_recursive_spawn() {
        let tool = make_spawn_tool();
        // Simulate being called from within a spawned sub-agent.
        let ctx = ToolContext::new().with_channel("subagent", "sub_1");

        let result = tool.execute(json!({"task": "do something"}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Cannot spawn from within a spawned task"));
    }

    #[tokio::test]
    async fn test_execute_no_channel_in_context() {
        let tool = make_spawn_tool();
        let ctx = ToolContext::new(); // no channel/chat_id

        let result = tool
            .execute(json!({"task": "summarize report"}), &ctx)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No channel available"));
    }

    #[tokio::test]
    async fn test_execute_no_chat_id_in_context() {
        let tool = make_spawn_tool();
        // Set channel but not chat_id.
        let mut ctx = ToolContext::new();
        ctx.channel = Some("telegram".to_string());
        // chat_id stays None

        let result = tool.execute(json!({"task": "some work"}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No chat_id available"));
    }

    #[tokio::test]
    async fn test_execute_success_returns_task_id() {
        let tool = make_spawn_tool();
        let ctx = ctx_with_channel();

        let result = tool.execute(json!({"task": "analyze logs"}), &ctx).await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("Spawned background task"));
        assert!(output.contains("analyze logs"));
        assert!(output.contains("id:"));
    }

    #[tokio::test]
    async fn test_execute_custom_label() {
        let tool = make_spawn_tool();
        let ctx = ctx_with_channel();

        let result = tool
            .execute(
                json!({"task": "analyze logs for errors", "label": "log-check"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("log-check"));
    }

    #[tokio::test]
    async fn test_execute_long_task_auto_truncated_label() {
        let tool = make_spawn_tool();
        let ctx = ctx_with_channel();

        let long_task = "a]".repeat(40); // 80 chars, exceeds 30-char threshold
        let result = tool.execute(json!({"task": long_task}), &ctx).await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        // The auto-label should be truncated with "..."
        assert!(output.contains("..."));
    }
}
