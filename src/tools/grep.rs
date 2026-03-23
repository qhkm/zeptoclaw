//! Grep tool — search file contents by regex pattern.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Result, ZeptoError};
use crate::security::validate_path_in_workspace;

use super::output::{truncate_tool_output, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Tool for searching file contents by pattern.
///
/// Shells out to system `grep -rn` for performance. Supports regex patterns,
/// glob file filters, case-insensitive search, and result limiting.
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files. Supports regex patterns and glob file filters."
    }

    fn compact_description(&self) -> &str {
        "Search files"
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
                    "description": "Regex or literal pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: workspace root)"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. '*.rs', '*.py')"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "Case-insensitive search (default: false)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum matches to return (default: 100)"
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
                "Workspace not configured; grep requires a workspace".to_string(),
            )
        })?;

        let search_path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => {
                let safe = validate_path_in_workspace(p, workspace)?;
                safe.as_path().to_string_lossy().to_string()
            }
            None => workspace.clone(),
        };

        let ignore_case = args
            .get("ignore_case")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        let glob_pattern = args.get("glob").and_then(|v| v.as_str());

        // Validate regex before spawning grep
        if let Err(e) = regex::Regex::new(pattern) {
            return Err(ZeptoError::Tool(format!("Invalid regex pattern: {}", e)));
        }

        let mut cmd_args = vec!["-rn".to_string()];
        if ignore_case {
            cmd_args.push("-i".to_string());
        }
        if let Some(glob_pat) = glob_pattern {
            cmd_args.push("--include".to_string());
            cmd_args.push(glob_pat.to_string());
        }
        cmd_args.push("--".to_string());
        cmd_args.push(pattern.to_string());
        cmd_args.push(search_path.clone());

        let output = tokio::process::Command::new("grep")
            .args(&cmd_args)
            .output()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to run grep: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        match output.status.code() {
            Some(0) => {}
            Some(1) if stderr.trim().is_empty() => {
                return Ok(ToolOutput::llm_only("No matches found".to_string()));
            }
            Some(code) => {
                let detail = stderr.trim();
                let suffix = if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail)
                };
                return Err(ZeptoError::Tool(format!(
                    "grep failed with exit code {}{}",
                    code, suffix
                )));
            }
            None => {
                let detail = stderr.trim();
                let suffix = if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail)
                };
                return Err(ZeptoError::Tool(format!(
                    "grep terminated by signal{}",
                    suffix
                )));
            }
        }

        let lines: Vec<&str> = stdout.lines().take(limit).collect();

        let total = stdout.lines().count();
        let mut result = lines.join("\n");
        if total > limit {
            result.push_str(&format!(
                "\n... ({} more matches, capped at {})",
                total - limit,
                limit
            ));
        }

        Ok(ToolOutput::llm_only(truncate_tool_output(
            &result,
            DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grep_tool_name() {
        assert_eq!(GrepTool.name(), "grep");
    }

    #[test]
    fn test_grep_tool_category() {
        assert!(matches!(GrepTool.category(), ToolCategory::FilesystemRead));
    }

    #[test]
    fn test_grep_parameters_schema() {
        let params = GrepTool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["pattern"].is_object());
        assert!(params["properties"]["path"].is_object());
        assert!(params["properties"]["glob"].is_object());
        assert!(params["properties"]["ignore_case"].is_object());
        assert!(params["properties"]["limit"].is_object());
        assert_eq!(params["required"], json!(["pattern"]));
    }

    #[tokio::test]
    async fn test_grep_requires_pattern() {
        let ctx = ToolContext::new().with_workspace("/tmp");
        let result = GrepTool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pattern"));
    }

    #[tokio::test]
    async fn test_grep_requires_workspace() {
        let ctx = ToolContext::new();
        let result = GrepTool.execute(json!({"pattern": "test"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Workspace"));
    }

    #[tokio::test]
    async fn test_grep_invalid_regex() {
        let ctx = ToolContext::new().with_workspace("/tmp");
        let result = GrepTool.execute(json!({"pattern": "[invalid"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = GrepTool
            .execute(json!({"pattern": "zzz_nonexistent_zzz"}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("No matches"));
    }

    #[tokio::test]
    async fn test_grep_finds_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = GrepTool
            .execute(json!({"pattern": "hello"}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("hello"));
    }

    #[tokio::test]
    async fn test_grep_reports_subprocess_errors() {
        let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
        let ctx = ToolContext::new().with_workspace(workspace.to_str().unwrap());
        let result = GrepTool
            .execute(
                json!({"pattern": "hello", "path": "Cargo.toml.missing"}),
                &ctx,
            )
            .await;

        match result {
            Err(ZeptoError::Tool(err)) => assert!(err.contains("grep failed")),
            other => panic!("expected grep tool error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_grep_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "Hello World").unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = GrepTool
            .execute(json!({"pattern": "hello", "ignore_case": true}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("Hello"));
    }

    #[tokio::test]
    async fn test_grep_with_glob_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("test.txt"), "fn main() {}").unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = GrepTool
            .execute(json!({"pattern": "fn main", "glob": "*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(result.for_llm.contains("test.rs"));
        assert!(!result.for_llm.contains("test.txt"));
    }

    #[tokio::test]
    async fn test_grep_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (0..20).map(|i| format!("match line {}\n", i)).collect();
        std::fs::write(dir.path().join("test.txt"), &content).unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = GrepTool
            .execute(json!({"pattern": "match", "limit": 5}), &ctx)
            .await
            .unwrap();
        let lines: Vec<&str> = result.for_llm.lines().collect();
        // 5 match lines + 1 "more matches" line
        assert!(
            lines.len() <= 6,
            "Expected at most 6 lines, got {}",
            lines.len()
        );
        assert!(result.for_llm.contains("more matches"));
    }
}
