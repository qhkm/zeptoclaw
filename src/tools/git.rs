//! Git CLI tool for ZeptoClaw.
//!
//! Shells out to the system `git` binary. No libgit2 dependency.
//! All commands run in the configured workspace directory.
//!
//! # Supported actions
//!
//! - `status`      — working-tree status
//! - `log`         — commit history (default 10 entries)
//! - `diff`        — unstaged diff (optionally scoped to a path)
//! - `blame`       — per-line authorship for a file (requires `path`)
//! - `branch_list` — list local branches
//! - `commit`      — stage all tracked changes and commit (requires `message`)
//! - `add`         — stage a file or directory (requires `path`)
//! - `checkout`    — switch branches (requires `branch`)

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolContext, ToolOutput};

const DEFAULT_LOG_COUNT: u64 = 10;
const MAX_LOG_COUNT: u64 = 200;

/// Tool that exposes common `git` operations by shelling out to the `git` CLI.
///
/// The tool is skipped at registration time when `git` is not found on PATH
/// (checked via `GitTool::is_available()`). All commands run in the workspace
/// directory supplied by `ToolContext`. When no workspace is set the tool
/// returns an error.
pub struct GitTool;

impl GitTool {
    /// Create a new GitTool instance.
    pub fn new() -> Self {
        Self
    }

