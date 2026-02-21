//! Shell tool for ZeptoClaw
//!
//! This module provides a tool for executing shell commands. Commands are run
//! in a subprocess with configurable timeout and workspace directory support.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Result, ZeptoError};
use crate::runtime::{ContainerConfig, ContainerRuntime, NativeRuntime};
use crate::security::ShellSecurityConfig;

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Tool for executing shell commands.
///
/// Executes a shell command and returns the combined stdout and stderr output.
/// Commands are run using `sh -c` for shell interpretation.
///
/// # Parameters
/// - `command`: The shell command to execute (required)
/// - `timeout`: Timeout in seconds, defaults to 60 (optional)
///
/// # Security
/// This tool validates commands against a configurable blocklist to prevent
/// dangerous operations. Use `ShellTool::permissive()` to disable security
/// checks in trusted environments.
///
/// # Example
/// ```rust
/// use zeptoclaw::tools::{Tool, ToolContext};
/// use zeptoclaw::tools::shell::ShellTool;
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let tool = ShellTool::new();
/// let ctx = ToolContext::new();
/// let result = tool.execute(json!({"command": "echo hello"}), &ctx).await;
/// assert!(result.is_ok());
/// assert_eq!(result.unwrap().for_llm.trim(), "hello");
/// # });
/// ```
pub struct ShellTool {
    security_config: ShellSecurityConfig,
    runtime: Arc<dyn ContainerRuntime>,
}

impl ShellTool {
    /// Create a new shell tool with default security settings and native runtime.
    pub fn new() -> Self {
        Self {
            security_config: ShellSecurityConfig::new(),
            runtime: Arc::new(NativeRuntime::new()),
        }
    }

    /// Create a shell tool with custom security configuration and native runtime.
    pub fn with_security(security_config: ShellSecurityConfig) -> Self {
        Self {
            security_config,
            runtime: Arc::new(NativeRuntime::new()),
        }
    }

    /// Create a shell tool with default security and custom runtime.
    pub fn with_runtime(runtime: Arc<dyn ContainerRuntime>) -> Self {
        Self {
            security_config: ShellSecurityConfig::new(),
            runtime,
        }
    }

    /// Create a shell tool with custom security configuration and runtime.
    pub fn with_security_and_runtime(
        security_config: ShellSecurityConfig,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self {
        Self {
            security_config,
            runtime,
        }
    }

    /// Create a shell tool with no security restrictions and native runtime.
    ///
    /// # Warning
    /// Only use in trusted environments where command injection is not a concern.
    pub fn permissive() -> Self {
        Self {
            security_config: ShellSecurityConfig::permissive(),
            runtime: Arc::new(NativeRuntime::new()),
        }
    }

    /// Get the name of the runtime being used.
    pub fn runtime_name(&self) -> &str {
        self.runtime.name()
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return the output"
    }

    fn compact_description(&self) -> &str {
        "Run shell command"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 60)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'command' argument".into()))?;

        // Security check
        self.security_config.validate_command(command)?;

        let timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(60);

        // Build container configuration
        let mut container_config = ContainerConfig::new().with_timeout(timeout_secs);

        // Set working directory and mount if workspace is specified
        if let Some(ref workspace) = ctx.workspace {
            let workspace_path = PathBuf::from(workspace);
            container_config = container_config
                .with_workdir(workspace_path.clone())
                .with_mount(workspace_path.clone(), workspace_path, false);
        }

        // Execute command via runtime
        let output = self
            .runtime
            .execute(command, &container_config)
            .await
            .map_err(|e| ZeptoError::Tool(e.to_string()))?;

        Ok(ToolOutput::user_visible(output.format()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_shell_echo() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"command": "echo hello"}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm.trim(), "hello");
    }

