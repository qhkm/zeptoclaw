//! Plugin tool adapter for ZeptoClaw
//!
//! This module provides `PluginTool`, an adapter that wraps a plugin tool
//! definition (`PluginToolDef`) and implements the `Tool` trait so that
//! plugin-defined commands can be executed as regular agent tools.
//!
//! # How it works
//!
//! Each `PluginTool` instance holds a single tool definition from a plugin
//! manifest. When the LLM invokes the tool, `execute()`:
//!
//! 1. Parses the JSON arguments
//! 2. Interpolates `{{param_name}}` placeholders in the command template
//! 3. Executes the resulting shell command via `tokio::process::Command`
//! 4. Returns stdout (or stderr on failure) as the tool result
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::tools::plugin::PluginTool;
//! use zeptoclaw::plugins::PluginToolDef;
//! use serde_json::json;
//!
//! let def = PluginToolDef {
//!     name: "git_status".to_string(),
//!     description: "Get git status".to_string(),
//!     parameters: json!({"type": "object", "properties": {}}),
//!     command: "git status --porcelain".to_string(),
//!     working_dir: None,
//!     timeout_secs: Some(10),
//!     env: None,
//! };
//!
//! let tool = PluginTool::new(def, "git-tools");
//! ```

use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

use crate::error::{Result, ZeptoError};
use crate::plugins::PluginToolDef;

use super::types::{Tool, ToolContext};

/// Shell-escape a string by wrapping it in single quotes.
///
/// Any embedded single quotes are escaped as `'\''` (end quote, escaped
/// quote, restart quote). This prevents command injection via `$(...)`,
/// backticks, `&&`, `;`, pipes, and all other shell metacharacters.
fn shell_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            escaped.push_str("'\\''");
        } else {
            escaped.push(ch);
        }
    }
    escaped.push('\'');
    escaped
}

/// Adapter that wraps a `PluginToolDef` and implements the `Tool` trait.
pub struct PluginTool {
    /// The plugin tool definition from the manifest.
    def: PluginToolDef,
    /// Name of the plugin that provides this tool (for logging).
    plugin_name: String,
}

impl PluginTool {
    /// Create a new plugin tool adapter.
    pub fn new(def: PluginToolDef, plugin_name: &str) -> Self {
        Self {
            def,
            plugin_name: plugin_name.to_string(),
        }
    }

