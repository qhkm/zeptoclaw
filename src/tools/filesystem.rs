//! Filesystem tools for ZeptoClaw
//!
//! This module provides tools for file system operations including reading,
//! writing, listing directories, and editing files. All paths can be either
//! absolute or relative to the workspace in the tool context.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

use crate::error::{Result, ZeptoError};
use crate::security::{check_hardlink_write, revalidate_path, validate_path_in_workspace};
use crate::tools::diff::apply_unified_diff;

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Resolve and validate a path relative to the workspace.
///
/// Requires a workspace to be configured. All paths are validated to stay
/// within workspace boundaries. This is the correct security posture --
/// filesystem tools must not operate outside a defined workspace.
///
/// Returns `(resolved_path, workspace)` so callers can re-validate before I/O.
fn resolve_path(path: &str, ctx: &ToolContext) -> Result<(String, String)> {
    let workspace = ctx.workspace.as_ref().ok_or_else(|| {
        ZeptoError::SecurityViolation(
            "Workspace not configured; filesystem tools require a workspace for safety".to_string(),
        )
    })?;
    let safe_path = validate_path_in_workspace(path, workspace)?;
    Ok((
        safe_path.as_path().to_string_lossy().to_string(),
        workspace.clone(),
    ))
}

/// Tool for reading file contents.
///
/// Reads the entire contents of a file and returns it as a string.
///
/// # Parameters
/// - `path`: The path to the file to read (required)
///
/// # Example
/// ```rust
/// use zeptoclaw::tools::{Tool, ToolContext};
/// use zeptoclaw::tools::filesystem::ReadFileTool;
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let tool = ReadFileTool;
/// let ctx = ToolContext::new();
/// // Assuming /tmp/test.txt exists with content "hello"
/// // let result = tool.execute(json!({"path": "/tmp/test.txt"}), &ctx).await;
/// # });
/// ```
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the specified path"
    }

    fn compact_description(&self) -> &str {
        "Read file"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FilesystemRead
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'path' argument".into()))?;

        let (full_path, workspace) = resolve_path(path, ctx)?;

        // TOCTOU: re-validate immediately before I/O
        revalidate_path(Path::new(&full_path), &workspace)?;

        let content = tokio::fs::read_to_string(&full_path)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to read file '{}': {}", full_path, e)))?;
        Ok(ToolOutput::llm_only(content))
    }
}

/// Tool for writing content to a file.
///
/// Writes the provided content to a file, creating it if it doesn't exist
/// or overwriting it if it does.
///
/// # Parameters
/// - `path`: The path to the file to write (required)
/// - `content`: The content to write to the file (required)
///
/// # Example
/// ```rust
/// use zeptoclaw::tools::{Tool, ToolContext};
/// use zeptoclaw::tools::filesystem::WriteFileTool;
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let tool = WriteFileTool;
/// let ctx = ToolContext::new();
/// // let result = tool.execute(json!({"path": "/tmp/test.txt", "content": "hello"}), &ctx).await;
/// # });
/// ```
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file at the specified path, creating it if necessary"
    }

    fn compact_description(&self) -> &str {
        "Write file"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FilesystemWrite
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'path' argument".into()))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'content' argument".into()))?;

        let (full_path, workspace) = resolve_path(path, ctx)?;
        let full_path_ref = Path::new(&full_path);

        // TOCTOU: re-validate BEFORE any filesystem mutation (including mkdir)
        revalidate_path(full_path_ref, &workspace)?;

        // Create parent directories if they don't exist
        if let Some(parent) = full_path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ZeptoError::Tool(format!("Failed to create parent directories: {}", e))
                })?;
            }
        }

        // Hardlink check: block writes to files with multiple hard links
        check_hardlink_write(full_path_ref)?;

        tokio::fs::write(&full_path, content).await.map_err(|e| {
            ZeptoError::Tool(format!("Failed to write file '{}': {}", full_path, e))
        })?;

        Ok(ToolOutput::llm_only(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            full_path
        )))
    }
}

