//! Kanban task model and JSON persistence.
//!
//! Provides [`KanbanTask`] and [`TaskStore`] — an async, `Arc`-cloneable store
//! backed by an optional JSON file. All mutating operations persist atomically
//! (write full file after every change).
//!
//! # Example
//!
//! ```rust
//! # tokio_test::block_on(async {
//! use zeptoclaw::api::tasks::TaskStore;
//!
//! let store = TaskStore::new_in_memory();
//! let id = store.create("Buy milk", "backlog", None).await.unwrap();
//! let tasks = store.list(None).await;
//! assert_eq!(tasks.len(), 1);
//! assert_eq!(tasks[0].title, "Buy milk");
//! # });
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// The valid kanban column identifiers.
pub const COLUMNS: &[&str] = &["backlog", "in_progress", "review", "done"];

/// A single task on the kanban board.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KanbanTask {
    /// Unique UUID v4 identifier.
    pub id: String,
    /// Short title displayed on the card.
    pub title: String,
    /// Optional longer description.
    #[serde(default)]
    pub description: String,
    /// Current column (`backlog`, `in_progress`, `review`, `done`).
    pub column: String,
    /// Who the task is assigned to (free-form string).
    pub assignee: Option<String>,
    /// Priority label (`low`, `medium`, `high`, `urgent` — free-form).
    pub priority: Option<String>,
    /// Arbitrary labels / tags.
    #[serde(default)]
    pub labels: Vec<String>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 last-updated timestamp.
    pub updated_at: String,
}

/// Async task store backed by an in-memory map and optional JSON file.
///
/// Clone is cheap — all clones share the same `Arc<RwLock<_>>`.
#[derive(Clone)]
pub struct TaskStore {
    tasks: Arc<RwLock<HashMap<String, KanbanTask>>>,
    path: Option<PathBuf>,
}

