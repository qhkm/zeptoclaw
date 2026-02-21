//! Provider plugin adapter for ZeptoClaw.
//!
//! Executes standalone provider binaries via JSON-RPC 2.0 over stdin/stdout.
//! Each `chat()` call spawns the configured binary, writes a request to stdin,
//! reads the JSON-RPC response from stdout, and returns the parsed result.
//!
//! # Protocol
//!
//! Request (one line to stdin):
//! ```json
//! {"jsonrpc":"2.0","id":1,"method":"chat","params":{"messages":[...],"tools":[...],"model":"...","options":{}}}
//! ```
//!
//! Response (one line from stdout):
//! ```json
//! {"jsonrpc":"2.0","id":1,"result":{"content":"...","tool_calls":[],"usage":{"input_tokens":0,"output_tokens":0}}}
//! ```
//!
//! Or an error:
//! ```json
//! {"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"..."}}
//! ```

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::error::{Result, ZeptoError};
use crate::providers::types::{ChatOptions, LLMProvider, LLMResponse, LLMToolCall};
use crate::providers::Usage;
use crate::session::Message;

// ---- JSON-RPC 2.0 wire types ----

#[derive(Serialize)]
struct PluginChatRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: PluginChatParams,
}

#[derive(Serialize)]
struct PluginChatParams {
    messages: Vec<Value>,
    tools: Vec<Value>,
    model: Option<String>,
    options: PluginChatOptions,
}

#[derive(Serialize)]
struct PluginChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    // TODO: forward output_format (JSON mode) when structured output is a first-class feature
    // TODO: forward stop sequences from ChatOptions.stop
}

#[derive(Deserialize)]
struct PluginChatResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<PluginChatResult>,
    error: Option<PluginChatError>,
}

#[derive(Deserialize)]
struct PluginChatResult {
    content: String,
    #[serde(default)]
    tool_calls: Value,
    #[serde(default)]
    usage: Option<PluginUsage>,
}

#[derive(Deserialize)]
struct PluginUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize)]
struct PluginChatError {
    code: i64,
    message: String,
}

// ---- Wire-format tool call ----

#[derive(Deserialize)]
struct WireToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: Option<Value>,
}

// ---- ProviderPlugin ----

/// A provider adapter that calls an external binary via JSON-RPC 2.0 over stdin/stdout.
///
/// The binary is spawned on-demand for each `chat()` call. It reads a single
/// JSON-RPC request from stdin, writes a JSON-RPC response to stdout, and exits.
///
/// # Configuration
/// ```json
/// {
///   "providers": {
///     "plugins": [
///       {"name": "myprovider", "command": "/usr/local/bin/my-llm-provider", "args": ["--mode", "chat"]}
///     ]
///   }
/// }
/// ```
pub struct ProviderPlugin {
    name: String,
    command: String,
    args: Vec<String>,
    timeout: Duration,
}

impl std::fmt::Debug for ProviderPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderPlugin")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .finish()
    }
}

impl ProviderPlugin {
    /// Create a new provider plugin.
    ///
    /// # Arguments
    /// * `name` - Provider name used in config and logging
    /// * `command` - Path to the binary
    /// * `args` - Extra arguments to pass to the binary
    pub fn new(name: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            timeout: Duration::from_secs(120),
        }
    }

    /// Override the default 120s timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs);
        self
    }
}

#[async_trait]
impl LLMProvider for ProviderPlugin {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<crate::providers::ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command;