/// Tool for listing directory contents.
///
/// Lists all files and directories in the specified path.
///
/// # Parameters
/// - `path`: The path to the directory to list (required)
///
/// # Example
/// ```rust
/// use zeptoclaw::tools::{Tool, ToolContext};
/// use zeptoclaw::tools::filesystem::ListDirTool;
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let tool = ListDirTool;
/// let ctx = ToolContext::new();
/// // let result = tool.execute(json!({"path": "/tmp"}), &ctx).await;
/// # });
/// ```
pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the contents of a directory at the specified path"
    }

    fn compact_description(&self) -> &str {
        "List directory"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FilesystemRead
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the directory to list"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'path' argument".into()))?;

        let (full_path, workspace) = resolve_path(path, ctx)?;

        // TOCTOU: re-validate immediately before I/O
        revalidate_path(Path::new(&full_path), &workspace)?;

        let mut entries = tokio::fs::read_dir(&full_path).await.map_err(|e| {
            ZeptoError::Tool(format!("Failed to read directory '{}': {}", full_path, e))
        })?;

        let mut items = Vec::new();

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to read directory entry: {}", e)))?
        {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().await.ok();

            let type_indicator = match file_type {
                Some(ft) if ft.is_dir() => "/",
                Some(ft) if ft.is_symlink() => "@",
                _ => "",
            };

            items.push(format!("{}{}", file_name, type_indicator));
        }

        items.sort();
        Ok(ToolOutput::llm_only(items.join("\n")))
    }
}

