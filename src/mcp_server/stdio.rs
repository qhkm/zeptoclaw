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

        // Log only safe metadata (size, method, id) -- never the full payload
        // which may contain secrets in tool arguments.
        let (log_method, log_id) = extract_log_metadata(&line);
        debug!(
            size = line.len(),
            method = %log_method,
            id = %log_id,
            "MCP stdin: received"
        );

        let resp = process_line(kernel, &line).await;

        // JSON-RPC 2.0 spec: notifications (no id) MUST NOT receive a
        // response.  Detect via the method name extracted from the raw line.
        if resp.is_none() {
            continue;
        }
        let resp = resp.unwrap();

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
///
/// Returns `None` for notifications (methods starting with `notifications/`)
/// since the JSON-RPC 2.0 spec forbids replying to them.
async fn process_line(kernel: &ZeptoKernel, line: &str) -> Option<McpResponse> {
    // Parse JSON
    let parsed: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "MCP: JSON parse error");
            return Some(make_parse_error());
        }
    };

    // Validate jsonrpc field
    if parsed.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        return Some(make_invalid_request(
            extract_id(&parsed),
            "Missing or invalid 'jsonrpc' field (expected \"2.0\")".to_string(),
        ));
    }

    // Extract method
    let method = match parsed.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return Some(make_invalid_request(
                extract_id(&parsed),
                "Missing or invalid 'method' field".to_string(),
            ));
        }
    };

    let id = extract_id(&parsed);
    let params = parsed.get("params").cloned();

    let resp = handler::handle_request(kernel, id, &method, params).await;

    // Suppress responses for notifications per JSON-RPC 2.0 spec.
    if handler::is_notification(&method) {
        return None;
    }

    Some(resp)
}

/// Extract safe metadata (method, id) from a raw JSON line for logging.
///
/// Best-effort: parses the JSON and extracts only the "method" and "id"
/// fields. Returns placeholders if parsing fails.
fn extract_log_metadata(line: &str) -> (String, String) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return ("<invalid-json>".to_string(), "null".to_string());
    };
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("<missing>")
        .to_string();
    let id = v
        .get("id")
        .map(|i| i.to_string())
        .unwrap_or_else(|| "null".to_string());
    (method, id)
}

/// Extract the `id` field from a JSON-RPC envelope.
///
/// Returns the raw `serde_json::Value` to preserve the original type
/// (number, string, or null) as required by JSON-RPC 2.0.
/// Returns `None` for notifications (missing or null id).
fn extract_id(value: &Value) -> Option<Value> {
    value
        .get("id")
        .and_then(|v| if v.is_null() { None } else { Some(v.clone()) })
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
fn make_invalid_request(id: Option<Value>, message: String) -> McpResponse {
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
            taint: None,
        }
    }

    #[test]
    fn test_extract_id_present() {
        let v = json!({"id": 42, "jsonrpc": "2.0", "method": "test"});
        assert_eq!(extract_id(&v), Some(json!(42)));
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
        // JSON-RPC 2.0 allows string IDs; extract_id preserves them
        let v = json!({"id": "abc", "jsonrpc": "2.0", "method": "test"});
        assert_eq!(extract_id(&v), Some(json!("abc")));
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
        let resp = make_invalid_request(Some(json!(5)), "bad request".to_string());
        assert_eq!(resp.id, Some(json!(5)));
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
        let resp = process_line(&kernel, line)
            .await
            .expect("should return response");

        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn test_process_line_invalid_json() {
        let kernel = test_kernel();
        let resp = process_line(&kernel, "not json at all")
            .await
            .expect("parse errors still produce a response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32700);
    }

    #[tokio::test]
    async fn test_process_line_missing_jsonrpc() {
        let kernel = test_kernel();
        let line = r#"{"id":1,"method":"initialize"}"#;
        let resp = process_line(&kernel, line)
            .await
            .expect("should return response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[tokio::test]
    async fn test_process_line_wrong_jsonrpc_version() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"1.0","id":1,"method":"initialize"}"#;
        let resp = process_line(&kernel, line)
            .await
            .expect("should return response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[tokio::test]
    async fn test_process_line_missing_method() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","id":1}"#;
        let resp = process_line(&kernel, line)
            .await
            .expect("should return response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    #[tokio::test]
    async fn test_process_line_tools_call() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"message":"test"}}}"#;
        let resp = process_line(&kernel, line)
            .await
            .expect("should return response");

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "test");
    }

    #[tokio::test]
    async fn test_process_line_notification_returns_none() {
        // JSON-RPC 2.0 spec: notifications MUST NOT receive a response.
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let resp = process_line(&kernel, line).await;

        assert!(
            resp.is_none(),
            "notifications should not produce a response"
        );
    }

    #[tokio::test]
    async fn test_process_line_string_id_preserved() {
        let kernel = test_kernel();
        let line = r#"{"jsonrpc":"2.0","id":"req-abc","method":"initialize"}"#;
        let resp = process_line(&kernel, line)
            .await
            .expect("should return response");

        assert_eq!(resp.id, Some(json!("req-abc")));
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_process_line_malformed_json_returns_parse_error() {
        // Ensures garbled input gets a proper -32700 response (not a
        // transport-level HTTP error).
        let kernel = test_kernel();
        for bad_input in &["{invalid", "}{", "", "null", "42", r#""string""#] {
            let resp = process_line(&kernel, bad_input).await;
            // Only truly un-parseable JSON returns -32700; valid JSON values
            // like "null", "42", or a string are valid JSON but fail later
            // checks. We only assert the first two here.
            if bad_input == &"{invalid" || bad_input == &"}{" {
                let resp = resp.expect("parse errors produce a response");
                assert_eq!(
                    resp.error.as_ref().unwrap().code,
                    -32700,
                    "input {bad_input:?} should produce -32700"
                );
            }
        }
    }

    #[test]
    fn test_extract_log_metadata_valid() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo"}}"#;
        let (method, id) = extract_log_metadata(line);
        assert_eq!(method, "tools/call");
        assert_eq!(id, "1");
    }

    #[test]
    fn test_extract_log_metadata_invalid_json() {
        let (method, id) = extract_log_metadata("not json");
        assert_eq!(method, "<invalid-json>");
        assert_eq!(id, "null");
    }

    #[test]
    fn test_extract_log_metadata_missing_fields() {
        let line = r#"{"jsonrpc":"2.0"}"#;
        let (method, id) = extract_log_metadata(line);
        assert_eq!(method, "<missing>");
        assert_eq!(id, "null");
    }
}
