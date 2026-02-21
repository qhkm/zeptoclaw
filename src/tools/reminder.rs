//! Reminder tool — persistent scheduled reminders.
//!
//! Provides a `ReminderStore` for CRUD operations on reminders with
//! JSON persistence, and a `ReminderTool` implementing the `Tool` trait
//! with 6 actions: add, list, complete, snooze, remove, overdue.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::cron::{CronPayload, CronSchedule, CronService};
use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the current unix epoch timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Default category for reminders without an explicit category.
fn default_category() -> String {
    "general".to_string()
}

/// Parse an ISO 8601 datetime string into a unix epoch timestamp (seconds).
///
/// Accepts RFC 3339 (e.g. `2026-03-01T09:00:00Z`) or date-only
/// (e.g. `2026-03-01`, treated as midnight UTC).
fn parse_iso_to_epoch(s: &str) -> Result<u64> {
    // Try RFC 3339 first (2026-03-01T09:00:00Z)
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        let ts = dt.timestamp();
        if ts < 0 {
            return Err(ZeptoError::Tool(format!(
                "Date '{}' is before Unix epoch",
                s
            )));
        }
        return Ok(ts as u64);
    }
    // Try date-only (2026-03-01) — assume midnight UTC
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ZeptoError::Tool(format!("Invalid date '{}'", s)))?;
        let ts = dt.and_utc().timestamp();
        if ts < 0 {
            return Err(ZeptoError::Tool(format!(
                "Date '{}' is before Unix epoch",
                s
            )));
        }
        return Ok(ts as u64);
    }
    Err(ZeptoError::Tool(format!(
        "Cannot parse '{}' as ISO 8601 datetime",
        s
    )))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Status of a reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderStatus {
    Pending,
    Done,
    Snoozed,
}

/// A single reminder entry with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderEntry {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_category")]
    pub category: String,
    pub status: ReminderStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_job_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// ReminderStore
// ---------------------------------------------------------------------------

/// Persistent store for reminders, backed by a JSON file.
#[derive(Debug)]
pub struct ReminderStore {
    entries: HashMap<String, ReminderEntry>,
    storage_path: PathBuf,
    next_id: u64,
}

impl ReminderStore {
    /// Create a new store at the default path (`~/.zeptoclaw/reminders.json`).
    pub fn new() -> Result<Self> {
        let path = Config::dir().join("reminders.json");
        Self::with_path(path)
    }

