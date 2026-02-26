//! Kanban task management tool — allows the agent to manage board tasks.
//!
//! Wraps [`TaskStore`] and exposes five actions (`list`, `create`, `update`,
//! `move`, `delete`) that the LLM can call via the tool-calling interface.
//!
//! # Example
//!
//! ```rust
//! # tokio_test::block_on(async {
//! use std::sync::Arc;
//! use zeptoclaw::api::tasks::TaskStore;
//! use zeptoclaw::tools::{Tool, ToolContext};
//! use zeptoclaw::tools::task::TaskTool;
//! use serde_json::json;
//!
//! let store = Arc::new(TaskStore::new_in_memory());
//! let tool = TaskTool::new(store);
//! let ctx = ToolContext::new();
//!
//! let out = tool.execute(json!({"action": "create", "title": "Hello", "column": "backlog"}), &ctx).await.unwrap();
//! assert!(out.for_llm.contains("Created task"));
//! # });
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::api::tasks::TaskStore;
use crate::error::Result;
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Agent tool for managing kanban board tasks.
///
/// Thin wrapper around [`TaskStore`]; translates JSON tool-call arguments
/// into store operations and formats results for the LLM.
pub struct TaskTool {
    store: Arc<TaskStore>,
}

impl TaskTool {
    /// Create a new `TaskTool` backed by the given store.
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Manage kanban board tasks. Use 'list' to see tasks, 'create' to add one, \
         'update' to edit fields, 'move' to change column, and 'delete' to remove a task."
    }

    fn compact_description(&self) -> &str {
        "Manage kanban board tasks"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "create", "update", "move", "delete"],
                    "description": "The operation to perform"
                },
                "id": {
                    "type": "string",
                    "description": "Task ID — required for update, move, delete"
                },
                "title": {
                    "type": "string",
                    "description": "Task title — required for create"
                },
                "column": {
                    "type": "string",
                    "enum": ["backlog", "in_progress", "review", "done"],
                    "description": "Kanban column — required for create and move; optional filter for list"
                },
                "assignee": {
                    "type": "string",
                    "description": "Assignee name — optional for create, settable via update"
                },
                "description": {
                    "type": "string",
                    "description": "Longer task description — settable via update"
                },
                "priority": {
                    "type": "string",
                    "description": "Priority level (e.g. low, medium, high, urgent) — settable via update"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let output = match action {
            "list" => {
                let col = args.get("column").and_then(|v| v.as_str());
                let tasks = self.store.list(col).await;
                if tasks.is_empty() {
                    match col {
                        Some(c) => format!("No tasks in column '{c}'"),
                        None => "No tasks found".to_string(),
                    }
                } else {
                    serde_json::to_string_pretty(&tasks).unwrap_or_default()
                }
            }

            "create" => {
                let title = args
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled");
                let column = args
                    .get("column")
                    .and_then(|v| v.as_str())
                    .unwrap_or("backlog");
                let assignee = args
                    .get("assignee")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                match self.store.create(title, column, assignee).await {
                    Ok(id) => format!("Created task {id}"),
                    Err(e) => format!("Error: {e}"),
                }
            }

            "update" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    "Error: 'id' is required for update".to_string()
                } else {
                    match self.store.update(id, args.clone()).await {
                        Ok(()) => format!("Updated task {id}"),
                        Err(e) => format!("Error: {e}"),
                    }
                }
            }

            "move" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let column = args.get("column").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    "Error: 'id' is required for move".to_string()
                } else if column.is_empty() {
                    "Error: 'column' is required for move".to_string()
                } else {
                    match self.store.move_task(id, column).await {
                        Ok(()) => format!("Moved task {id} to '{column}'"),
                        Err(e) => format!("Error: {e}"),
                    }
                }
            }

            "delete" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    "Error: 'id' is required for delete".to_string()
                } else {
                    match self.store.delete(id).await {
                        Ok(()) => format!("Deleted task {id}"),
                        Err(e) => format!("Error: {e}"),
                    }
                }
            }

            other => format!(
                "Unknown action '{other}'. Valid actions: list, create, update, move, delete"
            ),
        };

        Ok(ToolOutput::llm_only(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool() -> TaskTool {
        TaskTool::new(Arc::new(TaskStore::new_in_memory()))
    }

    fn ctx() -> ToolContext {
        ToolContext::new()
    }

    // ── metadata ─────────────────────────────────────────────────────────────

    #[test]
    fn test_tool_name() {
        assert_eq!(make_tool().name(), "task");
    }

    #[test]
    fn test_tool_description_non_empty() {
        let t = make_tool();
        assert!(!t.description().is_empty());
        assert!(!t.compact_description().is_empty());
    }

    #[test]
    fn test_tool_category_is_memory() {
        assert_eq!(make_tool().category(), ToolCategory::Memory);
    }

    #[test]
    fn test_parameters_schema() {
        let params = make_tool().parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert!(params["properties"]["id"].is_object());
        assert!(params["properties"]["title"].is_object());
        assert!(params["properties"]["column"].is_object());
        assert_eq!(params["required"], json!(["action"]));
    }

    // ── list ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_empty() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "list"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("No tasks"), "got: {out}");
    }

    #[tokio::test]
    async fn test_list_returns_json() {
        let t = make_tool();
        t.execute(
            json!({"action": "create", "title": "Alpha", "column": "backlog"}),
            &ctx(),
        )
        .await
        .unwrap();

        let out = t
            .execute(json!({"action": "list"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("Alpha"), "got: {out}");
        // Should be valid JSON
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[tokio::test]
    async fn test_list_with_column_filter() {
        let t = make_tool();
        t.execute(
            json!({"action": "create", "title": "A", "column": "backlog"}),
            &ctx(),
        )
        .await
        .unwrap();
        t.execute(
            json!({"action": "create", "title": "B", "column": "done"}),
            &ctx(),
        )
        .await
        .unwrap();

        let out = t
            .execute(json!({"action": "list", "column": "backlog"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("A"), "got: {out}");
        assert!(!out.contains("\"B\""), "got: {out}");
    }

    #[tokio::test]
    async fn test_list_empty_column_message() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "list", "column": "review"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("review"), "got: {out}");
    }

    // ── create ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_returns_id() {
        let t = make_tool();
        let out = t
            .execute(
                json!({"action": "create", "title": "My task", "column": "backlog"}),
                &ctx(),
            )
            .await
            .unwrap()
            .for_llm;
        assert!(out.starts_with("Created task "), "got: {out}");
        // The ID should be a UUID (36 chars)
        let id = out.trim_start_matches("Created task ");
        assert_eq!(id.len(), 36, "expected UUID, got: {id}");
    }

    #[tokio::test]
    async fn test_create_defaults_to_backlog() {
        let store = Arc::new(TaskStore::new_in_memory());
        let t = TaskTool::new(Arc::clone(&store));
        let out = t
            .execute(json!({"action": "create", "title": "No col"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.starts_with("Created task "), "got: {out}");
        let tasks = store.list(None).await;
        assert_eq!(tasks[0].column, "backlog");
    }

    #[tokio::test]
    async fn test_create_with_assignee() {
        let store = Arc::new(TaskStore::new_in_memory());
        let t = TaskTool::new(Arc::clone(&store));
        t.execute(
            json!({"action": "create", "title": "Assigned", "column": "backlog", "assignee": "dan"}),
            &ctx(),
        )
        .await
        .unwrap();
        let tasks = store.list(None).await;
        assert_eq!(tasks[0].assignee.as_deref(), Some("dan"));
    }

    #[tokio::test]
    async fn test_create_invalid_column_reports_error() {
        let t = make_tool();
        let out = t
            .execute(
                json!({"action": "create", "title": "Bad", "column": "wip"}),
                &ctx(),
            )
            .await
            .unwrap()
            .for_llm;
        assert!(out.starts_with("Error:"), "got: {out}");
    }

    // ── update ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_update_title() {
        let store = Arc::new(TaskStore::new_in_memory());
        let t = TaskTool::new(Arc::clone(&store));
        let id = store.create("Old", "backlog", None).await.unwrap();

        let out = t
            .execute(
                json!({"action": "update", "id": id, "title": "New"}),
                &ctx(),
            )
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains(&id), "got: {out}");
        assert_eq!(store.get(&id).await.unwrap().title, "New");
    }

    #[tokio::test]
    async fn test_update_missing_id() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "update", "title": "x"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("'id' is required"), "got: {out}");
    }

    #[tokio::test]
    async fn test_update_nonexistent_task() {
        let t = make_tool();
        let out = t
            .execute(
                json!({"action": "update", "id": "no-such-id", "title": "x"}),
                &ctx(),
            )
            .await
            .unwrap()
            .for_llm;
        assert!(out.starts_with("Error:"), "got: {out}");
    }

    // ── move ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_move_task() {
        let store = Arc::new(TaskStore::new_in_memory());
        let t = TaskTool::new(Arc::clone(&store));
        let id = store.create("Task", "backlog", None).await.unwrap();

        let out = t
            .execute(
                json!({"action": "move", "id": id, "column": "in_progress"}),
                &ctx(),
            )
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("in_progress"), "got: {out}");
        assert_eq!(store.get(&id).await.unwrap().column, "in_progress");
    }

    #[tokio::test]
    async fn test_move_missing_id() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "move", "column": "done"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("'id' is required"), "got: {out}");
    }

    #[tokio::test]
    async fn test_move_missing_column() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "move", "id": "some-id"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("'column' is required"), "got: {out}");
    }

    #[tokio::test]
    async fn test_move_invalid_column() {
        let store = Arc::new(TaskStore::new_in_memory());
        let t = TaskTool::new(Arc::clone(&store));
        let id = store.create("T", "backlog", None).await.unwrap();

        let out = t
            .execute(json!({"action": "move", "id": id, "column": "wip"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.starts_with("Error:"), "got: {out}");
    }

    // ── delete ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_task() {
        let store = Arc::new(TaskStore::new_in_memory());
        let t = TaskTool::new(Arc::clone(&store));
        let id = store.create("Gone", "done", None).await.unwrap();

        let out = t
            .execute(json!({"action": "delete", "id": id}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("Deleted task"), "got: {out}");
        assert!(store.get(&id).await.is_none());
    }

    #[tokio::test]
    async fn test_delete_missing_id() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "delete"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("'id' is required"), "got: {out}");
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "delete", "id": "ghost"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.starts_with("Error:"), "got: {out}");
    }

    // ── unknown action ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_unknown_action() {
        let t = make_tool();
        let out = t
            .execute(json!({"action": "frobnicate"}), &ctx())
            .await
            .unwrap()
            .for_llm;
        assert!(out.contains("Unknown action 'frobnicate'"), "got: {out}");
    }

    #[tokio::test]
    async fn test_missing_action_defaults_to_unknown() {
        let t = make_tool();
        let out = t.execute(json!({}), &ctx()).await.unwrap().for_llm;
        // Empty string action falls into the unknown arm
        assert!(out.contains("Unknown action"), "got: {out}");
    }
}