impl TaskStore {
    /// Create a store backed by a JSON file at `path`.
    ///
    /// Call [`load`](Self::load) after construction to restore previously
    /// persisted tasks.
    pub fn new(path: PathBuf) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            path: Some(path),
        }
    }

    /// Create an in-memory-only store (useful for tests).
    pub fn new_in_memory() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            path: None,
        }
    }

    /// Load tasks from the backing JSON file.
    ///
    /// No-op if the file does not exist or this is an in-memory store.
    /// Returns an error string if the file exists but cannot be read or parsed.
    pub async fn load(&self) -> Result<(), String> {
        let Some(ref path) = self.path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let data = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| e.to_string())?;
        let tasks: Vec<KanbanTask> = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let mut map = self.tasks.write().await;
        for t in tasks {
            map.insert(t.id.clone(), t);
        }
        Ok(())
    }

    /// Persist the current in-memory state to disk.
    ///
    /// Creates parent directories if they do not exist.
    /// No-op for in-memory stores.
    async fn save(&self) -> Result<(), String> {
        let Some(ref path) = self.path else {
            return Ok(());
        };
        let map = self.tasks.read().await;
        let mut tasks: Vec<&KanbanTask> = map.values().collect();
        // Stable ordering: creation timestamp ascending.
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let data = serde_json::to_string_pretty(&tasks).map_err(|e| e.to_string())?;
        drop(map);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(path, data)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Return all tasks, optionally filtered to a single column.
    ///
    /// Results are sorted by `created_at` ascending.
    pub async fn list(&self, column_filter: Option<&str>) -> Vec<KanbanTask> {
        let map = self.tasks.read().await;
        let mut tasks: Vec<KanbanTask> = if let Some(col) = column_filter {
            map.values().filter(|t| t.column == col).cloned().collect()
        } else {
            map.values().cloned().collect()
        };
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        tasks
    }

    /// Return a single task by `id`, or `None` if not found.
    pub async fn get(&self, id: &str) -> Option<KanbanTask> {
        self.tasks.read().await.get(id).cloned()
    }

    /// Create a new task and return its generated ID.
    ///
    /// Returns an error if `column` is not one of the valid [`COLUMNS`].
    pub async fn create(
        &self,
        title: &str,
        column: &str,
        assignee: Option<String>,
    ) -> Result<String, String> {
        if !COLUMNS.contains(&column) {
            return Err(format!(
                "Invalid column '{column}'. Valid columns: {COLUMNS:?}"
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let task = KanbanTask {
            id: id.clone(),
            title: title.to_string(),
            description: String::new(),
            column: column.to_string(),
            assignee,
            priority: None,
            labels: vec![],
            created_at: now.clone(),
            updated_at: now,
        };
        self.tasks.write().await.insert(id.clone(), task);
        self.save().await?;
        Ok(id)
    }

    /// Apply a JSON patch to a task's mutable fields.
    ///
    /// Recognised keys: `title`, `description`, `assignee`, `priority`.
    /// Unknown keys are silently ignored so the caller can pass the full args
    /// object from the agent without pre-filtering.
    pub async fn update(&self, id: &str, updates: serde_json::Value) -> Result<(), String> {
        {
            let mut map = self.tasks.write().await;
            let task = map
                .get_mut(id)
                .ok_or_else(|| format!("Task not found: {id}"))?;
            if let Some(title) = updates.get("title").and_then(|v| v.as_str()) {
                task.title = title.to_string();
            }
            if let Some(desc) = updates.get("description").and_then(|v| v.as_str()) {
                task.description = desc.to_string();
            }
            if let Some(assignee) = updates.get("assignee").and_then(|v| v.as_str()) {
                task.assignee = Some(assignee.to_string());
            }
            if let Some(priority) = updates.get("priority").and_then(|v| v.as_str()) {
                task.priority = Some(priority.to_string());
            }
            task.updated_at = chrono::Utc::now().to_rfc3339();
        }
        self.save().await
    }

    /// Move a task to a different column.
    ///
    /// Returns an error if `column` is not one of the valid [`COLUMNS`] or
    /// if the task does not exist.
    pub async fn move_task(&self, id: &str, column: &str) -> Result<(), String> {
        if !COLUMNS.contains(&column) {
            return Err(format!(
                "Invalid column '{column}'. Valid columns: {COLUMNS:?}"
            ));
        }
        {
            let mut map = self.tasks.write().await;
            let task = map
                .get_mut(id)
                .ok_or_else(|| format!("Task not found: {id}"))?;
            task.column = column.to_string();
            task.updated_at = chrono::Utc::now().to_rfc3339();
        }
        self.save().await
    }

    /// Delete a task by ID.
    ///
    /// Returns an error if the task does not exist.
    pub async fn delete(&self, id: &str) -> Result<(), String> {
        {
            let mut map = self.tasks.write().await;
            map.remove(id)
                .ok_or_else(|| format!("Task not found: {id}"))?;
        }
        self.save().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store_with_dir() -> (TaskStore, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("tasks.json");
        (TaskStore::new(path), dir)
    }

    // ── create / list ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_and_list() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Write tests", "backlog", None).await.unwrap();
        let tasks = store.list(None).await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, id);
        assert_eq!(tasks[0].title, "Write tests");
        assert_eq!(tasks[0].column, "backlog");
        assert!(tasks[0].assignee.is_none());
    }

    #[tokio::test]
    async fn test_create_with_assignee() {
        let store = TaskStore::new_in_memory();
        store
            .create("Design API", "in_progress", Some("alice".to_string()))
            .await
            .unwrap();
        let tasks = store.list(None).await;
        assert_eq!(tasks[0].assignee.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn test_list_multiple_sorted_by_created_at() {
        let store = TaskStore::new_in_memory();
        let id1 = store.create("Task A", "backlog", None).await.unwrap();
        let id2 = store.create("Task B", "backlog", None).await.unwrap();
        let id3 = store.create("Task C", "done", None).await.unwrap();

        let all = store.list(None).await;
        assert_eq!(all.len(), 3);
        // created_at sort: first inserted should be ≤ later ones
        assert!(all[0].id == id1 || all[0].created_at <= all[1].created_at);
        let _ = id2;
        let _ = id3;
    }

    // ── invalid column on create ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_invalid_column_rejected() {
        let store = TaskStore::new_in_memory();
        let err = store
            .create("Bad task", "invalid_col", None)
            .await
            .unwrap_err();
        assert!(err.contains("Invalid column"), "got: {err}");
        // No task should have been inserted
        assert!(store.list(None).await.is_empty());
    }

    // ── list with column filter ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_with_column_filter() {
        let store = TaskStore::new_in_memory();
        store.create("A", "backlog", None).await.unwrap();
        store.create("B", "backlog", None).await.unwrap();
        store.create("C", "done", None).await.unwrap();

        let backlog = store.list(Some("backlog")).await;
        assert_eq!(backlog.len(), 2);
        assert!(backlog.iter().all(|t| t.column == "backlog"));

        let done = store.list(Some("done")).await;
        assert_eq!(done.len(), 1);

        let review = store.list(Some("review")).await;
        assert!(review.is_empty());
    }

    // ── get ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_existing() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Get me", "backlog", None).await.unwrap();
        let task = store.get(&id).await.unwrap();
        assert_eq!(task.title, "Get me");
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let store = TaskStore::new_in_memory();
        assert!(store.get("does-not-exist").await.is_none());
    }

    // ── move ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_move_task_between_columns() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Move me", "backlog", None).await.unwrap();

        store.move_task(&id, "in_progress").await.unwrap();
        let task = store.get(&id).await.unwrap();
        assert_eq!(task.column, "in_progress");

        store.move_task(&id, "review").await.unwrap();
        let task = store.get(&id).await.unwrap();
        assert_eq!(task.column, "review");

        store.move_task(&id, "done").await.unwrap();
        let task = store.get(&id).await.unwrap();
        assert_eq!(task.column, "done");
    }

    #[tokio::test]
    async fn test_move_invalid_column() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Task", "backlog", None).await.unwrap();
        let err = store.move_task(&id, "wip").await.unwrap_err();
        assert!(err.contains("Invalid column"), "got: {err}");
        // Column unchanged
        assert_eq!(store.get(&id).await.unwrap().column, "backlog");
    }

    #[tokio::test]
    async fn test_move_nonexistent_task() {
        let store = TaskStore::new_in_memory();
        let err = store.move_task("ghost-id", "done").await.unwrap_err();
        assert!(err.contains("Task not found"), "got: {err}");
    }

    // ── update ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_update_title() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Old title", "backlog", None).await.unwrap();
        store
            .update(&id, serde_json::json!({"title": "New title"}))
            .await
            .unwrap();
        assert_eq!(store.get(&id).await.unwrap().title, "New title");
    }

    #[tokio::test]
    async fn test_update_description() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Task", "backlog", None).await.unwrap();
        store
            .update(&id, serde_json::json!({"description": "Some details"}))
            .await
            .unwrap();
        assert_eq!(store.get(&id).await.unwrap().description, "Some details");
    }

    #[tokio::test]
    async fn test_update_assignee() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Task", "backlog", None).await.unwrap();
        store
            .update(&id, serde_json::json!({"assignee": "bob"}))
            .await
            .unwrap();
        assert_eq!(
            store.get(&id).await.unwrap().assignee.as_deref(),
            Some("bob")
        );
    }

    #[tokio::test]
    async fn test_update_priority() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Task", "backlog", None).await.unwrap();
        store
            .update(&id, serde_json::json!({"priority": "high"}))
            .await
            .unwrap();
        assert_eq!(
            store.get(&id).await.unwrap().priority.as_deref(),
            Some("high")
        );
    }

    #[tokio::test]
    async fn test_update_multiple_fields() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Task", "backlog", None).await.unwrap();
        store
            .update(
                &id,
                serde_json::json!({
                    "title": "Updated",
                    "description": "Details",
                    "assignee": "carol",
                    "priority": "urgent"
                }),
            )
            .await
            .unwrap();
        let task = store.get(&id).await.unwrap();
        assert_eq!(task.title, "Updated");
        assert_eq!(task.description, "Details");
        assert_eq!(task.assignee.as_deref(), Some("carol"));
        assert_eq!(task.priority.as_deref(), Some("urgent"));
    }

    #[tokio::test]
    async fn test_update_nonexistent_task() {
        let store = TaskStore::new_in_memory();
        let err = store
            .update("nope", serde_json::json!({"title": "x"}))
            .await
            .unwrap_err();
        assert!(err.contains("Task not found"), "got: {err}");
    }

    #[tokio::test]
    async fn test_update_unknown_keys_ignored() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Task", "backlog", None).await.unwrap();
        // Should not error on unknown keys like "action" or "column"
        store
            .update(
                &id,
                serde_json::json!({"action": "update", "column": "done"}),
            )
            .await
            .unwrap();
        // column is not updated by update() — only move_task() changes column
        assert_eq!(store.get(&id).await.unwrap().column, "backlog");
    }

    // ── delete ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete() {
        let store = TaskStore::new_in_memory();
        let id = store.create("Delete me", "backlog", None).await.unwrap();
        store.delete(&id).await.unwrap();
        assert!(store.get(&id).await.is_none());
        assert!(store.list(None).await.is_empty());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let store = TaskStore::new_in_memory();
        let err = store.delete("ghost").await.unwrap_err();
        assert!(err.contains("Task not found"), "got: {err}");
    }

    // ── persistence (file-backed store) ─────────────────────────────────────

    #[tokio::test]
    async fn test_persist_and_reload() {
        let (store, dir) = store_with_dir();
        let id = store.create("Persisted", "review", None).await.unwrap();

        // Reload from same path
        let store2 = TaskStore::new(dir.path().join("tasks.json"));
        store2.load().await.unwrap();

        let task = store2.get(&id).await.unwrap();
        assert_eq!(task.title, "Persisted");
        assert_eq!(task.column, "review");
    }

    #[tokio::test]
    async fn test_load_nonexistent_file_is_ok() {
        let dir = TempDir::new().unwrap();
        let store = TaskStore::new(dir.path().join("missing.json"));
        // Should succeed silently
        store.load().await.unwrap();
        assert!(store.list(None).await.is_empty());
    }

    #[tokio::test]
    async fn test_in_memory_save_is_noop() {
        let store = TaskStore::new_in_memory();
        // No file path — save should never fail
        let id = store.create("Ephemeral", "done", None).await.unwrap();
        store.delete(&id).await.unwrap();
        assert!(store.list(None).await.is_empty());
    }

    // ── COLUMNS constant ─────────────────────────────────────────────────────

    #[test]
    fn test_columns_constant() {
        assert!(COLUMNS.contains(&"backlog"));
        assert!(COLUMNS.contains(&"in_progress"));
        assert!(COLUMNS.contains(&"review"));
        assert!(COLUMNS.contains(&"done"));
        assert_eq!(COLUMNS.len(), 4);
    }
}