    /// Create a store at a custom path. Useful for testing.
    pub fn with_path(path: PathBuf) -> Result<Self> {
        let entries = Self::load(&path)?;
        // Derive next_id from the highest existing numeric suffix.
        let max_id = entries
            .keys()
            .filter_map(|k| k.strip_prefix('r').and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        Ok(Self {
            entries,
            storage_path: path,
            next_id: max_id + 1,
        })
    }

    /// Add a new reminder and persist it immediately.
    pub fn add(
        &mut self,
        title: &str,
        description: Option<&str>,
        category: &str,
        due_at: Option<u64>,
        recurrence: Option<&str>,
    ) -> Result<ReminderEntry> {
        let id = format!("r{}", self.next_id);
        self.next_id += 1;
        let now = now_secs();

        let entry = ReminderEntry {
            id: id.clone(),
            title: title.to_string(),
            description: description.map(str::to_string),
            category: if category.is_empty() {
                default_category()
            } else {
                category.to_string()
            },
            status: ReminderStatus::Pending,
            due_at,
            recurrence: recurrence.map(str::to_string),
            cron_job_id: None,
            created_at: now,
            updated_at: now,
        };

        self.entries.insert(id, entry.clone());
        self.save()?;
        Ok(entry)
    }

    /// Lookup a reminder by id.
    pub fn get(&self, id: &str) -> Option<&ReminderEntry> {
        self.entries.get(id)
    }

    /// List reminders with optional status and category filters.
    pub fn list(
        &self,
        status_filter: Option<&ReminderStatus>,
        category_filter: Option<&str>,
    ) -> Vec<&ReminderEntry> {
        let mut results: Vec<&ReminderEntry> = self
            .entries
            .values()
            .filter(|e| {
                if let Some(status) = status_filter {
                    if &e.status != status {
                        return false;
                    }
                }
                if let Some(cat) = category_filter {
                    if !e.category.eq_ignore_ascii_case(cat) {
                        return false;
                    }
                }
                true
            })
            .collect();
        results.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        results
    }

    /// Mark a reminder as done.
    pub fn complete(&mut self, id: &str) -> Result<bool> {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.status = ReminderStatus::Done;
            entry.updated_at = now_secs();
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Snooze a reminder to a new due time.
    pub fn snooze(&mut self, id: &str, new_due_at: u64) -> Result<bool> {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.status = ReminderStatus::Snoozed;
            entry.due_at = Some(new_due_at);
            entry.updated_at = now_secs();
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Remove a reminder entirely.
    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let existed = self.entries.remove(id).is_some();
        if existed {
            self.save()?;
        }
        Ok(existed)
    }

    /// Return all pending reminders whose `due_at` is in the past.
    pub fn overdue(&self) -> Vec<&ReminderEntry> {
        let now = now_secs();
        let mut results: Vec<&ReminderEntry> = self
            .entries
            .values()
            .filter(|e| {
                e.status == ReminderStatus::Pending && e.due_at.is_some_and(|due| due < now)
            })
            .collect();
        results.sort_by(|a, b| a.due_at.cmp(&b.due_at));
        results
    }

    /// Associate a cron job id with a reminder.
    pub fn set_cron_job_id(&mut self, id: &str, cron_job_id: &str) -> Result<bool> {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.cron_job_id = Some(cron_job_id.to_string());
            entry.updated_at = now_secs();
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Number of stored reminders.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // -- persistence --

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ZeptoError::Tool(format!(
                    "Failed to create reminders directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| ZeptoError::Tool(format!("Failed to serialize reminders: {}", e)))?;

        std::fs::write(&self.storage_path, json).map_err(|e| {
            ZeptoError::Tool(format!(
                "Failed to write reminders to {}: {}",
                self.storage_path.display(),
                e
            ))
        })?;

        Ok(())
    }

    fn load(path: &PathBuf) -> Result<HashMap<String, ReminderEntry>> {
        if !path.exists() {
            return Ok(HashMap::new());
        }

        let content = std::fs::read_to_string(path).map_err(|e| {
            ZeptoError::Tool(format!(
                "Failed to read reminders from {}: {}",
                path.display(),
                e
            ))
        })?;

        if content.trim().is_empty() {
            return Ok(HashMap::new());
        }

        let entries: HashMap<String, ReminderEntry> = serde_json::from_str(&content)
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse reminders JSON: {}", e)))?;

        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// ReminderTool
// ---------------------------------------------------------------------------

/// Agent tool for managing persistent reminders.
pub struct ReminderTool {
    store: Arc<Mutex<ReminderStore>>,
    cron: Option<Arc<CronService>>,
}

impl ReminderTool {
    /// Create a new reminder tool at the default storage path.
    pub fn new(cron: Option<Arc<CronService>>) -> Result<Self> {
        let store = ReminderStore::new()?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            cron,
        })
    }

    /// Create a reminder tool with a pre-existing store. Useful for testing.
    pub fn with_store(store: Arc<Mutex<ReminderStore>>, cron: Option<Arc<CronService>>) -> Self {
        Self { store, cron }
    }
}

#[async_trait]
impl Tool for ReminderTool {
    fn name(&self) -> &str {
        "reminder"
    }

    fn description(&self) -> &str {
        "Manage persistent reminders. Actions: add, list, complete, snooze, remove, overdue."
    }

    fn compact_description(&self) -> &str {
        "Reminders"
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
                    "enum": ["add", "list", "complete", "snooze", "remove", "overdue"],
                    "description": "Action to perform"
                },
                "title": {
                    "type": "string",
                    "description": "Reminder title (required for add)"
                },
                "description": {
                    "type": "string",
                    "description": "Optional longer description"
                },
                "category": {
                    "type": "string",
                    "description": "Category for grouping (default: general)"
                },
                "due_at": {
                    "type": "string",
                    "description": "ISO 8601 datetime or date (e.g. 2026-03-01T09:00:00Z or 2026-03-01)"
                },
                "recurrence": {
                    "type": "string",
                    "description": "Cron expression for recurring reminders (e.g. '0 9 * * 1' for every Monday 9am)"
                },
                "id": {
                    "type": "string",
                    "description": "Reminder id (for complete, snooze, remove)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "done", "snoozed"],
                    "description": "Filter by status (for list)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' argument".into()))?;

        let s = match action {
            "add" => self.execute_add(&args, ctx).await?,
            "list" => self.execute_list(&args).await?,
            "complete" => self.execute_complete(&args).await?,
            "snooze" => self.execute_snooze(&args).await?,
            "remove" => self.execute_remove(&args).await?,
            "overdue" => self.execute_overdue().await?,
            other => {
                return Err(ZeptoError::Tool(format!(
                    "Unknown reminder action '{}'",
                    other
                )))
            }
        };
        Ok(ToolOutput::llm_only(s))
    }
}

impl ReminderTool {
    async fn execute_add(&self, args: &Value, ctx: &ToolContext) -> Result<String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'title' for reminder add".into()))?;

        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("general");

        let due_at = if let Some(due_str) = args.get("due_at").and_then(|v| v.as_str()) {
            Some(parse_iso_to_epoch(due_str)?)
        } else {
            None
        };

        let recurrence = args
            .get("recurrence")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let entry = {
            let mut store = self.store.lock().await;
            store.add(title, description, category, due_at, recurrence)?
        };

        // If we have a cron service, a due_at, and a channel context, schedule
        // a one-shot cron job to deliver the reminder.
        if let (Some(cron), Some(due_epoch)) = (&self.cron, due_at) {
            if let (Some(channel), Some(chat_id)) = (&ctx.channel, &ctx.chat_id) {
                let schedule = CronSchedule::At {
                    at_ms: (due_epoch as i64) * 1000,
                };
                let payload = CronPayload {
                    message: format!("Reminder: {}", entry.title),
                    channel: channel.clone(),
                    chat_id: chat_id.clone(),
                };
                match cron
                    .add_job(entry.title.clone(), schedule, payload, true)
                    .await
                {
                    Ok(job) => {
                        let mut store = self.store.lock().await;
                        let _ = store.set_cron_job_id(&entry.id, &job.id);
                    }
                    Err(_) => {
                        // Cron scheduling is best-effort; the reminder is still
                        // persisted even if the cron job fails.
                    }
                }
            }
        }

        let due_info = if let Some(d) = due_at {
            format!(" (due at {})", d)
        } else {
            String::new()
        };

        Ok(format!(
            "Created reminder '{}' [{}]{}",
            entry.title, entry.id, due_info
        ))
    }

    async fn execute_list(&self, args: &Value) -> Result<String> {
        let status_filter = args
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "pending" => Some(ReminderStatus::Pending),
                "done" => Some(ReminderStatus::Done),
                "snoozed" => Some(ReminderStatus::Snoozed),
                _ => None,
            });

