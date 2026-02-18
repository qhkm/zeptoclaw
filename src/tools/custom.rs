//! Custom CLI-defined tool adapter for ZeptoClaw.
//!
//! Wraps a [`CustomToolDef`] from config and implements the [`Tool`] trait.
//! Commands execute via `sh -c` with shell-escaped parameter interpolation.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

use crate::config::CustomToolDef;
use crate::error::{Result, ZeptoError};
use crate::security::ShellSecurityConfig;

use super::types::{Tool, ToolContext};

/// Maximum output bytes to capture from custom tool stdout (50KB).
const MAX_OUTPUT_BYTES: usize = 50_000;

/// Minimum timeout in seconds for custom tool execution.
const MIN_TIMEOUT_SECS: u64 = 1;

/// Shell-escape a value by wrapping in single quotes.
/// Embedded single quotes become `'\''`.
fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Interpolate `{{key}}` placeholders in a command template with shell-escaped values.
fn interpolate(template: &str, args: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in args {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, &shell_escape(value));
    }
    result
}

/// A tool defined as a shell command in config.
pub struct CustomTool {
    def: CustomToolDef,
    security: ShellSecurityConfig,
}

impl CustomTool {
    /// Create a new custom tool from a config definition.
    pub fn new(def: CustomToolDef) -> Self {
        Self {
            def,
            security: ShellSecurityConfig::default(),
        }
    }
}

