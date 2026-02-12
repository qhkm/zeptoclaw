//! Cron scheduling tool.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::cron::{
    is_valid_cron_expr, parse_at_datetime_ms, CronPayload, CronSchedule, CronService,
};
use crate::error::{PicoError, Result};

use super::{Tool, ToolContext};

/// Tool for creating and managing scheduled jobs.
pub struct CronTool {
    cron: Arc<CronService>,
}

impl CronTool {
    /// Create a new cron tool.
    pub fn new(cron: Arc<CronService>) -> Self {
        Self { cron }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Schedule reminders and recurring tasks. Actions: add, list, remove."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove"],
                    "description": "Action to perform"
                },
                "message": {
                    "type": "string",
                    "description": "Message for add action"
                },
                "name": {
                    "type": "string",
                    "description": "Optional job name"
                },
                "every_seconds": {
                    "type": "integer",
                    "description": "Run interval in seconds"
                },
                "cron_expr": {
                    "type": "string",
                    "description": "Cron expression (UTC)"
                },
                "at": {
                    "type": "string",
                    "description": "One-shot ISO datetime"
                },
                "job_id": {
                    "type": "string",
                    "description": "Target job id for remove"
                },
                "channel": {
                    "type": "string",
                    "description": "Optional target channel (defaults to current)"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Optional target chat id (defaults to current)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PicoError::Tool("Missing 'action' argument".into()))?;

        match action {
            "add" => self.execute_add(args, ctx).await,
            "list" => self.execute_list(args).await,
            "remove" => self.execute_remove(args).await,
            other => Err(PicoError::Tool(format!("Unknown cron action '{}'", other))),
        }
    }
}

impl CronTool {
    async fn execute_add(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PicoError::Tool("Missing 'message' for cron add".into()))?;

        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if message.len() > 30 {
                    format!("{}...", &message[..30])
                } else {
                    message.to_string()
                }
            });

        let every_seconds = args.get("every_seconds").and_then(|v| v.as_i64());
        let cron_expr = args.get("cron_expr").and_then(|v| v.as_str());
        let at = args.get("at").and_then(|v| v.as_str());

        let mut schedule_count = 0;
        if every_seconds.is_some() {
            schedule_count += 1;
        }
        if cron_expr.is_some() {
            schedule_count += 1;
        }
        if at.is_some() {
            schedule_count += 1;
        }
        if schedule_count != 1 {
            return Err(PicoError::Tool(
                "Specify exactly one of: every_seconds, cron_expr, at".to_string(),
            ));
        }

        let (schedule, delete_after_run) = if let Some(seconds) = every_seconds {
            if seconds <= 0 {
                return Err(PicoError::Tool(
                    "'every_seconds' must be greater than zero".to_string(),
                ));
            }
            (
                CronSchedule::Every {
                    every_ms: seconds * 1_000,
                },
                false,
            )
        } else if let Some(expr) = cron_expr {
            let schedule = CronSchedule::Cron {
                expr: expr.to_string(),
            };
            if !is_valid_cron_expr(expr) {
                return Err(PicoError::Tool(format!(
                    "Invalid or non-runnable cron expression '{}'",
                    expr
                )));
            }
            (schedule, false)
        } else {
            let at_ms = parse_at_datetime_ms(at.unwrap())?;
            (CronSchedule::At { at_ms }, true)
        };

        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.channel.clone())
            .ok_or_else(|| PicoError::Tool("No channel available in tool context".into()))?;

        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| ctx.chat_id.clone())
            .ok_or_else(|| PicoError::Tool("No chat_id available in tool context".into()))?;

        let job = self
            .cron
            .add_job(
                name,
                schedule,
                CronPayload {
                    message: message.to_string(),
                    channel,
                    chat_id,
                },
                delete_after_run,
            )
            .await?;

        Ok(format!("Created cron job '{}' (id: {})", job.name, job.id))
    }

    async fn execute_list(&self, args: Value) -> Result<String> {
        let include_disabled = args
            .get("include_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let jobs = self.cron.list_jobs(include_disabled).await;
        if jobs.is_empty() {
            return Ok("No scheduled jobs".to_string());
        }

        let mut lines = Vec::new();
        for job in jobs {
            let schedule = match &job.schedule {
                CronSchedule::At { at_ms } => format!("at({})", at_ms),
                CronSchedule::Every { every_ms } => format!("every({}ms)", every_ms),
                CronSchedule::Cron { expr } => format!("cron({})", expr),
            };
            lines.push(format!(
                "- {} [{}] {} -> {}:{}",
                job.name, job.id, schedule, job.payload.channel, job.payload.chat_id
            ));
        }
        Ok(format!("Scheduled jobs:\n{}", lines.join("\n")))
    }

    async fn execute_remove(&self, args: Value) -> Result<String> {
        let job_id = args
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PicoError::Tool("Missing 'job_id' for cron remove".into()))?;

        if self.cron.remove_job(job_id).await? {
            Ok(format!("Removed cron job {}", job_id))
        } else {
            Ok(format!("Cron job {} not found", job_id))
        }
    }
}