/// Tool for editing a file by replacing content.
///
/// Searches for a specific string in the file and replaces it with new content.
/// This is useful for making targeted edits without rewriting the entire file.
///
/// # Parameters
/// - `path`: The path to the file to edit (required)
/// - `old_text`: The text to search for and replace (required)
/// - `new_text`: The text to replace it with (required)
///
/// # Example
/// ```rust
/// use zeptoclaw::tools::{Tool, ToolContext};
/// use zeptoclaw::tools::filesystem::EditFileTool;
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let tool = EditFileTool;
/// let ctx = ToolContext::new();
/// // let result = tool.execute(json!({
/// //     "path": "/tmp/test.txt",
/// //     "old_text": "hello",
/// //     "new_text": "world"
/// // }), &ctx).await;
/// # });
/// ```
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file using either exact string replacement (old_text/new_text) or a unified diff patch (diff)."
    }

    fn compact_description(&self) -> &str {
        "Edit file"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FilesystemWrite
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "The text to search for and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The text to replace it with"
                },
                "diff": {
                    "type": "string",
                    "description": "A unified diff patch to apply. Use standard @@ hunk headers with +/- lines. Mutually exclusive with old_text/new_text."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'path' argument".into()))?;

        let diff_param = args.get("diff").and_then(|v| v.as_str());
        let old_text = args.get("old_text").and_then(|v| v.as_str());
        let new_text = args.get("new_text").and_then(|v| v.as_str());

        if diff_param.is_some() && (old_text.is_some() || new_text.is_some()) {
            return Err(ZeptoError::Tool(
                "Provide either 'diff' or 'old_text'/'new_text', not both.".into(),
            ));
        }

        if diff_param.is_none() && (old_text.is_none() || new_text.is_none()) {
            return Err(ZeptoError::Tool(
                "Provide either 'diff' or 'old_text'/'new_text'".into(),
            ));
        }

        let (full_path, workspace) = resolve_path(path, ctx)?;
        let full_path_ref = Path::new(&full_path);

        if let Some(diff_str) = diff_param {
            // --- Unified diff mode ---
            revalidate_path(full_path_ref, &workspace)?;

            let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| {
                ZeptoError::Tool(format!("Failed to read file '{}': {}", full_path, e))
            })?;

            let (new_content, summary) = apply_unified_diff(&content, diff_str)
                .map_err(|e| ZeptoError::Tool(format!("Diff apply failed: {}", e)))?;

            revalidate_path(full_path_ref, &workspace)?;
            check_hardlink_write(full_path_ref)?;

            tokio::fs::write(&full_path, &new_content)
                .await
                .map_err(|e| {
                    ZeptoError::Tool(format!("Failed to write file '{}': {}", full_path, e))
                })?;

            Ok(ToolOutput::llm_only(format!(
                "Applied {} hunk(s): +{} -{} in {}",
                summary.hunks_applied, summary.lines_added, summary.lines_removed, full_path
            )))
        } else if let (Some(old_text), Some(new_text)) = (old_text, new_text) {
            // --- String replacement mode (existing logic) ---
            revalidate_path(full_path_ref, &workspace)?;

            let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| {
                ZeptoError::Tool(format!("Failed to read file '{}': {}", full_path, e))
            })?;

            if !content.contains(old_text) {
                return Err(ZeptoError::Tool(format!(
                    "Text '{}' not found in file '{}'",
                    crate::utils::string::preview(old_text, 50),
                    full_path
                )));
            }

            let new_content = content.replace(old_text, new_text);

            revalidate_path(full_path_ref, &workspace)?;
            check_hardlink_write(full_path_ref)?;

            tokio::fs::write(&full_path, &new_content)
                .await
                .map_err(|e| {
                    ZeptoError::Tool(format!("Failed to write file '{}': {}", full_path, e))
                })?;

            let replacements = content.matches(old_text).count();
            Ok(ToolOutput::llm_only(format!(
                "Successfully replaced {} occurrence(s) in {}",
                replacements, full_path
            )))
        } else {
            // unreachable due to early validation, but kept for safety
            Err(ZeptoError::Tool(
                "Provide either 'diff' or 'old_text'/'new_text'".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_read_file_tool() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("zeptoclaw_test_read.txt");
        fs::write(&file_path, "test content").unwrap();

        let tool = ReadFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(json!({"path": "zeptoclaw_test_read.txt"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "test content");
    }

    #[tokio::test]
    async fn test_read_file_tool_not_found() {
        let dir = tempdir().unwrap();
        // Use canonical path to avoid macOS /var -> /private/var mismatch
        let canonical = dir.path().canonicalize().unwrap();
        let tool = ReadFileTool;
        let ctx = ToolContext::new().with_workspace(canonical.to_str().unwrap());

        let result = tool
            .execute(json!({"path": "nonexistent_file.txt"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to read file"));
    }

    #[tokio::test]
    async fn test_read_file_tool_missing_path() {
        let tool = ReadFileTool;
        let ctx = ToolContext::new().with_workspace("/tmp");

        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'path'"));
    }

    #[tokio::test]
    async fn test_read_file_tool_rejects_no_workspace() {
        let tool = ReadFileTool;
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"path": "/tmp/some_file.txt"}), &ctx)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Workspace not configured"));
    }

    #[tokio::test]
    async fn test_read_file_tool_with_workspace() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "workspace content").unwrap();

        let tool = ReadFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"path": "test.txt"}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "workspace content");
    }

    #[tokio::test]
    async fn test_write_file_tool() {
        let dir = tempdir().unwrap();
        // Use canonical path to avoid macOS /var -> /private/var mismatch
        let canonical = dir.path().canonicalize().unwrap();

        let tool = WriteFileTool;
        let ctx = ToolContext::new().with_workspace(canonical.to_str().unwrap());

        let result = tool
            .execute(
                json!({"path": "write_test.txt", "content": "written content"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("Successfully wrote"));

        // Verify
        assert_eq!(
            fs::read_to_string(canonical.join("write_test.txt")).unwrap(),
            "written content"
        );
    }

    #[tokio::test]
    async fn test_write_file_tool_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        // Use canonical path to avoid macOS /var -> /private/var mismatch
        let canonical = dir.path().canonicalize().unwrap();

        let tool = WriteFileTool;
        let ctx = ToolContext::new().with_workspace(canonical.to_str().unwrap());

        let result = tool
            .execute(json!({"path": "a/b/c/test.txt", "content": "nested"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert_eq!(
            fs::read_to_string(canonical.join("a/b/c/test.txt")).unwrap(),
            "nested"
        );
    }

    #[tokio::test]
    async fn test_write_file_tool_missing_content() {
        let tool = WriteFileTool;
        let ctx = ToolContext::new().with_workspace("/tmp");

        let result = tool.execute(json!({"path": "test.txt"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'content'"));
    }

    #[tokio::test]
    async fn test_list_dir_tool() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("file1.txt"), "").unwrap();
        fs::write(dir.path().join("file2.txt"), "").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let tool = ListDirTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"path": "."}), &ctx).await;
        assert!(result.is_ok());

        let output = result.unwrap().for_llm;
        assert!(output.contains("file1.txt"));
        assert!(output.contains("file2.txt"));
        assert!(output.contains("subdir/"));
    }

    #[tokio::test]
    async fn test_list_dir_tool_not_found() {
        let dir = tempdir().unwrap();
        // Use canonical path to avoid macOS /var -> /private/var mismatch
        let canonical = dir.path().canonicalize().unwrap();

        let tool = ListDirTool;
        let ctx = ToolContext::new().with_workspace(canonical.to_str().unwrap());

        let result = tool.execute(json!({"path": "nonexistent_dir"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to read directory"));
    }

    #[tokio::test]
    async fn test_list_dir_tool_with_workspace() {
        let dir = tempdir().unwrap();
        let subdir = dir.path().join("mydir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("inner.txt"), "").unwrap();

        let tool = ListDirTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"path": "mydir"}), &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("inner.txt"));
    }

    #[tokio::test]
    async fn test_edit_file_tool() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("edit_test.txt");
        fs::write(&file_path, "Hello World").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "edit_test.txt",
                    "old_text": "World",
                    "new_text": "Rust"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        assert!(result
            .unwrap()
            .for_llm
            .contains("Successfully replaced 1 occurrence"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "Hello Rust");
    }

    #[tokio::test]
    async fn test_edit_file_tool_multiple_occurrences() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("edit_multi.txt");
        fs::write(&file_path, "foo bar foo baz foo").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "edit_multi.txt",
                    "old_text": "foo",
                    "new_text": "qux"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("3 occurrence"));
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "qux bar qux baz qux"
        );
    }

    #[tokio::test]
    async fn test_edit_file_tool_text_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("edit_notfound.txt");
        fs::write(&file_path, "Hello World").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "edit_notfound.txt",
                    "old_text": "NotPresent",
                    "new_text": "Replacement"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not found in file"));
    }

    #[tokio::test]
    async fn test_edit_file_tool_missing_args() {
        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace("/tmp");

        // Missing old_text (only new_text provided)
        let result = tool
            .execute(json!({"path": "test.txt", "new_text": "new"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Provide either 'diff' or 'old_text'/'new_text'"));

        // Missing new_text (only old_text provided)
        let result = tool
            .execute(json!({"path": "test.txt", "old_text": "old"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Provide either 'diff' or 'old_text'/'new_text'"));
    }

    #[test]
    fn test_resolve_path_rejects_without_workspace() {
        let ctx = ToolContext::new();
        let result = resolve_path("relative/path", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Workspace not configured"));
    }

    #[test]
    fn test_resolve_path_relative_with_workspace() {
        let dir = tempdir().unwrap();
        // Create the relative path structure
        std::fs::create_dir_all(dir.path().join("relative")).unwrap();
        std::fs::write(dir.path().join("relative/path"), "").unwrap();

        let workspace = dir.path().to_str().unwrap();
        let ctx = ToolContext::new().with_workspace(workspace);
        let result = resolve_path("relative/path", &ctx);
        assert!(result.is_ok());
        let (resolved, _ws) = result.unwrap();
        // The path should contain "relative/path" and be within workspace
        assert!(resolved.contains("relative/path") || resolved.ends_with("relative/path"));
    }

    #[test]
    fn test_resolve_path_blocks_absolute_outside_workspace() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
        let result = resolve_path("/etc/passwd", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_names() {
        assert_eq!(ReadFileTool.name(), "read_file");
        assert_eq!(WriteFileTool.name(), "write_file");
        assert_eq!(ListDirTool.name(), "list_dir");
        assert_eq!(EditFileTool.name(), "edit_file");
    }

    #[test]
    fn test_tool_descriptions() {
        assert!(!ReadFileTool.description().is_empty());
        assert!(!WriteFileTool.description().is_empty());
        assert!(!ListDirTool.description().is_empty());
        assert!(!EditFileTool.description().is_empty());
    }

    #[test]
    fn test_tool_parameters() {
        for tool in [
            &ReadFileTool as &dyn Tool,
            &WriteFileTool,
            &ListDirTool,
            &EditFileTool,
        ] {
            let params = tool.parameters();
            assert!(params.is_object());
            assert_eq!(params["type"], "object");
            assert!(params["properties"].is_object());
            assert!(params["required"].is_array());
        }
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = tempdir().unwrap();

        let tool = ReadFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        // Attempt path traversal
        let result = tool
            .execute(json!({"path": "../../../etc/passwd"}), &ctx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Security violation") || err.contains("escapes workspace"));
    }

    #[tokio::test]
    async fn test_absolute_path_outside_workspace_blocked() {
        let dir = tempdir().unwrap();

        let tool = ReadFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"path": "/etc/passwd"}), &ctx).await;

        assert!(result.is_err());
    }

    // ==================== ADDITIONAL SECURITY/ERROR PATH TESTS ====================

    #[tokio::test]
    async fn test_write_tool_rejects_traversal_outside_workspace() {
        let dir = tempdir().unwrap();
        let tool = WriteFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({"path": "../../etc/shadow", "content": "pwned"}),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Security violation") || err.contains("traversal"),
            "Expected security error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_list_dir_rejects_absolute_outside_workspace() {
        let dir = tempdir().unwrap();
        let tool = ListDirTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"path": "/etc"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Security violation") || err.contains("escapes workspace"),
            "Expected security error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_edit_tool_rejects_no_workspace() {
        let tool = EditFileTool;
        let ctx = ToolContext::new(); // No workspace configured

        let result = tool
            .execute(
                json!({
                    "path": "/tmp/test.txt",
                    "old_text": "a",
                    "new_text": "b"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Workspace not configured"),
            "Expected workspace error, got: {}",
            err
        );
    }

    #[test]
    fn test_resolve_path_blocks_url_encoded_traversal() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        // URL-encoded ".." should be caught by the traversal pattern checker
        let result = resolve_path("%2e%2e/etc/passwd", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Security violation") || err.contains("traversal"),
            "Expected security error for URL-encoded traversal, got: {}",
            err
        );
    }

    #[test]
    fn test_resolve_path_blocks_double_encoded_traversal() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        // Double URL-encoded ".." (%252e%252e) should be caught
        let result = resolve_path("%252e%252e/etc/passwd", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Security violation") || err.contains("traversal"),
            "Expected security error for double-encoded traversal, got: {}",
            err
        );
    }

    // ==================== TOCTOU + HARDLINK SECURITY TESTS ====================

    #[tokio::test]
    async fn test_write_blocks_hardlinked_file() {
        let dir = tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a regular file
        let original = canonical.join("original.txt");
        fs::write(&original, "original content").unwrap();

        // Create a hard link to it
        let hardlink = canonical.join("hardlink.txt");
        fs::hard_link(&original, &hardlink).unwrap();

        let tool = WriteFileTool;
        let ctx = ToolContext::new().with_workspace(workspace);

        // Writing to the hardlinked file should be blocked
        let result = tool
            .execute(
                json!({"path": "hardlink.txt", "content": "malicious"}),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hard links"),
            "Expected hardlink error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_edit_blocks_hardlinked_file() {
        let dir = tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a regular file
        let original = canonical.join("editable.txt");
        fs::write(&original, "Hello World").unwrap();

        // Create a hard link
        let hardlink = canonical.join("edit_link.txt");
        fs::hard_link(&original, &hardlink).unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(workspace);

        let result = tool
            .execute(
                json!({
                    "path": "edit_link.txt",
                    "old_text": "Hello",
                    "new_text": "Goodbye"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hard links"),
            "Expected hardlink error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_write_allows_single_link_file() {
        let dir = tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a regular file (nlink = 1)
        fs::write(canonical.join("normal.txt"), "original").unwrap();

        let tool = WriteFileTool;
        let ctx = ToolContext::new().with_workspace(workspace);

        let result = tool
            .execute(json!({"path": "normal.txt", "content": "updated"}), &ctx)
            .await;

        assert!(result.is_ok());
        assert_eq!(
            fs::read_to_string(canonical.join("normal.txt")).unwrap(),
            "updated"
        );
    }

    #[tokio::test]
    async fn test_edit_file_diff_mode_simple() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("diff_test.txt");
        fs::write(&file_path, "line one\nline two\nline three\n").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "diff_test.txt",
                    "diff": "@@ -1,3 +1,3 @@\n line one\n-line two\n+LINE TWO\n line three"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("Applied 1 hunk"));
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "line one\nLINE TWO\nline three\n"
        );
    }

    #[tokio::test]
    async fn test_edit_file_diff_mode_context_mismatch() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("diff_mismatch.txt");
        fs::write(&file_path, "foo\nbar\nbaz\n").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "diff_mismatch.txt",
                    "diff": "@@ -1,3 +1,3 @@\n foo\n WRONG\n baz"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("context mismatch"));
    }

    #[tokio::test]
    async fn test_edit_file_diff_and_old_text_mutually_exclusive() {
        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace("/tmp");

        let result = tool
            .execute(
                json!({
                    "path": "test.txt",
                    "diff": "@@ -1,1 +1,1 @@\n-a\n+b",
                    "old_text": "a",
                    "new_text": "b"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not both"));
    }
}