#[async_trait]
impl Tool for CustomTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn compact_description(&self) -> &str {
        self.description()
    }

    fn parameters(&self) -> Value {
        match &self.def.parameters {
            None => json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            Some(params) => {
                let mut properties = serde_json::Map::new();
                let mut required = Vec::new();
                for (name, type_str) in params {
                    properties.insert(name.clone(), json!({"type": type_str}));
                    required.push(json!(name));
                }
                json!({
                    "type": "object",
                    "properties": properties,
                    "required": required
                })
            }
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        // Extract string args from JSON for interpolation
        let string_args: HashMap<String, String> = if let Some(obj) = args.as_object() {
            obj.iter()
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), val)
                })
                .collect()
        } else {
            HashMap::new()
        };

        // Interpolate command template
        let command = interpolate(&self.def.command, &string_args);

        // Validate against shell security blocklist (cached config, no regex recompilation)
        if let Err(e) = self.security.validate_command(&command) {
            return Err(ZeptoError::Tool(format!(
                "Command blocked by security policy: {}",
                e
            )));
        }

        debug!(tool = %self.def.name, command = %command, "Executing custom tool");

        // Determine timeout (clamp to minimum to prevent zero-duration timeouts)
        let timeout_secs = self.def.timeout_secs.unwrap_or(30).max(MIN_TIMEOUT_SECS);
        let timeout = Duration::from_secs(timeout_secs);

        // Build command
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(&command);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Working directory: custom def > workspace > current dir
        if let Some(ref dir) = self.def.working_dir {
            cmd.current_dir(dir);
        } else if let Some(ref ws) = ctx.workspace {
            cmd.current_dir(ws);
        }

        // Environment variables
        if let Some(ref env_vars) = self.def.env {
            for (k, v) in env_vars {
                cmd.env(k, v);
            }
        }

        // Execute with timeout
        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(ZeptoError::Tool(format!(
                    "Failed to execute command: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(ZeptoError::Tool(format!(
                    "Command timed out after {}s",
                    timeout_secs
                )));
            }
        };

        if output.status.success() {
            let mut stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // Truncate oversized output to prevent blowing up the LLM context
            if stdout.len() > MAX_OUTPUT_BYTES {
                let mut end = MAX_OUTPUT_BYTES;
                while !stdout.is_char_boundary(end) {
                    end -= 1;
                }
                stdout.truncate(end);
                stdout.push_str("\n... (output truncated)");
            }
            Ok(if stdout.is_empty() {
                "(no output)".to_string()
            } else {
                stdout
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Err(ZeptoError::Tool(format!(
                "Command failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { &stdout } else { &stderr }
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::types::ToolContext;

    fn test_ctx() -> ToolContext {
        ToolContext {
            workspace: Some(std::env::temp_dir().to_string_lossy().to_string()),
            channel: None,
            chat_id: None,
        }
    }

    fn simple_def(name: &str, command: &str) -> CustomToolDef {
        CustomToolDef {
            name: name.to_string(),
            description: format!("Test tool {}", name),
            command: command.to_string(),
            parameters: None,
            working_dir: None,
            timeout_secs: None,
            env: None,
        }
    }

    // === Unit tests ===

    #[test]
    fn test_shell_escape_basic() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn test_shell_escape_injection() {
        let escaped = shell_escape("'; rm -rf / #");
        assert_eq!(escaped, "''\\''; rm -rf / #'");
        // The result when passed to sh -c is treated as a literal string
    }

    #[test]
    fn test_interpolate_basic() {
        let mut args = HashMap::new();
        args.insert("name".to_string(), "world".to_string());
        let result = interpolate("echo {{name}}", &args);
        assert_eq!(result, "echo 'world'");
    }

    #[test]
    fn test_interpolate_missing_param() {
        let args = HashMap::new();
        let result = interpolate("echo {{name}}", &args);
        // Missing params are left as-is
        assert_eq!(result, "echo {{name}}");
    }

    #[test]
    fn test_tool_name() {
        let tool = CustomTool::new(simple_def("cpu_temp", "echo 42"));
        assert_eq!(tool.name(), "cpu_temp");
    }

    #[test]
    fn test_tool_description() {
        let tool = CustomTool::new(simple_def("cpu_temp", "echo 42"));
        assert_eq!(tool.description(), "Test tool cpu_temp");
    }

    #[test]
    fn test_parameters_no_params() {
        let tool = CustomTool::new(simple_def("test", "echo"));
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_parameters_with_params() {
        let mut def = simple_def("test", "echo");
        let mut params = HashMap::new();
        params.insert("pattern".to_string(), "string".to_string());
        params.insert("limit".to_string(), "integer".to_string());
        def.parameters = Some(params);
        let tool = CustomTool::new(def);
        let schema = tool.parameters();
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(props.len(), 2);
        assert_eq!(props["pattern"]["type"], "string");
        assert_eq!(props["limit"]["type"], "integer");
    }

    #[test]
    fn test_security_config_cached() {
        let tool = CustomTool::new(simple_def("test", "echo hi"));
        // Verify security config is constructed once and stored
        assert!(tool.security.validate_command("echo hello").is_ok());
    }

    #[test]
    fn test_min_timeout_clamped() {
        let mut def = simple_def("test", "echo");
        def.timeout_secs = Some(0);
        let tool = CustomTool::new(def);
        // timeout_secs of 0 should be clamped to MIN_TIMEOUT_SECS
        assert_eq!(
            tool.def.timeout_secs.unwrap_or(30).max(MIN_TIMEOUT_SECS),
            MIN_TIMEOUT_SECS
        );
    }

    // === Async execution tests ===

    #[tokio::test]
    async fn test_execute_simple_command() {
        let tool = CustomTool::new(simple_def("test", "echo hello"));
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_execute_with_interpolation() {
        let mut def = simple_def("test", "echo {{msg}}");
        let mut params = HashMap::new();
        params.insert("msg".to_string(), "string".to_string());
        def.parameters = Some(params);
        let tool = CustomTool::new(def);
        let result = tool
            .execute(json!({"msg": "hello world"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result, "hello world");
    }

    #[tokio::test]
    async fn test_execute_blocks_injection() {
        let mut def = simple_def("test", "echo {{msg}}");
        let mut params = HashMap::new();
        params.insert("msg".to_string(), "string".to_string());
        def.parameters = Some(params);
        let tool = CustomTool::new(def);
        // Injection attempt: $(whoami) should be treated as literal
        let result = tool
            .execute(json!({"msg": "$(whoami)"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result, "$(whoami)");
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let tool = CustomTool::new(simple_def("test", "exit 1"));
        let result = tool.execute(json!({}), &test_ctx()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed") || err.contains("exit 1"),
            "Got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let mut def = simple_def("test", "sleep 10");
        def.timeout_secs = Some(1);
        let tool = CustomTool::new(def);
        let result = tool.execute(json!({}), &test_ctx()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_with_env() {
        let mut def = simple_def("test", "echo $TEST_VAR_CUSTOM");
        let mut env = HashMap::new();
        env.insert("TEST_VAR_CUSTOM".to_string(), "custom_value".to_string());
        def.env = Some(env);
        let tool = CustomTool::new(def);
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert_eq!(result, "custom_value");
    }

    #[tokio::test]
    async fn test_execute_with_working_dir() {
        let tool = CustomTool::new(CustomToolDef {
            name: "test".to_string(),
            description: "test".to_string(),
            command: "pwd".to_string(),
            parameters: None,
            working_dir: Some("/tmp".to_string()),
            timeout_secs: None,
            env: None,
        });
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        // On macOS /tmp is a symlink to /private/tmp
        assert!(result.contains("tmp"), "Got: {}", result);
    }

    #[tokio::test]
    async fn test_execute_empty_stdout() {
        let tool = CustomTool::new(simple_def("test", "true"));
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert_eq!(result, "(no output)");
    }

    #[tokio::test]
    async fn test_execute_shell_blocklist() {
        // The ShellSecurityConfig blocks dangerous patterns like `rm -rf /`
        let tool = CustomTool::new(simple_def("test", "rm -rf /"));
        let result = tool.execute(json!({}), &test_ctx()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("blocked") || err.contains("security"),
            "Got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_execute_no_params_ignores_args() {
        let tool = CustomTool::new(simple_def("test", "echo fixed"));
        // Extra args should be ignored for no-param tools
        let result = tool
            .execute(json!({"extra": "stuff"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(result, "fixed");
    }

    #[tokio::test]
    async fn test_execute_output_truncated() {
        // Generate output larger than MAX_OUTPUT_BYTES
        let repeat = MAX_OUTPUT_BYTES + 1000;
        let cmd = format!("printf '%0.s-' $(seq 1 {})", repeat);
        let tool = CustomTool::new(simple_def("test", &cmd));
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(result.contains("(output truncated)"));
        // Output should be capped near MAX_OUTPUT_BYTES
        assert!(result.len() <= MAX_OUTPUT_BYTES + 100);
    }
}