        let category_filter = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let store = self.store.lock().await;
        let items = store.list(status_filter.as_ref(), category_filter);

        if items.is_empty() {
            return Ok("No reminders found".to_string());
        }

        let mut lines = Vec::new();
        for item in &items {
            let emoji = match item.status {
                ReminderStatus::Pending => "\u{23f3}",  // hourglass
                ReminderStatus::Done => "\u{2705}",     // check mark
                ReminderStatus::Snoozed => "\u{1f4a4}", // zzz
            };
            let due_info = item
                .due_at
                .map(|d| format!(" (due: {})", d))
                .unwrap_or_default();
            lines.push(format!(
                "{} [{}] {}{}  [{}]",
                emoji, item.id, item.title, due_info, item.category
            ));
        }

        Ok(format!(
            "{} reminder{}:\n{}",
            items.len(),
            if items.len() == 1 { "" } else { "s" },
            lines.join("\n")
        ))
    }

    async fn execute_complete(&self, args: &Value) -> Result<String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'id' for reminder complete".into()))?;

        // Cancel associated cron job if present.
        let cron_job_id = {
            let store = self.store.lock().await;
            store.get(id).and_then(|e| e.cron_job_id.clone())
        };
        if let (Some(cron), Some(job_id)) = (&self.cron, cron_job_id) {
            let _ = cron.remove_job(&job_id).await;
        }

        let mut store = self.store.lock().await;
        if store.complete(id)? {
            Ok(format!("Completed reminder {}", id))
        } else {
            Ok(format!("Reminder {} not found", id))
        }
    }

    async fn execute_snooze(&self, args: &Value) -> Result<String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'id' for reminder snooze".into()))?;

        let due_str = args
            .get("due_at")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'due_at' for reminder snooze".into()))?;

        let new_due_at = parse_iso_to_epoch(due_str)?;

        let mut store = self.store.lock().await;
        if store.snooze(id, new_due_at)? {
            Ok(format!("Snoozed reminder {} until {}", id, new_due_at))
        } else {
            Ok(format!("Reminder {} not found", id))
        }
    }

    async fn execute_remove(&self, args: &Value) -> Result<String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'id' for reminder remove".into()))?;

        // Cancel associated cron job if present.
        let cron_job_id = {
            let store = self.store.lock().await;
            store.get(id).and_then(|e| e.cron_job_id.clone())
        };
        if let (Some(cron), Some(job_id)) = (&self.cron, cron_job_id) {
            let _ = cron.remove_job(&job_id).await;
        }

        let mut store = self.store.lock().await;
        if store.remove(id)? {
            Ok(format!("Removed reminder {}", id))
        } else {
            Ok(format!("Reminder {} not found", id))
        }
    }

    async fn execute_overdue(&self) -> Result<String> {
        let store = self.store.lock().await;
        let items = store.overdue();

        if items.is_empty() {
            return Ok("No overdue reminders".to_string());
        }

        let mut lines = Vec::new();
        for item in &items {
            let due_info = item
                .due_at
                .map(|d| format!(" (was due: {})", d))
                .unwrap_or_default();
            lines.push(format!(
                "\u{26a0}\u{fe0f} [{}] {}{}  [{}]",
                item.id, item.title, due_info, item.category
            ));
        }

        Ok(format!(
            "{} overdue reminder{}:\n{}",
            items.len(),
            if items.len() == 1 { "" } else { "s" },
            lines.join("\n")
        ))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a ReminderStore backed by a temp directory.
    fn temp_store() -> (ReminderStore, TempDir) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("reminders.json");
        let store = ReminderStore::with_path(path).expect("failed to create store");
        (store, dir)
    }

    /// Helper: create a ReminderTool backed by a temp store (no cron).
    fn temp_tool() -> (ReminderTool, TempDir) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("reminders.json");
        let store = ReminderStore::with_path(path).expect("failed to create store");
        let tool = ReminderTool::with_store(Arc::new(Mutex::new(store)), None);
        (tool, dir)
    }

    fn ctx() -> ToolContext {
        ToolContext::new()
    }

    // ---- Type tests ----

    #[test]
    fn test_reminder_status_serialize() {
        let pending = serde_json::to_string(&ReminderStatus::Pending).unwrap();
        assert_eq!(pending, "\"pending\"");
        let done = serde_json::to_string(&ReminderStatus::Done).unwrap();
        assert_eq!(done, "\"done\"");
        let snoozed = serde_json::to_string(&ReminderStatus::Snoozed).unwrap();
        assert_eq!(snoozed, "\"snoozed\"");
    }

    #[test]
    fn test_reminder_status_deserialize() {
        let pending: ReminderStatus = serde_json::from_str("\"pending\"").unwrap();
        assert_eq!(pending, ReminderStatus::Pending);
        let done: ReminderStatus = serde_json::from_str("\"done\"").unwrap();
        assert_eq!(done, ReminderStatus::Done);
        let snoozed: ReminderStatus = serde_json::from_str("\"snoozed\"").unwrap();
        assert_eq!(snoozed, ReminderStatus::Snoozed);
    }

    #[test]
    fn test_reminder_entry_serialize_roundtrip() {
        let entry = ReminderEntry {
            id: "r1".to_string(),
            title: "Buy milk".to_string(),
            description: Some("From the store".to_string()),
            category: "shopping".to_string(),
            status: ReminderStatus::Pending,
            due_at: Some(1700000000),
            recurrence: None,
            cron_job_id: None,
            created_at: 1699999000,
            updated_at: 1699999000,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let roundtripped: ReminderEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtripped.id, "r1");
        assert_eq!(roundtripped.title, "Buy milk");
        assert_eq!(roundtripped.description, Some("From the store".to_string()));
        assert_eq!(roundtripped.category, "shopping");
        assert_eq!(roundtripped.status, ReminderStatus::Pending);
        assert_eq!(roundtripped.due_at, Some(1700000000));
        assert!(roundtripped.recurrence.is_none());
        assert!(roundtripped.cron_job_id.is_none());
    }

    #[test]
    fn test_reminder_entry_default_category() {
        let json = r#"{
            "id": "r1",
            "title": "Test",
            "status": "pending",
            "created_at": 1000,
            "updated_at": 1000
        }"#;
        let entry: ReminderEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.category, "general");
    }

    #[test]
    fn test_reminder_entry_skip_none_fields() {
        let entry = ReminderEntry {
            id: "r1".to_string(),
            title: "Test".to_string(),
            description: None,
            category: "general".to_string(),
            status: ReminderStatus::Pending,
            due_at: None,
            recurrence: None,
            cron_job_id: None,
            created_at: 1000,
            updated_at: 1000,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("description"));
        assert!(!json.contains("due_at"));
        assert!(!json.contains("recurrence"));
        assert!(!json.contains("cron_job_id"));
    }

    #[test]
    fn test_now_secs_returns_reasonable_value() {
        let ts = now_secs();
        // Should be after 2024-01-01
        assert!(ts > 1_704_067_200);
        // Should be before 2100-01-01
        assert!(ts < 4_102_444_800);
    }

    // ---- Store tests ----

    #[test]
    fn test_store_new_empty() {
        let (store, _dir) = temp_store();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_store_add_and_get() {
        let (mut store, _dir) = temp_store();
        let entry = store
            .add("Buy milk", Some("2% milk"), "shopping", None, None)
            .unwrap();

        assert_eq!(entry.title, "Buy milk");
        assert_eq!(entry.description, Some("2% milk".to_string()));
        assert_eq!(entry.category, "shopping");
        assert_eq!(entry.status, ReminderStatus::Pending);
        assert_eq!(store.len(), 1);

        let retrieved = store.get(&entry.id).unwrap();
        assert_eq!(retrieved.title, "Buy milk");
    }

    #[test]
    fn test_store_add_increments_id() {
        let (mut store, _dir) = temp_store();
        let e1 = store.add("First", None, "general", None, None).unwrap();
        let e2 = store.add("Second", None, "general", None, None).unwrap();
        let e3 = store.add("Third", None, "general", None, None).unwrap();

        assert_eq!(e1.id, "r1");
        assert_eq!(e2.id, "r2");
        assert_eq!(e3.id, "r3");
    }

    #[test]
    fn test_store_persistence_roundtrip() {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("reminders.json");

        // Create and populate.
        {
            let mut store = ReminderStore::with_path(path.clone()).unwrap();
            store.add("Task A", None, "work", None, None).unwrap();
            store
                .add(
                    "Task B",
                    Some("details"),
                    "personal",
                    Some(2000000000),
                    None,
                )
                .unwrap();
        }

        // Reload and verify.
        {
            let store = ReminderStore::with_path(path).unwrap();
            assert_eq!(store.len(), 2);
            let entry = store.get("r1").unwrap();
            assert_eq!(entry.title, "Task A");
            let entry2 = store.get("r2").unwrap();
            assert_eq!(entry2.title, "Task B");
            assert_eq!(entry2.description, Some("details".to_string()));
        }
    }

    #[test]
    fn test_store_complete() {
        let (mut store, _dir) = temp_store();
        let entry = store
            .add("Do something", None, "general", None, None)
            .unwrap();

        assert!(store.complete(&entry.id).unwrap());
        let updated = store.get(&entry.id).unwrap();
        assert_eq!(updated.status, ReminderStatus::Done);
    }

    #[test]
    fn test_store_complete_not_found() {
        let (mut store, _dir) = temp_store();
        assert!(!store.complete("r999").unwrap());
    }

    #[test]
    fn test_store_snooze() {
        let (mut store, _dir) = temp_store();
        let entry = store
            .add("Task", None, "general", Some(1700000000), None)
            .unwrap();

        assert!(store.snooze(&entry.id, 1800000000).unwrap());
        let updated = store.get(&entry.id).unwrap();
        assert_eq!(updated.status, ReminderStatus::Snoozed);
        assert_eq!(updated.due_at, Some(1800000000));
    }

    #[test]
    fn test_store_remove() {
        let (mut store, _dir) = temp_store();
        let entry = store.add("Delete me", None, "general", None, None).unwrap();
        assert_eq!(store.len(), 1);

        assert!(store.remove(&entry.id).unwrap());
        assert_eq!(store.len(), 0);
        assert!(store.get(&entry.id).is_none());
    }

    #[test]
    fn test_store_list_all() {
        let (mut store, _dir) = temp_store();
        store.add("A", None, "work", None, None).unwrap();
        store.add("B", None, "personal", None, None).unwrap();
        store.add("C", None, "work", None, None).unwrap();

        let all = store.list(None, None);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_store_list_by_status() {
        let (mut store, _dir) = temp_store();
        let e1 = store.add("A", None, "general", None, None).unwrap();
        store.add("B", None, "general", None, None).unwrap();
        store.complete(&e1.id).unwrap();

        let pending = store.list(Some(&ReminderStatus::Pending), None);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "B");

        let done = store.list(Some(&ReminderStatus::Done), None);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].title, "A");
    }

    #[test]
    fn test_store_list_by_category() {
        let (mut store, _dir) = temp_store();
        store.add("A", None, "work", None, None).unwrap();
        store.add("B", None, "personal", None, None).unwrap();
        store.add("C", None, "work", None, None).unwrap();

        let work = store.list(None, Some("work"));
        assert_eq!(work.len(), 2);
        assert!(work.iter().all(|e| e.category == "work"));

        let personal = store.list(None, Some("personal"));
        assert_eq!(personal.len(), 1);
    }

    #[test]
    fn test_store_overdue() {
        let (mut store, _dir) = temp_store();
        // Past due (epoch 1000 is 1970).
        store
            .add("Old task", None, "general", Some(1000), None)
            .unwrap();
        // Future due (year ~2033).
        store
            .add("Future task", None, "general", Some(2000000000), None)
            .unwrap();
        // No due date.
        store.add("No date", None, "general", None, None).unwrap();

        let overdue = store.overdue();
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].title, "Old task");
    }

    // ---- Parse tests ----

    #[test]
    fn test_parse_iso_rfc3339_utc() {
        let epoch = parse_iso_to_epoch("2026-03-01T09:00:00Z").unwrap();
        // Should be a reasonable timestamp in 2026
        assert!(epoch > 1700000000, "epoch should be after 2023");
        assert!(epoch < 1900000000, "epoch should be before 2030");
    }

    #[test]
    fn test_parse_iso_rfc3339_offset() {
        let utc = parse_iso_to_epoch("2026-03-01T09:00:00Z").unwrap();
        let plus8 = parse_iso_to_epoch("2026-03-01T17:00:00+08:00").unwrap();
        // Both represent the same instant
        assert_eq!(utc, plus8);
    }

    #[test]
    fn test_parse_iso_date_only() {
        let epoch = parse_iso_to_epoch("2026-03-01").unwrap();
        // Should be midnight UTC on 2026-03-01
        assert!(epoch > 1700000000, "epoch should be after 2023");
        assert!(epoch < 1900000000, "epoch should be before 2030");
        // Date-only should be earlier than or equal to 09:00 UTC same day
        let nine_am = parse_iso_to_epoch("2026-03-01T09:00:00Z").unwrap();
        assert!(epoch <= nine_am);
    }

    #[test]
    fn test_parse_iso_invalid() {
        let result = parse_iso_to_epoch("not-a-date");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cannot parse"));
    }

    // ---- Tool trait tests ----

    #[test]
    fn test_tool_name() {
        let (tool, _dir) = temp_tool();
        assert_eq!(tool.name(), "reminder");
    }

    #[test]
    fn test_tool_description_not_empty() {
        let (tool, _dir) = temp_tool();
        assert!(!tool.description().is_empty());
        assert!(tool.description().contains("reminder"));
    }

    #[test]
    fn test_tool_parameters_has_action() {
        let (tool, _dir) = temp_tool();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert_eq!(params["required"], json!(["action"]));
    }

    #[tokio::test]
    async fn test_execute_add_basic() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "add",
                    "title": "Buy groceries",
                    "category": "shopping"
                }),
                &c,
            )
            .await
            .unwrap()
            .for_llm;

        assert!(result.contains("Created reminder"));
        assert!(result.contains("Buy groceries"));
        assert!(result.contains("r1"));
    }

    #[tokio::test]
    async fn test_execute_list_empty() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "list"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert_eq!(result, "No reminders found");
    }

    #[tokio::test]
    async fn test_execute_add_then_list() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "add", "title": "Task A", "category": "work"}),
            &c,
        )
        .await
        .unwrap()
        .for_llm;

        tool.execute(
            json!({"action": "add", "title": "Task B", "category": "personal"}),
            &c,
        )
        .await
        .unwrap()
        .for_llm;

        let result = tool
            .execute(json!({"action": "list"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("2 reminders"));
        assert!(result.contains("Task A"));
        assert!(result.contains("Task B"));
    }

    #[tokio::test]
    async fn test_execute_complete() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(json!({"action": "add", "title": "Finish report"}), &c)
            .await
            .unwrap()
            .for_llm;

        let result = tool
            .execute(json!({"action": "complete", "id": "r1"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("Completed reminder r1"));

        // Verify it shows as done in list.
        let list = tool
            .execute(json!({"action": "list", "status": "done"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert!(list.contains("Finish report"));
    }

    #[tokio::test]
    async fn test_execute_remove() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(json!({"action": "add", "title": "Temp task"}), &c)
            .await
            .unwrap()
            .for_llm;

        let result = tool
            .execute(json!({"action": "remove", "id": "r1"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("Removed reminder r1"));

        let list = tool
            .execute(json!({"action": "list"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert_eq!(list, "No reminders found");
    }

    #[tokio::test]
    async fn test_execute_missing_action() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'action'"));
    }

    #[tokio::test]
    async fn test_execute_unknown_action() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({"action": "invalid"}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown reminder action 'invalid'"));
    }

    #[tokio::test]
    async fn test_execute_snooze() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "add", "title": "Snoozeable", "due_at": "2026-03-01T09:00:00Z"}),
            &c,
        )
        .await
        .unwrap()
        .for_llm;
        let result = tool
            .execute(
                json!({"action": "snooze", "id": "r1", "due_at": "2026-06-01T09:00:00Z"}),
                &c,
            )
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("snoozed") || result.contains("Snoozed"));
        assert!(result.contains("r1"));
    }

    #[tokio::test]
    async fn test_execute_overdue() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        // Add a reminder with due_at in the past (epoch 1 = 1970-01-01)
        {
            let mut store = tool.store.lock().await;
            store
                .add("Overdue task", None, "work", Some(1), None)
                .unwrap();
        }
        let result = tool
            .execute(json!({"action": "overdue"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("overdue"));
        assert!(result.contains("Overdue task"));
    }

    #[tokio::test]
    async fn test_execute_overdue_empty() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "overdue"}), &c)
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("No overdue"));
    }

    #[tokio::test]
    async fn test_execute_add_with_due_at() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({"action": "add", "title": "Timed task", "due_at": "2026-12-25T10:00:00Z"}),
                &c,
            )
            .await
            .unwrap()
            .for_llm;
        assert!(result.contains("Timed task"));
        assert!(result.contains("r1"));

        // Verify the due_at was parsed and stored
        let store = tool.store.lock().await;
        let entry = store.get("r1").unwrap();
        assert!(entry.due_at.is_some());
        assert!(entry.due_at.unwrap() > 1700000000);
    }

    #[test]
    fn test_parse_iso_negative_epoch() {
        assert!(parse_iso_to_epoch("1960-01-01T00:00:00Z").is_err());
    }
}
