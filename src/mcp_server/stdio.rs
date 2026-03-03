//! Stdio transport for the MCP server.
//!
//! Reads line-delimited JSON-RPC 2.0 from stdin, dispatches to the handler,
//! writes JSON responses to stdout (one per line).

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error};

use crate::kernel::ZeptoKernel;
use crate::tools::mcp::protocol::{McpError, McpResponse};

use super::handler;

/// Run the MCP server over stdio.
///
/// Reads JSON-RPC lines from stdin, processes each through `handler::handle_request`,
/// and writes JSON responses to stdout.  Exits cleanly on EOF.
pub async fn run_stdio(kernel: &ZeptoKernel) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        debug!(line = %line, "MCP stdin: received");

        let resp = process_line(kernel, &line).await;
        let output = serde_json::to_string(&resp).unwrap_or_else(|e| {
            // Fallback: emit a parse-error response as raw JSON.
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"Serialization error: {}"}}}}"#,
                e
            )
        });

        stdout.write_all(output.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    debug!("MCP stdin: EOF, shutting down");
    Ok(())
}

/// Process a single JSON line into an MCP response.
async fn process_line(kernel: &ZeptoKernel, line: &str) -> McpResponse {
    // Parse JSON
    let parsed: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "MCP: JSON parse error");
            return make_parse_error();
        }
    };

    // Validate jsonrpc field
    if parsed.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        return make_invalid_request(
            extract_id(&parsed),
            "Missing or invalid 'jsonrpc' field (expected \"2.0\")".to_string(),
        );
    }

    // Extract method
    let method = match parsed.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return make_invalid_request(
                extract_id(&parsed),
                "Missing or invalid 'method' field".to_string(),
            );
        }
    };

    let id = extract_id(&parsed);
    let params = parsed.get("params").cloned();

    handler::handle_request(kernel, id, &method, params).await
}

/// Extract the `id` field from a JSON-RPC envelope.
///
/// Returns `None` for notifications (missing or null id).
fn extract_id(value: &Value) -> Option<u64> {
    value.get("id").and_then(|v| v.as_u64())
}

/// JSON-RPC parse error (-32700).
fn make_parse_error() -> McpResponse {
    McpResponse {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: Some(McpError {
            code: -32700,
            message: "Parse error: invalid JSON".to_string(),
            data: None,
        }),
    }
}

/// JSON-RPC invalid request (-32600).
fn make_invalid_request(id: Option<u64>, message: String) -> McpResponse {
    McpResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(McpError {
            code: -32600,
            message,
            data: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::hooks::HookEngine;
    use crate::safety::SafetyLayer;
    use crate::tools::{EchoTool, ToolRegistry};
    use crate::utils::metrics::MetricsCollector;
    use serde_json::json;
    use std::sync::Arc;

    fn test_kernel() -> ZeptoKernel {
        let config = Config::default();
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));

        ZeptoKernel {
            config: Arc::new(config.clone()),
            provider: None,
            tools,
            safety: if config.safety.enabled {
                Some(SafetyLayer::new(config.safety.clone()))
            } else {
                None
            },
            metrics: Arc::new(MetricsCollector::new()),
            hooks: Arc::new(HookEngine::new(config.hooks.clone())),
            mcp_clients: vec![],
            ltm: None,
        }
    }

    #[test]
    fn test_extract_id_present() {
        let v = json!({"id": 42, "jsonrpc": "2.0", "method": "test"});
        assert_eq!(extract_id(&v), Some(42));
    }

    #[test]
    fn test_extract_id_missing() {
        let v = json!({"jsonrpc": "2.0", "method": "test"});
        assert_eq!(extract_id(&v), None);
    }

    #[test]
    fn test_extract_id_null() {
        let v = json!({"id": null, "jsonrpc": "2.0", "method": "test"});
        assert_eq!(extract_id(&v), None);
    }

    #[test]
    fn test_extract_id_string() {
        // MCP spec uses numeric IDs; string IDs return None from as_u64
        let v = json!({"id": "abc", "jsonrpc": "2.0", "method": "test"});
        assert_eq!(extract_id(&v), None);
    }

    #[test]
    fn test_make_parse_error() {
        let resp = make_parse_error();
        assert!(resp.id.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32700);
        assert!(err.message.contains("Parse error"));
    }

    #[test]
    fn test_make_invalid_request_with_id() {
        let resp = make_invalid_request(Some(5), "bad request".to_string());
        assert_eq!(resp.id, Some(5));
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "bad request");
    }

    #[test]
    fn test_make_invalid_request_without_id() {
        let resp = make_invalid_request(None, "no id".to_string());
        assert!(resp.id.is_none());
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn test_process_line_valid_initialize() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let resp = process_line(&kernel, line).await;

        assert_eq!(resp.id, Some(1));
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn test_process_line_invalid_json() {
        let kernel = test_kernel();
        let resp = process_line(&kernel, "not json at all").await;

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32700);
    }

    #[tokio::test]
    async fn test_process_line_missing_jsonrpc() {
        let kernel = test_kernel();
        let line = r#"{"id":1,"method":"initialize"}"#;
        let resp = process_line(&kernel, line).await;

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[tokio::test]
    async fn test_process_line_wrong_jsonrpc_version() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"1.0","id":1,"method":"initialize"}"#;
        let resp = process_line(&kernel, line).await;

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[tokio::test]
    async fn test_process_line_missing_method() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","id":1}"#;
        let resp = process_line(&kernel, line).await;

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[tokio::test]
    async fn test_process_line_tools_call() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"message":"test"}}}"#;
        let resp = process_line(&kernel, line).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "test");
    }

    #[tokio::test]
    async fn test_process_line_notification() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let resp = process_line(&kernel, line).await;

        assert!(resp.error.is_none());
        assert!(resp.id.is_none());
    }
}
