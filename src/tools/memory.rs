//! Workspace memory tools.
//!
//! Provides:
//! - `memory_search`: search memory markdown files in the workspace.
//! - `memory_get`: read a memory file with optional line window.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::{MemoryCitationsMode, MemoryConfig};
use crate::error::{Result, ZeptoError};
use crate::memory::builtin_searcher::BuiltinSearcher;
use crate::memory::traits::MemorySearcher;
use crate::memory::{read_workspace_memory, search_workspace_memory};

use super::{Tool, ToolContext};

/// Tool for searching workspace memory files.
pub struct MemorySearchTool {
    config: MemoryConfig,
    searcher: Arc<dyn MemorySearcher>,
}

impl MemorySearchTool {
    /// Create a new memory search tool.
    pub fn new(config: MemoryConfig) -> Self {
        Self {
            config,
            searcher: Arc::new(BuiltinSearcher),
        }
    }

    /// Create a new memory search tool with a custom searcher.
    pub fn with_searcher(config: MemoryConfig, searcher: Arc<dyn MemorySearcher>) -> Self {
        Self { config, searcher }
    }
}

/// Tool for reading workspace memory files.
pub struct MemoryGetTool {
    config: MemoryConfig,
}

impl MemoryGetTool {
    /// Create a new memory get tool.
    pub fn new(config: MemoryConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search workspace memory markdown files and return relevant snippets."
    }

    fn compact_description(&self) -> &str {
        "Search memory"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Query text to search in memory"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (1-50)"
                },
                "min_score": {
                    "type": "number",
                    "description": "Minimum score threshold (0.0-1.0)"
                },
                "include_citations": {
                    "type": "boolean",
                    "description": "Override citation behavior for this call"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'query' parameter".to_string()))?;

        let workspace = ctx.workspace.as_deref().ok_or_else(|| {
            ZeptoError::Tool("Memory tools require a workspace context".to_string())
        })?;

        let max_results = args
            .get("max_results")
            .and_then(Value::as_u64)
            .map(|v| v as usize);
        let min_score = args
            .get("min_score")
            .and_then(Value::as_f64)
            .map(|v| v as f32);

        let include_citations = resolve_citations(&args, ctx, &self.config.citations);

        let results = search_workspace_memory(
            Path::new(workspace),
            query,
            &self.config,
            self.searcher.clone(),
            max_results,
            min_score,
            include_citations,
        )
        .await?;

        if results.is_empty() {
            return Ok(format!("No memory entries found for '{}'.", query));
        }

        let mut output = format!(
            "Found {} memory result(s) for '{}':\n\n",
            results.len(),
            query
        );
        for (index, item) in results.iter().enumerate() {
            output.push_str(&format!(
                "{}. {} (score {:.3}, lines {}-{})\n{}\n\n",
                index + 1,
                item.path,
                item.score,
                item.start_line,
                item.end_line,
                item.snippet.trim()
            ));
        }

        Ok(output.trim_end().to_string())
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn name(&self) -> &str {
        "memory_get"
    }

    fn description(&self) -> &str {
        "Read a memory markdown file from workspace memory paths."
    }

    fn compact_description(&self) -> &str {
        "Read memory"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative memory file path"
                },
                "from": {
                    "type": "integer",
                    "description": "Starting line (1-based)"
                },
                "lines": {
                    "type": "integer",
                    "description": "Number of lines to read (max 400)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'path' parameter".to_string()))?;

        let workspace = ctx.workspace.as_deref().ok_or_else(|| {
            ZeptoError::Tool("Memory tools require a workspace context".to_string())
        })?;

        let from = args.get("from").and_then(Value::as_u64).map(|v| v as usize);
        let lines = args
            .get("lines")
            .and_then(Value::as_u64)
            .map(|v| v as usize);

        let result =
            read_workspace_memory(Path::new(workspace), path, from, lines, &self.config).await?;

        let mut output = format!(
            "Memory file: {}\nLines: {}-{} of {}\nTruncated: {}",
            result.path, result.start_line, result.end_line, result.total_lines, result.truncated
        );

        if !result.text.is_empty() {
            output.push_str("\n\n");
            output.push_str(&result.text);
        }

        Ok(output)
    }
}

fn resolve_citations(args: &Value, ctx: &ToolContext, mode: &MemoryCitationsMode) -> bool {
    if let Some(explicit) = args.get("include_citations").and_then(Value::as_bool) {
        return explicit;
    }

    match mode {
        MemoryCitationsMode::On => true,
        MemoryCitationsMode::Off => false,
        MemoryCitationsMode::Auto => {
            let channel = ctx.channel.as_deref().unwrap_or("cli");
            matches!(channel, "cli")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn test_memory_search_tool_properties() {
        let tool = MemorySearchTool::new(MemoryConfig::default());
        assert_eq!(tool.name(), "memory_search");
        assert!(tool.description().contains("memory"));
    }

    #[test]
    fn test_memory_get_tool_properties() {
        let tool = MemoryGetTool::new(MemoryConfig::default());
        assert_eq!(tool.name(), "memory_get");
        assert!(tool.description().contains("memory"));
    }

    #[tokio::test]
    async fn test_memory_search_tool_executes() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("MEMORY.md"),
            "User preference: concise answers\nProject: ZeptoClaw\n",
        )
        .unwrap();

        let tool = MemorySearchTool::new(MemoryConfig::default());
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = tool
            .execute(json!({"query": "concise preference"}), &ctx)
            .await
            .unwrap();

        assert!(result.contains("MEMORY.md"));
        assert!(result.contains("concise"));
    }

    #[tokio::test]
    async fn test_memory_get_tool_executes() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("memory")).unwrap();
        fs::write(
            dir.path().join("memory/notes.md"),
            "line1\nline2\nline3\nline4\n",
        )
        .unwrap();

        let tool = MemoryGetTool::new(MemoryConfig::default());
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = tool
            .execute(
                json!({"path": "memory/notes.md", "from": 2, "lines": 2}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.contains("line2\nline3"));
        assert!(result.contains("Lines: 2-3"));
    }

    #[tokio::test]
    async fn test_memory_search_requires_query() {
        let tool = MemorySearchTool::new(MemoryConfig::default());
        let ctx = ToolContext::new().with_workspace("/tmp");
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
    }
}