    #[tokio::test]
    async fn test_shell_multiple_commands() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "echo first && echo second"}), &ctx)
            .await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("first"));
        assert!(output.contains("second"));
    }

    #[tokio::test]
    async fn test_shell_with_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "workspace file").unwrap();

        let tool = ShellTool::new();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"command": "cat test.txt"}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm.trim(), "workspace file");
    }

    #[tokio::test]
    async fn test_shell_pwd_with_workspace() {
        let dir = tempdir().unwrap();

        let tool = ShellTool::new();
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool.execute(json!({"command": "pwd"}), &ctx).await;
        assert!(result.is_ok());

        // The output should contain the temp directory path
        let output = result.unwrap().for_llm;
        // On macOS, /tmp is symlinked to /private/tmp, so we compare canonical paths
        let expected = dir.path().canonicalize().unwrap();
        let actual_path = std::path::Path::new(output.trim());
        let actual = actual_path
            .canonicalize()
            .unwrap_or_else(|_| actual_path.to_path_buf());
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_shell_stderr() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "echo error >&2"}), &ctx)
            .await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("error"));
    }

    #[tokio::test]
    async fn test_shell_combined_output() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "echo stdout && echo stderr >&2"}), &ctx)
            .await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("stdout"));
        assert!(output.contains("stderr"));
        assert!(output.contains("--- stderr ---"));
    }

    #[tokio::test]
    async fn test_shell_exit_code() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"command": "exit 42"}), &ctx).await;
        assert!(result.is_ok());
        let output = result.unwrap().for_llm;
        assert!(output.contains("[Exit code: 42]"));
    }

    #[tokio::test]
    async fn test_shell_failed_command() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "ls /nonexistent_picoclaw_path"}), &ctx)
            .await;
        assert!(result.is_ok()); // The tool returns Ok with error in output
        let output = result.unwrap().for_llm;
        assert!(output.contains("Exit code:") || output.contains("No such file"));
    }

    #[tokio::test]
    async fn test_shell_missing_command() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'command'"));
    }

    #[tokio::test]
    async fn test_shell_timeout() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "sleep 10", "timeout": 1}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn test_shell_custom_timeout_success() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(
                json!({"command": "sleep 0.1 && echo done", "timeout": 5}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("done"));
    }

    #[tokio::test]
    async fn test_shell_environment_variables() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "MY_VAR=hello && echo $MY_VAR"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("hello"));
    }

    #[tokio::test]
    async fn test_shell_piped_commands() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "echo 'hello world' | tr ' ' '-'"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm.trim(), "hello-world");
    }

    #[tokio::test]
    async fn test_shell_special_characters() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "echo \"hello 'world'\""}), &ctx)
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("hello 'world'"));
    }

    #[test]
    fn test_shell_tool_name() {
        assert_eq!(ShellTool::new().name(), "shell");
    }

    #[test]
    fn test_shell_tool_description() {
        assert!(!ShellTool::new().description().is_empty());
        assert!(ShellTool::new().description().contains("shell"));
    }

    #[test]
    fn test_shell_tool_parameters() {
        let params = ShellTool::new().parameters();
        assert!(params.is_object());
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["command"].is_object());
        assert!(params["properties"]["timeout"].is_object());
        assert_eq!(params["required"][0], "command");
    }

    #[tokio::test]
    async fn test_dangerous_command_blocked() {
        let tool = ShellTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"command": "rm -rf /"}), &ctx).await;
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(err.contains("Security violation"));
    }

    #[tokio::test]
    async fn test_permissive_mode_allows_dangerous() {
        let tool = ShellTool::permissive();
        let ctx = ToolContext::new();

        // This would normally be blocked, but we're just testing the security bypass
        // Don't actually execute rm -rf /!
        let result = tool
            .execute(json!({"command": "echo 'rm -rf /'"}), &ctx)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_custom_security_config() {
        let config = ShellSecurityConfig::new().block_pattern("forbidden");
        let tool = ShellTool::with_security(config);
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"command": "echo forbidden"}), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_shell_tool_default() {
        let tool = ShellTool::default();
        // Should have security enabled by default
        assert!(tool.security_config.enabled);
    }

    #[test]
    fn test_shell_tool_runtime_name() {
        let tool = ShellTool::new();
        assert_eq!(tool.runtime_name(), "native");
    }

    #[tokio::test]
    async fn test_shell_tool_with_custom_runtime() {
        use crate::runtime::NativeRuntime;
        use std::sync::Arc;

        let runtime = Arc::new(NativeRuntime::new());
        let tool = ShellTool::with_runtime(runtime);
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"command": "echo test"}), &ctx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().for_llm.contains("test"));
    }

    #[test]
    fn test_shell_tool_with_security_and_runtime() {
        use crate::runtime::NativeRuntime;
        use std::sync::Arc;

        let security = ShellSecurityConfig::permissive();
        let runtime = Arc::new(NativeRuntime::new());
        let tool = ShellTool::with_security_and_runtime(security, runtime);

        assert_eq!(tool.runtime_name(), "native");
        assert!(!tool.security_config.enabled);
    }

    #[test]
    fn test_shell_tool_default_uses_native_runtime() {
        let tool = ShellTool::default();
        assert_eq!(tool.runtime_name(), "native");
    }

    #[tokio::test]
    async fn test_shell_tool_permissive_uses_native_runtime() {
        let tool = ShellTool::permissive();
        assert_eq!(tool.runtime_name(), "native");
    }
}
