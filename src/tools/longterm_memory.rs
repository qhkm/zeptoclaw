//! Long-term memory tool.
//!
//! Exposes the `LongTermMemory` store to the AI agent, allowing it to remember
//! facts, preferences, and learnings that persist across sessions.

use std::sync::Arc;

use tokio::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Result, ZeptoError};
use crate::memory::longterm::LongTermMemory;

use super::{Tool, ToolContext};

/// Tool for storing and retrieving long-term memories across sessions.
pub struct LongTermMemoryTool {
    memory: Arc<Mutex<LongTermMemory>>,
}

impl LongTermMemoryTool {
    /// Create a new long-term memory tool.
    ///
    /// Initializes the underlying `LongTermMemory` store, loading any
    /// previously persisted entries from `~/.zeptoclaw/memory/longterm.json`.
    pub fn new() -> Result<Self> {
        let memory = LongTermMemory::new()?;
        Ok(Self {
            memory: Arc::new(Mutex::new(memory)),
        })
    }

    /// Create a long-term memory tool with a pre-existing memory instance.
    /// Useful for testing or shared ownership scenarios.
    pub fn with_memory(memory: Arc<Mutex<LongTermMemory>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for LongTermMemoryTool {
    fn name(&self) -> &str {
        "longterm_memory"
    }

    fn description(&self) -> &str {
        "Store and retrieve long-term memories (facts, preferences, learnings) that persist across sessions. Use 'set' to remember something, 'get' to recall by key, 'search' to find memories by keyword."
    }

    fn compact_description(&self) -> &str {
        "Long-term memory store"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["set", "get", "search", "delete", "list", "categories", "pin"],
                    "description": "Action to perform"
                },
                "key": {
                    "type": "string",
                    "description": "Memory key (e.g., 'user:name', 'preference:language')"
                },
                "value": {
                    "type": "string",
                    "description": "Memory value/content to store"
                },
                "category": {
                    "type": "string",
                    "description": "Category for grouping (e.g., 'user', 'preference', 'fact', 'learning')"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for search and organization"
                },
                "importance": {
                    "type": "number",
                    "description": "Optional importance weight (0.0-2.0+, default 1.0). Higher values decay slower over time."
                },
                "query": {
                    "type": "string",
                    "description": "Search query (searches across key, value, category, and tags)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".to_string()))?;

        match action {
            "set" => self.execute_set(&args).await,
            "get" => self.execute_get(&args).await,
            "search" => self.execute_search(&args).await,
            "delete" => self.execute_delete(&args).await,
            "list" => self.execute_list(&args).await,
            "categories" => self.execute_categories().await,
            "pin" => self.execute_pin(&args).await,
            other => Err(ZeptoError::Tool(format!(
                "Unknown longterm_memory action '{}'. Valid actions: set, get, search, delete, list, categories, pin",
                other
            ))),
        }
    }
}