        // Serialize messages and tools to generic JSON so the wire format
        // is provider-agnostic. Propagate serialization failures — a null element
        // in the array would produce a structurally-valid but garbage request.
        let messages_json: Vec<Value> = messages
            .iter()
            .map(|m| {
                serde_json::to_value(m).map_err(|e| {
                    ZeptoError::Provider(format!(
                        "Failed to serialize message for plugin '{}': {}",
                        self.name, e
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::to_value(t).map_err(|e| {
                    ZeptoError::Provider(format!(
                        "Failed to serialize tool definition for plugin '{}': {}",
                        self.name, e
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let request = PluginChatRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "chat".to_string(),
            params: PluginChatParams {
                messages: messages_json,
                tools: tools_json,
                model: model.map(|s| s.to_string()),
                options: PluginChatOptions {
                    max_tokens: options.max_tokens,
                    temperature: options.temperature,
                    top_p: options.top_p,
                },
            },
        };

        let request_json = serde_json::to_string(&request).map_err(|e| {
            ZeptoError::Provider(format!(
                "Failed to serialize provider plugin request: {}",
                e
            ))
        })?;

        // Spawn the binary with no shell
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            ZeptoError::Provider(format!(
                "Failed to spawn provider plugin '{}' ({}): {}",
                self.name, self.command, e
            ))
        })?;

        // Write request to stdin and close
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| {
                    ZeptoError::Provider(format!(
                        "Failed to write to provider plugin '{}' stdin: {}",
                        self.name, e
                    ))
                })?;
            stdin.write_all(b"\n").await.ok();
            // stdin dropped here, closing the pipe
        }

        // Await with timeout
        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(ZeptoError::Provider(format!(
                    "Provider plugin '{}' process error: {}",
                    self.name, e
                )));
            }
            Err(_) => {
                // child was consumed by wait_with_output()'s future which is now
                // dropped — Tokio's Child Drop impl sends SIGKILL automatically.
                return Err(ZeptoError::Provider(format!(
                    "Provider plugin '{}' timed out after {}s",
                    self.name,
                    self.timeout.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let detail = if stderr.is_empty() {
                stdout.to_string()
            } else {
                stderr.to_string()
            };
            return Err(ZeptoError::Provider(format!(
                "Provider plugin '{}' exited with code {}: {}",
                self.name,
                code,
                detail.trim()
            )));
        }

        // Find the last non-empty line of stdout
        let response_line = stdout
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");

        if response_line.is_empty() {
            return Err(ZeptoError::Provider(format!(
                "Provider plugin '{}' produced no output",
                self.name
            )));
        }

        // Parse JSON-RPC response
        let response: PluginChatResponse = serde_json::from_str(response_line).map_err(|e| {
            ZeptoError::Provider(format!(
                "Provider plugin '{}' returned invalid JSON-RPC: {} (raw: {})",
                self.name,
                e,
                &response_line[..response_line.len().min(200)]
            ))
        })?;

        // Handle JSON-RPC error
        if let Some(err) = response.error {
            warn!(
                provider = %self.name,
                code = err.code,
                "Provider plugin returned error"
            );
            return Err(ZeptoError::Provider(format!(
                "Provider plugin '{}' error (code {}): {}",
                self.name, err.code, err.message
            )));
        }

        match response.result {
            Some(result) => {
                let tool_calls = parse_tool_calls(&result.tool_calls);
                let usage = result
                    .usage
                    .map(|u| Usage::new(u.input_tokens, u.output_tokens));
                let mut llm_response = LLMResponse::with_tools(&result.content, tool_calls);
                if let Some(u) = usage {
                    llm_response = llm_response.with_usage(u);
                }
                Ok(llm_response)
            }
            None => Err(ZeptoError::Provider(format!(
                "Provider plugin '{}' returned neither result nor error",
                self.name
            ))),
        }
    }

    fn default_model(&self) -> &str {
        "plugin-default"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Parse tool calls from a JSON value, returning an empty vec on any failure.
fn parse_tool_calls(value: &Value) -> Vec<LLMToolCall> {
    let arr = match value.as_array() {
        Some(a) => a,
        None => return vec![],
    };

    arr.iter()
        .filter_map(|entry| {
            let wc: WireToolCall = serde_json::from_value(entry.clone()).ok()?;
            let id = wc.id.unwrap_or_else(|| "call_0".to_string());
            let name = wc.name?;
            let args = wc
                .arguments
                .map(|v| {
                    if v.is_string() {
                        v.as_str().unwrap_or("{}").to_string()
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_else(|| "{}".to_string());
            Some(LLMToolCall::new(&id, &name, &args))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::StreamEvent;
    use serde_json::json;

    // ---- Unit tests ----

    #[test]
    fn test_plugin_name() {
        let plugin = ProviderPlugin::new("my-llm", "/usr/bin/my-llm", vec![]);
        assert_eq!(plugin.name(), "my-llm");
    }

    #[test]
    fn test_plugin_default_model() {
        let plugin = ProviderPlugin::new("my-llm", "/usr/bin/my-llm", vec![]);
        assert_eq!(plugin.default_model(), "plugin-default");
    }

    #[test]
    fn test_request_serialization() {
        let req = PluginChatRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "chat".to_string(),
            params: PluginChatParams {
                messages: vec![json!({"role":"user","content":"hi"})],
                tools: vec![],
                model: Some("gpt-4".to_string()),
                options: PluginChatOptions {
                    max_tokens: Some(100),
                    temperature: None,
                    top_p: None,
                },
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "chat");
        assert_eq!(v["params"]["model"], "gpt-4");
        assert!(v["params"]["messages"].is_array());
    }

    #[test]
    fn test_plugin_new_stores_fields() {
        let plugin = ProviderPlugin::new(
            "test-provider",
            "/opt/providers/custom",
            vec!["--debug".to_string(), "--timeout=60".to_string()],
        );
        assert_eq!(plugin.name, "test-provider");
        assert_eq!(plugin.command, "/opt/providers/custom");
        assert_eq!(plugin.args, vec!["--debug", "--timeout=60"]);
    }

    #[test]
    fn test_plugin_with_timeout() {
        let plugin = ProviderPlugin::new("t", "/bin/t", vec![]).with_timeout(30);
        assert_eq!(plugin.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_parse_tool_calls_empty_array() {
        let v = json!([]);
        let calls = parse_tool_calls(&v);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_tool_calls_null() {
        let v = Value::Null;
        let calls = parse_tool_calls(&v);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_tool_calls_valid() {
        let v = json!([
            {"id": "call_1", "name": "web_search", "arguments": {"query": "rust"}}
        ]);
        let calls = parse_tool_calls(&v);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "web_search");
    }

    #[test]
    fn test_parse_tool_calls_string_arguments() {
        let v = json!([
            {"id": "call_2", "name": "shell", "arguments": "{\"cmd\":\"ls\"}"}
        ]);
        let calls = parse_tool_calls(&v);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn test_parse_tool_calls_missing_name_skipped() {
        // Entry without "name" should be skipped (returns None from filter_map)
        let v = json!([
            {"id": "call_1", "arguments": {"query": "rust"}}
        ]);
        let calls = parse_tool_calls(&v);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_parse_tool_calls_missing_id_defaults() {
        let v = json!([
            {"name": "my_tool", "arguments": {}}
        ]);
        let calls = parse_tool_calls(&v);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_0");
    }

    #[test]
    fn test_parse_tool_calls_invalid_entry_skipped() {
        // Malformed entry should be skipped gracefully
        let v = json!([null, {"name": "ok_tool", "id": "c1", "arguments": {}}]);
        let calls = parse_tool_calls(&v);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "ok_tool");
    }

    #[test]
    fn test_response_success_deser() {
        let json_str = r#"{"jsonrpc":"2.0","id":1,"result":{"content":"hello","tool_calls":[],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let resp: PluginChatResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result.content, "hello");
        let usage = result.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_response_error_deser() {
        let json_str =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"model error"}}"#;
        let resp: PluginChatResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert_eq!(err.message, "model error");
    }

    // ---- Process execution tests (unix only) ----

    #[cfg(unix)]
    fn create_test_script(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("provider.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{}", content)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chat_success() {
        let (_dir, script) = create_test_script(
            r#"read input
echo '{"jsonrpc":"2.0","id":1,"result":{"content":"hello from plugin","tool_calls":[]}}'"#,
        );
        let plugin = ProviderPlugin::new("test", script.to_str().unwrap(), vec![]);
        use crate::session::Message;
        let result = plugin
            .chat(
                vec![Message::user("hi")],
                vec![],
                None,
                ChatOptions::default(),
            )
            .await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let resp = result.unwrap();
        assert_eq!(resp.content, "hello from plugin");
        assert!(!resp.has_tool_calls());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chat_with_tool_calls() {
        let (_dir, script) = create_test_script(
            r#"read input
echo '{"jsonrpc":"2.0","id":1,"result":{"content":"","tool_calls":[{"id":"c1","name":"search","arguments":{"q":"rust"}}]}}'"#,
        );
        let plugin = ProviderPlugin::new("test", script.to_str().unwrap(), vec![]);
        use crate::session::Message;
        let result = plugin
            .chat(
                vec![Message::user("search rust")],
                vec![],
                None,
                ChatOptions::default(),
            )
            .await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let resp = result.unwrap();
        assert!(resp.has_tool_calls());
        assert_eq!(resp.tool_calls[0].name, "search");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chat_error_response() {
        let (_dir, script) = create_test_script(
            r#"read input
echo '{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"model unavailable"}}'"#,
        );
        let plugin = ProviderPlugin::new("test", script.to_str().unwrap(), vec![]);
        use crate::session::Message;
        let result = plugin
            .chat(
                vec![Message::user("hi")],
                vec![],
                None,
                ChatOptions::default(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("model unavailable"), "err was: {}", err);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chat_spawn_failure() {
        let plugin = ProviderPlugin::new("test", "/nonexistent/provider/binary", vec![]);
        use crate::session::Message;
        let result = plugin
            .chat(
                vec![Message::user("hi")],
                vec![],
                None,
                ChatOptions::default(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to spawn"), "err was: {}", err);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chat_timeout() {
        let (_dir, script) = create_test_script("sleep 10");
        let plugin = ProviderPlugin::new("test", script.to_str().unwrap(), vec![]).with_timeout(1);
        use crate::session::Message;
        let result = plugin
            .chat(
                vec![Message::user("hi")],
                vec![],
                None,
                ChatOptions::default(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"), "err was: {}", err);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chat_stream_fallback() {
        // chat_stream() should fall back to chat() and emit StreamEvent::Done
        let (_dir, script) = create_test_script(
            r#"read input
echo '{"jsonrpc":"2.0","id":1,"result":{"content":"streamed response","tool_calls":[]}}'"#,
        );
        let plugin = ProviderPlugin::new("test", script.to_str().unwrap(), vec![]);
        use crate::session::Message;
        let mut rx = plugin
            .chat_stream(
                vec![Message::user("hi")],
                vec![],
                None,
                ChatOptions::default(),
            )
            .await
            .unwrap();
        let event = rx.recv().await.unwrap();
        match event {
            StreamEvent::Done { content, .. } => {
                assert_eq!(content, "streamed response");
            }
            other => panic!("Expected Done, got {:?}", other),
        }
    }
}
