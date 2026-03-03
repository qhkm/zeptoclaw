//! MCP JSON-RPC 2.0 request handler.
//!
//! Maps incoming JSON-RPC methods to kernel operations:
//! - `initialize` -> server capabilities
//! - `notifications/initialized` -> acknowledgement
//! - `tools/list` -> kernel tool definitions
//! - `tools/call` -> kernel tool execution

use serde_json::{json, Value};
use tracing::warn;

use crate::kernel::ZeptoKernel;
use crate::tools::mcp::protocol::{
    CallToolResult, ContentBlock, ListToolsResult, McpError, McpResponse, McpTool,
};
use crate::tools::ToolContext;

/// MCP protocol version we implement.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server name reported during initialization.
const SERVER_NAME: &str = "zeptoclaw";

/// Check whether a JSON-RPC method is a notification (no response expected).
///
/// Per JSON-RPC 2.0, notifications are requests without an `id`. MCP uses
/// the convention that notification method names start with `notifications/`.
pub fn is_notification(method: &str) -> bool {
    method.starts_with("notifications/")
}

/// Handle a parsed JSON-RPC 2.0 request and return a response.
///
/// For notifications (`is_notification()` returns true), callers should
/// discard the response — the JSON-RPC 2.0 spec forbids replying to
/// notifications.
///
/// The `id` is a `serde_json::Value` to preserve the original type sent by
/// the client (number, string, or null) as required by JSON-RPC 2.0.
pub async fn handle_request(
    kernel: &ZeptoKernel,
    id: Option<Value>,
    method: &str,
    params: Option<Value>,
) -> McpResponse {
    match method {
        "initialize" => handle_initialize(id),
        "notifications/initialized" => handle_notifications_initialized(id),
        "tools/list" => handle_tools_list(kernel, id),
        "tools/call" => handle_tools_call(kernel, id, params).await,
        _ => {
            warn!(method = method, "MCP server: unknown method");
            make_error(id, -32601, format!("Method not found: {method}"))
        }
    }
}

/// `initialize` -- return server info and capabilities.
fn handle_initialize(id: Option<Value>) -> McpResponse {
    let result = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION")
        }
    });

    McpResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(result),
        error: None,
    }
}

/// `notifications/initialized` -- client acknowledges init. No-op.
fn handle_notifications_initialized(id: Option<Value>) -> McpResponse {
    McpResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(json!({})),
        error: None,
    }
}

/// `tools/list` -- return all registered tools as MCP tool definitions.
fn handle_tools_list(kernel: &ZeptoKernel, id: Option<Value>) -> McpResponse {
    let defs = kernel.tool_definitions();
    let tools: Vec<McpTool> = defs
        .into_iter()
        .map(|d| McpTool {
            name: d.name,
            description: Some(d.description),
            input_schema: d.parameters,
        })
        .collect();

    let result = ListToolsResult { tools };
    McpResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(serde_json::to_value(result).unwrap_or(json!({}))),
        error: None,
    }
}

/// `tools/call` -- execute a tool and return the result.
async fn handle_tools_call(
    kernel: &ZeptoKernel,
    id: Option<Value>,
    params: Option<Value>,
) -> McpResponse {
    let params = match params {
        Some(p) => p,
        None => {
            return make_error(id, -32602, "Missing params for tools/call".to_string());
        }
    };

    let tool_name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return make_error(
                id,
                -32602,
                "Missing or invalid 'name' in tools/call params".to_string(),
            );
        }
    };

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let ctx = ToolContext::new().with_channel("mcp", "mcp-server");

    let output = crate::kernel::execute_tool(
        &kernel.tools,
        &tool_name,
        arguments,
        &ctx,
        kernel.safety.as_ref(),
        &kernel.metrics,
        kernel.taint.as_ref(),
    )
    .await;

    match output {
        Ok(tool_output) => {
            let result = CallToolResult {
                content: vec![ContentBlock::Text {
                    text: tool_output.for_llm,
                }],
                is_error: tool_output.is_error,
            };
            McpResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::to_value(result).unwrap_or(json!({}))),
                error: None,
            }
        }
        Err(e) => {
            let result = CallToolResult {
                content: vec![ContentBlock::Text {
                    text: format!("Tool execution error: {e}"),
                }],
                is_error: true,
            };
            McpResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::to_value(result).unwrap_or(json!({}))),
                error: None,
            }
        }
    }
}