impl LongTermMemoryTool {
    async fn execute_set(&self, args: &Value) -> Result<String> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'key' parameter for set action".to_string())
            })?;

        let value = args
            .get("value")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'value' parameter for set action".to_string())
            })?;

        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'category' parameter for set action".to_string())
            })?;

        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Default importance to 1.0 if not specified
        let importance = args
            .get("importance")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(1.0);

        let mut memory = self.memory.lock().await;

        // Check if this is an update or a new entry.
        let is_update = memory.get_readonly(key).is_some();
        memory.set(key, value, category, tags, importance).await?;

        if is_update {
            Ok(format!("Updated memory '{}'", key))
        } else {
            Ok(format!(
                "Stored memory '{}' in category '{}'",
                key, category
            ))
        }
    }

    async fn execute_get(&self, args: &Value) -> Result<String> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'key' parameter for get action".to_string())
            })?;

        let mut memory = self.memory.lock().await;

        match memory.get(key) {
            Some(entry) => {
                let entry_json = serde_json::to_string_pretty(entry).map_err(|e| {
                    ZeptoError::Tool(format!("Failed to serialize memory entry: {}", e))
                })?;
                Ok(entry_json)
            }
            None => Ok(format!("No memory found for key '{}'", key)),
        }
    }

    async fn execute_search(&self, args: &Value) -> Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'query' parameter for search action".to_string())
            })?;

        let memory = self.memory.lock().await;

        let results = memory.search(query);

        if results.is_empty() {
            return Ok(format!("No memories found matching '{}'", query));
        }

        let entries: Vec<&crate::memory::longterm::MemoryEntry> = results;
        let json = serde_json::to_string_pretty(&entries)
            .map_err(|e| ZeptoError::Tool(format!("Failed to serialize search results: {}", e)))?;

        Ok(format!(
            "Found {} matching memor{}:\n{}",
            entries.len(),
            if entries.len() == 1 { "y" } else { "ies" },
            json
        ))
    }

    async fn execute_delete(&self, args: &Value) -> Result<String> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'key' parameter for delete action".to_string())
            })?;

        let mut memory = self.memory.lock().await;

        if memory.delete(key).await? {
            Ok(format!("Deleted memory '{}'", key))
        } else {
            Ok(format!("No memory found for key '{}'", key))
        }
    }

    async fn execute_list(&self, args: &Value) -> Result<String> {
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let memory = self.memory.lock().await;

        let results: Vec<&crate::memory::longterm::MemoryEntry> = if let Some(cat) = category {
            memory.list_by_category(cat)
        } else {
            memory.list_all()
        };

        if results.is_empty() {
            return if let Some(cat) = category {
                Ok(format!("No memories in category '{}'", cat))
            } else {
                Ok("No memories stored yet".to_string())
            };
        }

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| ZeptoError::Tool(format!("Failed to serialize memory entries: {}", e)))?;

        let label = if let Some(cat) = category {
            format!(
                "{} memor{} in category '{}'",
                results.len(),
                if results.len() == 1 { "y" } else { "ies" },
                cat
            )
        } else {
            format!(
                "{} total memor{}",
                results.len(),
                if results.len() == 1 { "y" } else { "ies" }
            )
        };

        Ok(format!("{}:\n{}", label, json))
    }

    async fn execute_categories(&self) -> Result<String> {
        let memory = self.memory.lock().await;

        let categories = memory.categories();

        if categories.is_empty() {
            return Ok("No categories yet (memory is empty)".to_string());
        }

        let summary = memory.summary();
        let json = serde_json::to_string_pretty(&categories)
            .map_err(|e| ZeptoError::Tool(format!("Failed to serialize categories: {}", e)))?;

        Ok(format!("{}\nCategories:\n{}", summary, json))
    }

    async fn execute_pin(&self, args: &Value) -> Result<String> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'key' parameter for pin action".to_string())
            })?;

        let value = args
            .get("value")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool("Missing 'value' parameter for pin action".to_string())
            })?;

        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let mut memory = self.memory.lock().await;

        memory.set(key, value, "pinned", tags, 1.0).await?;
        Ok(format!("Pinned memory '{}'", key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a LongTermMemoryTool backed by a temp directory.
    fn temp_tool() -> (LongTermMemoryTool, TempDir) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("longterm.json");
        let memory = LongTermMemory::with_path(path).expect("failed to create memory");
        let tool = LongTermMemoryTool::with_memory(Arc::new(Mutex::new(memory)));
        (tool, dir)
    }

    fn ctx() -> ToolContext {
        ToolContext::new()
    }

    #[test]
    fn test_tool_name() {
        let (tool, _dir) = temp_tool();
        assert_eq!(tool.name(), "longterm_memory");
    }

    #[test]
    fn test_tool_description() {
        let (tool, _dir) = temp_tool();
        assert!(tool.description().contains("long-term memories"));
        assert!(tool.description().contains("persist across sessions"));
    }

    #[test]
    fn test_tool_parameters_schema() {
        let (tool, _dir) = temp_tool();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert!(params["properties"]["key"].is_object());
        assert!(params["properties"]["value"].is_object());
        assert!(params["properties"]["category"].is_object());
        assert!(params["properties"]["tags"].is_object());
        assert!(params["properties"]["query"].is_object());
        assert_eq!(params["required"], json!(["action"]));
    }

    #[tokio::test]
    async fn test_set_and_get() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "set",
                    "key": "user:name",
                    "value": "Alice",
                    "category": "user",
                    "tags": ["identity"]
                }),
                &c,
            )
            .await
            .unwrap();
        assert!(result.contains("Stored memory 'user:name'"));
        assert!(result.contains("category 'user'"));

        let result = tool
            .execute(json!({"action": "get", "key": "user:name"}), &c)
            .await
            .unwrap();
        assert!(result.contains("Alice"));
        assert!(result.contains("user:name"));
    }

    #[tokio::test]
    async fn test_set_update() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "set", "key": "k1", "value": "v1", "category": "test"}),
            &c,
        )
        .await
        .unwrap();

        let result = tool
            .execute(
                json!({"action": "set", "key": "k1", "value": "v2", "category": "test"}),
                &c,
            )
            .await
            .unwrap();
        assert!(result.contains("Updated memory 'k1'"));
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "get", "key": "nope"}), &c)
            .await
            .unwrap();
        assert!(result.contains("No memory found for key 'nope'"));
    }

    #[tokio::test]
    async fn test_search() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "set", "key": "fact:lang", "value": "Rust is fast", "category": "fact"}),
            &c,
        )
        .await
        .unwrap();

        tool.execute(
            json!({"action": "set", "key": "fact:db", "value": "PostgreSQL is reliable", "category": "fact"}),
            &c,
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "search", "query": "Rust"}), &c)
            .await
            .unwrap();
        assert!(result.contains("Found 1 matching memory"));
        assert!(result.contains("Rust is fast"));
    }

    #[tokio::test]
    async fn test_search_no_results() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "search", "query": "nonexistent"}), &c)
            .await
            .unwrap();
        assert!(result.contains("No memories found matching 'nonexistent'"));
    }

    #[tokio::test]
    async fn test_delete() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "set", "key": "k1", "value": "v1", "category": "test"}),
            &c,
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "delete", "key": "k1"}), &c)
            .await
            .unwrap();
        assert!(result.contains("Deleted memory 'k1'"));

        let result = tool
            .execute(json!({"action": "get", "key": "k1"}), &c)
            .await
            .unwrap();
        assert!(result.contains("No memory found"));
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "delete", "key": "nope"}), &c)
            .await
            .unwrap();
        assert!(result.contains("No memory found for key 'nope'"));
    }

    #[tokio::test]
    async fn test_list_all() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "set", "key": "k1", "value": "v1", "category": "a"}),
            &c,
        )
        .await
        .unwrap();

        tool.execute(
            json!({"action": "set", "key": "k2", "value": "v2", "category": "b"}),
            &c,
        )
        .await
        .unwrap();

        let result = tool.execute(json!({"action": "list"}), &c).await.unwrap();
        assert!(result.contains("2 total memories"));
        assert!(result.contains("k1"));
        assert!(result.contains("k2"));
    }

    #[tokio::test]
    async fn test_list_by_category() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "set", "key": "k1", "value": "v1", "category": "user"}),
            &c,
        )
        .await
        .unwrap();

        tool.execute(
            json!({"action": "set", "key": "k2", "value": "v2", "category": "fact"}),
            &c,
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "list", "category": "user"}), &c)
            .await
            .unwrap();
        assert!(result.contains("1 memory in category 'user'"));
        assert!(result.contains("k1"));
        assert!(!result.contains("k2"));
    }

    #[tokio::test]
    async fn test_list_empty() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({"action": "list"}), &c).await.unwrap();
        assert!(result.contains("No memories stored yet"));
    }

    #[tokio::test]
    async fn test_list_empty_category() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "list", "category": "nope"}), &c)
            .await
            .unwrap();
        assert!(result.contains("No memories in category 'nope'"));
    }

    #[tokio::test]
    async fn test_categories() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({"action": "set", "key": "k1", "value": "v1", "category": "user"}),
            &c,
        )
        .await
        .unwrap();

        tool.execute(
            json!({"action": "set", "key": "k2", "value": "v2", "category": "fact"}),
            &c,
        )
        .await
        .unwrap();

        tool.execute(
            json!({"action": "set", "key": "k3", "value": "v3", "category": "user"}),
            &c,
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "categories"}), &c)
            .await
            .unwrap();
        assert!(result.contains("fact"));
        assert!(result.contains("user"));
        assert!(result.contains("3 entries"));
        assert!(result.contains("2 categories"));
    }

    #[tokio::test]
    async fn test_categories_empty() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "categories"}), &c)
            .await
            .unwrap();
        assert!(result.contains("No categories yet"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({"action": "invalid"}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown longterm_memory action 'invalid'"));
    }

    #[tokio::test]
    async fn test_missing_action() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'action' parameter"));
    }

    #[tokio::test]
    async fn test_set_missing_key() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({"action": "set", "value": "v1", "category": "test"}),
                &c,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'key'"));
    }

    #[tokio::test]
    async fn test_set_missing_value() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({"action": "set", "key": "k1", "category": "test"}),
                &c,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'value'"));
    }

    #[tokio::test]
    async fn test_set_missing_category() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(json!({"action": "set", "key": "k1", "value": "v1"}), &c)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'category'"));
    }

    #[tokio::test]
    async fn test_get_missing_key() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({"action": "get"}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'key'"));
    }

    #[tokio::test]
    async fn test_search_missing_query() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({"action": "search"}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'query'"));
    }

    #[tokio::test]
    async fn test_delete_missing_key() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool.execute(json!({"action": "delete"}), &c).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'key'"));
    }

    #[tokio::test]
    async fn test_set_with_tags() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        tool.execute(
            json!({
                "action": "set",
                "key": "pref:theme",
                "value": "dark mode",
                "category": "preference",
                "tags": ["ui", "visual", "display"]
            }),
            &c,
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "search", "query": "visual"}), &c)
            .await
            .unwrap();
        assert!(result.contains("dark mode"));
    }

    #[tokio::test]
    async fn test_set_without_tags() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "set",
                    "key": "fact:color",
                    "value": "blue",
                    "category": "fact"
                }),
                &c,
            )
            .await
            .unwrap();
        assert!(result.contains("Stored memory 'fact:color'"));

        let result = tool
            .execute(json!({"action": "get", "key": "fact:color"}), &c)
            .await
            .unwrap();
        assert!(result.contains("blue"));
    }

    #[tokio::test]
    async fn test_pin_action() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "pin",
                    "key": "important:fact",
                    "value": "This should never be forgotten",
                    "tags": ["critical", "permanent"]
                }),
                &c,
            )
            .await
            .unwrap();
        assert!(result.contains("Pinned memory 'important:fact'"));

        // Verify it's in "pinned" category
        let result = tool
            .execute(json!({"action": "get", "key": "important:fact"}), &c)
            .await
            .unwrap();
        assert!(result.contains("This should never be forgotten"));
        assert!(result.contains("pinned"));
    }

    #[tokio::test]
    async fn test_pin_missing_key() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "pin",
                    "value": "some value"
                }),
                &c,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'key'"));
    }

    #[tokio::test]
    async fn test_pin_missing_value() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "pin",
                    "key": "some:key"
                }),
                &c,
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'value'"));
    }

    #[tokio::test]
    async fn test_set_with_importance() {
        let (tool, _dir) = temp_tool();
        let c = ctx();

        let result = tool
            .execute(
                json!({
                    "action": "set",
                    "key": "critical:data",
                    "value": "Very important information",
                    "category": "critical",
                    "importance": 2.0
                }),
                &c,
            )
            .await
            .unwrap();
        assert!(result.contains("Stored memory 'critical:data'"));

        // Verify it was stored
        let result = tool
            .execute(json!({"action": "get", "key": "critical:data"}), &c)
            .await
            .unwrap();
        assert!(result.contains("Very important information"));
        assert!(result.contains("critical"));
    }
}