    /// Return `true` if the `git` binary is reachable on PATH.
    pub fn is_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Run `git <args>` in `dir` and return stdout as a String.
    fn run(args: &[&str], dir: &str) -> Result<String> {
        if !Path::new(dir).is_dir() {
            return Err(ZeptoError::Tool(format!(
                "Workspace '{}' is not a directory",
                dir
            )));
        }

        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| ZeptoError::Tool(format!("Failed to run git: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(stdout)
        } else {
            let detail = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            Err(ZeptoError::Tool(format!("git error: {}", detail)))
        }
    }
}

impl Default for GitTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Run git operations (status, log, diff, blame, branch_list, commit, add, checkout) in the workspace."
    }

    fn compact_description(&self) -> &str {
        "Git operations"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "log", "diff", "blame", "branch_list", "commit", "add", "checkout"],
                    "description": "Git operation to perform."
                },
                "path": {
                    "type": "string",
                    "description": "File or directory path. Required for blame and add; optional for diff."
                },
                "message": {
                    "type": "string",
                    "description": "Commit message. Required for commit."
                },
                "branch": {
                    "type": "string",
                    "description": "Branch name. Required for checkout."
                },
                "count": {
                    "type": "integer",
                    "description": "Number of log entries to return (default 10, max 200).",
                    "minimum": 1,
                    "maximum": 200
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".to_string()))?;

        let workspace = ctx
            .workspace
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ZeptoError::Tool(
                    "Workspace not configured; git tool requires a workspace".to_string(),
                )
            })?;

        match action {
            "status" => {
                let out = Self::run(&["status", "--short", "--branch"], workspace)?;
                if out.trim().is_empty() {
                    Ok(ToolOutput::llm_only("Nothing to report (clean working tree).".to_string()))
                } else {
                    Ok(ToolOutput::llm_only(out))
                }
            }

            "log" => {
                let count = args
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_LOG_COUNT)
                    .clamp(1, MAX_LOG_COUNT);

                let count_str = count.to_string();
                let format = "--pretty=format:%h %ad %an: %s";
                let out = Self::run(
                    &["log", &format!("-{}", count_str), "--date=short", format],
                    workspace,
                )?;
                if out.trim().is_empty() {
                    Ok(ToolOutput::llm_only("No commits found.".to_string()))
                } else {
                    Ok(ToolOutput::llm_only(out))
                }
            }

            "diff" => {
                let path = args.get("path").and_then(Value::as_str);
                let out = if let Some(p) = path {
                    Self::run(&["diff", "--", p], workspace)?
                } else {
                    Self::run(&["diff"], workspace)?
                };
                if out.trim().is_empty() {
                    Ok(ToolOutput::llm_only("No differences found.".to_string()))
                } else {
                    Ok(ToolOutput::llm_only(out))
                }
            }

            "blame" => {
                let path = args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Missing 'path' parameter; blame requires a file path".to_string(),
                        )
                    })?;
                Self::run(&["blame", "--", path], workspace).map(ToolOutput::llm_only)
            }

            "branch_list" => {
                let out = Self::run(&["branch", "--list", "--sort=-committerdate"], workspace)?;
                if out.trim().is_empty() {
                    Ok(ToolOutput::llm_only("No local branches found.".to_string()))
                } else {
                    Ok(ToolOutput::llm_only(out))
                }
            }

            "commit" => {
                let message = args
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Missing 'message' parameter; commit requires a commit message"
                                .to_string(),
                        )
                    })?;
                // Only commit tracked changes (no -A to avoid accidentally staging untracked files).
                Self::run(&["commit", "-m", message], workspace).map(ToolOutput::llm_only)
            }

            "add" => {
                let path = args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Missing 'path' parameter; add requires a file or directory path"
                                .to_string(),
                        )
                    })?;
                let out = Self::run(&["add", "--", path], workspace)?;
                Ok(ToolOutput::llm_only(if out.trim().is_empty() {
                    format!("Staged '{}'.", path)
                } else {
                    out
                }))
            }

            "checkout" => {
                let branch = args
                    .get("branch")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Missing 'branch' parameter; checkout requires a branch name"
                                .to_string(),
                        )
                    })?;
                Self::run(&["checkout", branch], workspace).map(ToolOutput::llm_only)
            }

            other => Err(ZeptoError::Tool(format!(
                "Unknown git action '{}'. Supported: status, log, diff, blame, branch_list, commit, add, checkout",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Repo root used by tests that need a real git workspace.
    fn repo_root() -> String {
        // Walk up from the manifest dir until we find a .git directory.
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        loop {
            if dir.join(".git").exists() {
                return dir.to_string_lossy().to_string();
            }
            if !dir.pop() {
                // Fallback: just return manifest dir and let the test handle it.
                return env!("CARGO_MANIFEST_DIR").to_string();
            }
        }
    }

    fn ctx_with_workspace(ws: &str) -> ToolContext {
        ToolContext::new().with_workspace(ws)
    }

    fn ctx_no_workspace() -> ToolContext {
        ToolContext::new()
    }

    // --- Availability ---

    #[test]
    fn test_git_is_available() {
        // git must be on PATH in any dev environment.
        assert!(GitTool::is_available(), "git binary not found on PATH");
    }

    // --- run() helper ---

    #[test]
    fn test_run_git_version() {
        let repo = repo_root();
        let out = GitTool::run(&["--version"], &repo).expect("git --version should succeed");
        assert!(out.contains("git version"), "unexpected output: {}", out);
    }

    #[test]
    fn test_run_git_invalid_dir() {
        let result = GitTool::run(&["status"], "/this/path/does/not/exist/at/all/ever");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not a directory"),
            "expected 'not a directory' in error, got: {}",
            err
        );
    }

    // --- execute() — action dispatch ---

    #[tokio::test]
    async fn test_execute_missing_action() {
        let tool = GitTool::new();
        let ctx = ctx_no_workspace();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Missing 'action'"),
            "expected missing action error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_execute_unknown_action() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "push"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unknown git action 'push'"),
            "expected unknown action error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_execute_missing_workspace() {
        let tool = GitTool::new();
        let ctx = ctx_no_workspace();
        let result = tool.execute(json!({"action": "status"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Workspace not configured"),
            "expected workspace error, got: {}",
            err
        );
    }

    // --- status ---

    #[tokio::test]
    async fn test_execute_status() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "status"}), &ctx).await;
        assert!(result.is_ok(), "status failed: {:?}", result);
        // Output is either the short status format or the clean-tree message.
        let out = result.unwrap();
        assert!(!out.for_llm.is_empty());
    }

    // --- log ---

    #[tokio::test]
    async fn test_execute_log() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool
            .execute(json!({"action": "log", "count": 5}), &ctx)
            .await;
        assert!(result.is_ok(), "log failed: {:?}", result);
        let out = result.unwrap();
        // Each log line contains a short hash and a date.
        assert!(!out.for_llm.is_empty());
    }

    // --- branch_list ---

    #[tokio::test]
    async fn test_execute_branch_list() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "branch_list"}), &ctx).await;
        assert!(result.is_ok(), "branch_list failed: {:?}", result);
        let out = result.unwrap();
        // There must be at least one branch (we are on one right now).
        assert!(
            !out.for_llm.trim().is_empty(),
            "expected at least one branch, got empty output"
        );
    }

    // --- diff ---

    #[tokio::test]
    async fn test_execute_diff_no_path() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "diff"}), &ctx).await;
        // Diff succeeds regardless of whether there are changes.
        assert!(result.is_ok(), "diff failed: {:?}", result);
    }

    // --- blame (requires path) ---

    #[tokio::test]
    async fn test_blame_requires_path() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "blame"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("path"),
            "expected 'path' in error message, got: {}",
            err
        );
    }

    // --- commit (requires message) ---

    #[tokio::test]
    async fn test_commit_requires_message() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "commit"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("message"),
            "expected 'message' in error message, got: {}",
            err
        );
    }

    // --- checkout (requires branch) ---

    #[tokio::test]
    async fn test_checkout_requires_branch() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "checkout"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("branch"),
            "expected 'branch' in error message, got: {}",
            err
        );
    }

    // --- add (requires path) ---

    #[tokio::test]
    async fn test_add_requires_path() {
        let tool = GitTool::new();
        let ctx = ctx_with_workspace(&repo_root());
        let result = tool.execute(json!({"action": "add"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("path"),
            "expected 'path' in error message, got: {}",
            err
        );
    }

    // --- tool metadata ---

    #[test]
    fn test_tool_name_and_descriptions() {
        let tool = GitTool::new();
        assert_eq!(tool.name(), "git");
        assert!(
            tool.description().len() > tool.compact_description().len(),
            "full description should be longer than compact"
        );
        assert!(tool.compact_description().contains("Git"));
    }

    #[test]
    fn test_tool_parameters_schema() {
        let tool = GitTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert!(params["properties"]["path"].is_object());
        assert!(params["properties"]["message"].is_object());
        assert!(params["properties"]["branch"].is_object());
        assert!(params["properties"]["count"].is_object());
        // action is the only required field
        let required = params["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "action");
    }

    #[test]
    fn test_default_impl() {
        let _tool = GitTool::default();
    }
}