/// Build an error response.
fn make_error(id: Option<Value>, code: i64, message: String) -> McpResponse {
    McpResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(McpError {
            code,
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
    use std::sync::Arc;

    /// Build a minimal test kernel with one tool (echo).
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

    #[tokio::test]
    async fn test_handle_initialize() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!(1)), "initialize", None).await;

        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
    }

    #[tokio::test]
    async fn test_handle_initialize_has_version() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!(1)), "initialize", None).await;
        let result = resp.result.unwrap();
        let version = result["serverInfo"]["version"].as_str().unwrap();
        assert!(!version.is_empty());
    }

    #[tokio::test]
    async fn test_handle_notifications_initialized() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, None, "notifications/initialized", None).await;

        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    #[tokio::test]
    async fn test_handle_tools_list() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!(2)), "tools/list", None).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
    }

    #[tokio::test]
    async fn test_handle_tools_list_has_description() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!(3)), "tools/list", None).await;
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert!(!tools[0]["description"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_tools_list_has_input_schema() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!(4)), "tools/list", None).await;
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert!(tools[0]["inputSchema"].is_object());
    }

    #[tokio::test]
    async fn test_handle_tools_call_echo() {
        let kernel = test_kernel();
        let params = json!({
            "name": "echo",
            "arguments": {
                "message": "hello from MCP"
            }
        });

        let resp = handle_request(&kernel, Some(json!(5)), "tools/call", Some(params)).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let content = result["content"].as_array().unwrap();
        assert_eq!(content[0]["text"], "hello from MCP");
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn test_handle_tools_call_missing_name() {
        let kernel = test_kernel();
        let params = json!({ "arguments": {} });

        let resp = handle_request(&kernel, Some(json!(6)), "tools/call", Some(params)).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("name"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_missing_params() {
        let kernel = test_kernel();

        let resp = handle_request(&kernel, Some(json!(7)), "tools/call", None).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn test_handle_tools_call_tool_not_found() {
        let kernel = test_kernel();
        let params = json!({
            "name": "nonexistent_tool",
            "arguments": {}
        });

        let resp = handle_request(&kernel, Some(json!(8)), "tools/call", Some(params)).await;

        // Tool-not-found is returned as a tool result with is_error=true,
        // not as a JSON-RPC error.
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Tool not found"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_no_arguments() {
        let kernel = test_kernel();
        let params = json!({ "name": "echo" });

        let resp = handle_request(&kernel, Some(json!(9)), "tools/call", Some(params)).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        // EchoTool with no message returns "(no message)"
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "(no message)");
    }

    #[tokio::test]
    async fn test_handle_unknown_method() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!(10)), "unknown/method", None).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Method not found"));
    }

    #[test]
    fn test_make_error() {
        let resp = make_error(Some(json!(99)), -32600, "Bad request".to_string());

        assert_eq!(resp.id, Some(json!(99)));
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Bad request");
    }

    #[test]
    fn test_make_error_with_none_id() {
        let resp = make_error(None, -32700, "Parse error".to_string());

        assert!(resp.id.is_none());
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn test_handle_tools_list_empty_registry() {
        let config = Config::default();
        let kernel = ZeptoKernel {
            config: Arc::new(config.clone()),
            provider: None,
            tools: ToolRegistry::new(),
            safety: None,
            metrics: Arc::new(MetricsCollector::new()),
            hooks: Arc::new(HookEngine::new(config.hooks.clone())),
            mcp_clients: vec![],
            ltm: None,
            taint: None,
        };

        let resp = handle_request(&kernel, Some(json!(11)), "tools/list", None).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_handle_request_with_string_id() {
        let kernel = test_kernel();
        let resp = handle_request(&kernel, Some(json!("abc-123")), "initialize", None).await;

        assert_eq!(resp.id, Some(json!("abc-123")));
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_is_notification_true() {
        assert!(is_notification("notifications/initialized"));
        assert!(is_notification("notifications/cancelled"));
        assert!(is_notification("notifications/progress"));
    }

    #[test]
    fn test_is_notification_false() {
        assert!(!is_notification("initialize"));
        assert!(!is_notification("tools/list"));
        assert!(!is_notification("tools/call"));
        assert!(!is_notification(""));
        assert!(!is_notification("notification")); // no trailing slash prefix match
    }
}