    /// Interpolate `{{param_name}}` placeholders in a command template.
    ///
    /// All parameter values are shell-escaped to prevent command injection.
    /// Values are wrapped in single quotes with any embedded single quotes
    /// escaped as `'\''`.
    fn interpolate(command: &str, args: &Value) -> String {
        let mut result = command.to_string();
        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                let placeholder = format!("{{{{{}}}}}", key);
                let raw = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let replacement = shell_escape(&raw);
                result = result.replace(&placeholder, &replacement);
            }
        }
        result
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn parameters(&self) -> Value {
        self.def.parameters.clone()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let command = Self::interpolate(&self.def.command, &args);
        let timeout = Duration::from_secs(self.def.effective_timeout());

        tracing::debug!(
            plugin = %self.plugin_name,
            tool = %self.def.name,
            command = %command,
            "Executing plugin tool"
        );

        // Build the command
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(&command);

        // Apply working directory: tool def > workspace from context
        if let Some(ref wd) = self.def.working_dir {
            cmd.current_dir(wd);
        } else if let Some(ref ws) = ctx.workspace {
            cmd.current_dir(ws);
        }

        // Apply environment variables from tool definition
        if let Some(ref env_vars) = self.def.env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        // Execute with timeout
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                ZeptoError::Tool(format!(
                    "Plugin tool '{}' timed out after {}s",
                    self.def.name,
                    timeout.as_secs()
                ))
            })?
            .map_err(|e| {
                ZeptoError::Tool(format!(
                    "Failed to execute plugin tool '{}': {}",
                    self.def.name, e
                ))
            })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(ZeptoError::Tool(format!(
                "Plugin tool '{}' failed (exit {}): {}{}",
                self.def.name,
                output.status.code().unwrap_or(-1),
                stderr,
                if !stdout.is_empty() {
                    format!("\nstdout: {}", stdout)
                } else {
                    String::new()
                }
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn test_def(command: &str) -> PluginToolDef {
        PluginToolDef {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
            command: command.to_string(),
            working_dir: None,
            timeout_secs: Some(5),
            env: None,
        }
    }

    #[test]
    fn test_shell_escape_basic() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn test_shell_escape_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_shell_escape_injection_attempt() {
        // $(rm -rf /) should become a literal string, not executed
        assert_eq!(shell_escape("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_escape("`whoami`"), "'`whoami`'");
        assert_eq!(shell_escape("foo; rm -rf /"), "'foo; rm -rf /'");
        assert_eq!(shell_escape("foo && evil"), "'foo && evil'");
        assert_eq!(shell_escape("foo | evil"), "'foo | evil'");
    }

    #[test]
    fn test_interpolate_basic() {
        let cmd = "echo {{message}}";
        let args = json!({"message": "hello"});
        assert_eq!(PluginTool::interpolate(cmd, &args), "echo 'hello'");
    }

    #[test]
    fn test_interpolate_multiple() {
        let cmd = "git -C {{path}} log --oneline -{{count}}";
        let args = json!({"path": "/tmp/repo", "count": 5});
        assert_eq!(
            PluginTool::interpolate(cmd, &args),
            "git -C '/tmp/repo' log --oneline -'5'"
        );
    }

    #[test]
    fn test_interpolate_no_match() {
        let cmd = "echo hello";
        let args = json!({"unused": "val"});
        assert_eq!(PluginTool::interpolate(cmd, &args), "echo hello");
    }

    #[test]
    fn test_interpolate_missing_param() {
        let cmd = "echo {{missing}}";
        let args = json!({});
        assert_eq!(PluginTool::interpolate(cmd, &args), "echo {{missing}}");
    }

    #[test]
    fn test_interpolate_prevents_command_injection() {
        let cmd = "echo {{input}}";
        let args = json!({"input": "$(cat /etc/passwd)"});
        let result = PluginTool::interpolate(cmd, &args);
        // The $() should be inside single quotes, making it a literal string
        assert_eq!(result, "echo '$(cat /etc/passwd)'");
        assert!(!result.contains("$(cat /etc/passwd)'") || result.starts_with("echo '"));
    }

    #[test]
    fn test_tool_name() {
        let tool = PluginTool::new(test_def("echo"), "test-plugin");
        assert_eq!(tool.name(), "test_tool");
    }

    #[test]
    fn test_tool_description() {
        let tool = PluginTool::new(test_def("echo"), "test-plugin");
        assert_eq!(tool.description(), "A test tool");
    }

    #[tokio::test]
    async fn test_execute_echo() {
        let def = test_def("echo 'hello world'");
        let tool = PluginTool::new(def, "test-plugin");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "hello world");
    }

    #[tokio::test]
    async fn test_execute_with_interpolation() {
        // Note: {{msg}} is replaced with shell-escaped value 'greetings'
        // The command becomes: echo 'greetings'
        let def = test_def("echo {{msg}}");
        let tool = PluginTool::new(def, "test-plugin");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"msg": "greetings"}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "greetings");
    }

    #[tokio::test]
    async fn test_execute_blocks_command_injection() {
        let def = test_def("echo {{input}}");
        let tool = PluginTool::new(def, "test-plugin");
        let ctx = ToolContext::new();
        // This should NOT execute the subcommand — should print it literally
        let result = tool
            .execute(json!({"input": "$(echo INJECTED)"}), &ctx)
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("$(echo INJECTED)"),
            "Should contain literal $() not executed result: {}",
            output
        );
        assert!(
            !output.contains("INJECTED\n"),
            "Should not have executed the subcommand"
        );
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let def = test_def("false");
        let tool = PluginTool::new(def, "test-plugin");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_with_env() {
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "test_value".to_string());
        let def = PluginToolDef {
            name: "env_tool".to_string(),
            description: "Tests env".to_string(),
            parameters: json!({}),
            command: "echo $MY_VAR".to_string(),
            working_dir: None,
            timeout_secs: Some(5),
            env: Some(env),
        };
        let tool = PluginTool::new(def, "test-plugin");
        let ctx = ToolContext::new();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().trim(), "test_value");
    }
}
