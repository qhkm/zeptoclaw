//! Find tool — search for files by glob pattern.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Result, ZeptoError};
use crate::security::validate_path_in_workspace;

use super::output::{truncate_tool_output, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Tool for finding files by glob pattern.
///
/// Uses the `glob` crate for pattern matching. Supports recursive
/// patterns like `**/*.rs` and result limiting.
pub struct FindTool;

#[async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Search for files and directories by name pattern. Uses glob matching (e.g. '**/*.rs')."
    }

    fn compact_description(&self) -> &str {
        "Find files"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FilesystemRead
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. '**/*.rs', 'src/**/test_*')"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory to search from (default: workspace root)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 200)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'pattern' argument".into()))?;

        let workspace = ctx.workspace.as_ref().ok_or_else(|| {
            ZeptoError::SecurityViolation(
                "Workspace not configured; find requires a workspace".to_string(),
            )
        })?;

        let root = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => {
                let safe = validate_path_in_workspace(p, workspace)?;
                safe.as_path().to_string_lossy().to_string()
            }
            None => workspace.clone(),
        };

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize;

        let full_pattern = format!("{}/{}", root, pattern);

        let entries: Vec<String> = glob::glob(&full_pattern)
            .map_err(|e| ZeptoError::Tool(format!("Invalid glob pattern: {}", e)))?
            .filter_map(|r| r.ok())
            .take(limit)
            .map(|p| p.display().to_string())
            .collect();

        if entries.is_empty() {
            return Ok(ToolOutput::llm_only(
                "No files found matching pattern".to_string(),
            ));
        }

        let count = entries.len();
        let output = format!("{}\n({} files)", entries.join("\n"), count);
        Ok(ToolOutput::llm_only(truncate_tool_output(
            &output,
            DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_tool_name() {
        assert_eq!(FindTool.name(), "find");
    }

    #[test]
    fn test_find_tool_category() {
        assert!(matches!(FindTool.category(), ToolCategory::FilesystemRead));
    }

    #[test]
    fn test_find_parameters_schema() {
        let params = FindTool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["pattern"].is_object());
        assert!(params["properties"]["path"].is_object());
        assert!(params["properties"]["limit"].is_object());
        assert_eq!(params["required"], json!(["pattern"]));
    }

    #[tokio::test]
    async fn test_find_requires_pattern() {
        let ctx = ToolContext::new().with_workspace("/tmp");
        let result = FindTool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pattern"));
    }

    #[tokio::test]
    async fn test_find_requires_workspace() {
        let ctx = ToolContext::new();
        let result = FindTool.execute(json!({"pattern": "*.rs"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Workspace"));
    }

    #[tokio::test]
    async fn test_find_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = FindTool
            .execute(json!({"pattern": "*.nonexistent"}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("No files found"));
    }

    #[tokio::test]
    async fn test_find_matches_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "// lib").unwrap();
        std::fs::write(dir.path().join("readme.md"), "# readme").unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = FindTool
            .execute(json!({"pattern": "*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("main.rs"));
        assert!(result.for_llm.contains("lib.rs"));
        assert!(!result.for_llm.contains("readme.md"));
        assert!(result.for_llm.contains("2 files"));
    }

    #[tokio::test]
    async fn test_find_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("file_{}.txt", i)), "content").unwrap();
        }
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = FindTool
            .execute(json!({"pattern": "*.txt", "limit": 3}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("3 files"));
    }

    #[tokio::test]
    async fn test_find_invalid_glob() {
        let ctx = ToolContext::new().with_workspace("/tmp");
        let result = FindTool.execute(json!({"pattern": "[invalid"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid glob"));
    }
}
