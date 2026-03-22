//! MCP Server — exposes ZeptoClaw tools to MCP-capable clients.
//!
//! Supports two transports:
//! - **Stdio** (default): line-delimited JSON-RPC 2.0 over stdin/stdout
//! - **HTTP** (feature-gated behind `panel`): POST endpoint via axum

pub mod handler;
pub mod stdio;

use std::sync::Arc;

use crate::kernel::ZeptoKernel;

/// MCP server wrapping a booted kernel.
///
/// Provides `start_stdio()` for the default transport and `start_http()` for
/// HTTP-based MCP clients (requires `panel` feature for axum).
pub struct McpServer {
    kernel: Arc<ZeptoKernel>,
}

impl McpServer {
    /// Create a new MCP server from a booted kernel.
    pub fn new(kernel: Arc<ZeptoKernel>) -> Self {
        Self { kernel }
    }

    /// Start the stdio transport (blocks until stdin EOF).
    pub async fn start_stdio(&self) -> anyhow::Result<()> {
        stdio::run_stdio(&self.kernel).await
    }

    /// Start the HTTP transport on the given address.
    ///
    /// The address should be in `host:port` or `:port` format.
    /// Feature-gated behind `panel` since it depends on axum.
    #[cfg(feature = "panel")]
    pub async fn start_http(&self, addr: &str) -> anyhow::Result<()> {
        use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
        use serde_json::Value;
        use tokio::sync::mpsc;
        use tracing::info;

        use crate::tools::mcp::protocol::McpResponse;

        // Parse bind address (support ":3000" shorthand)
        let bind_addr = if addr.starts_with(':') {
            format!("0.0.0.0{addr}")
        } else {
            addr.to_string()
        };

        let kernel = Arc::clone(&self.kernel);

        // Channel pattern: axum handler sends request, background task processes
        // it with the kernel (avoids lifetime issues with axum extractors).
        type ReqMsg = (Value, tokio::sync::oneshot::Sender<McpResponse>);
        let (tx, mut rx) = mpsc::channel::<ReqMsg>(64);

        // Background processor task
        let processor_kernel = Arc::clone(&kernel);
        tokio::spawn(async move {
            while let Some((body, reply_tx)) = rx.recv().await {
                let id = body
                    .get("id")
                    .and_then(|v| if v.is_null() { None } else { Some(v.clone()) });

                // Return Invalid Request (-32600) immediately when "method"
                // is absent or not a string, matching the stdio transport.
                let method = match body.get("method").and_then(|v| v.as_str()) {
                    Some(m) => m.to_string(),
                    None => {
                        let resp = McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id,
                            result: None,
                            error: Some(crate::tools::mcp::protocol::McpError {
                                code: -32600,
                                message: "Invalid request: missing or non-string 'method' field"
                                    .to_string(),
                                data: None,
                            }),
                        };
                        let _ = reply_tx.send(resp);
                        continue;
                    }
                };

                let params = body.get("params").cloned();

                let resp = handler::handle_request(&processor_kernel, id, &method, params).await;
                let _ = reply_tx.send(resp);
            }
        });

        // Axum route handler — accepts raw `String` body instead of `Json<Value>`
        // so we can return a proper JSON-RPC -32700 parse error for malformed JSON
        // (Axum's `Json` extractor would reject it with its own 422 response).
        async fn mcp_handler(
            State(tx): State<mpsc::Sender<ReqMsg>>,
            body: String,
        ) -> axum::response::Response {
            use axum::response::IntoResponse;

            // Parse JSON manually to return JSON-RPC -32700 on failure.
            let body: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(_) => {
                    let resp = McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(crate::tools::mcp::protocol::McpError {
                            code: -32700,
                            message: "Parse error: invalid JSON".to_string(),
                            data: None,
                        }),
                    };
                    return (StatusCode::OK, Json(resp)).into_response();
                }
            };

            // Validate jsonrpc field
            if body.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
                let resp = McpResponse {
                    jsonrpc: "2.0".to_string(),
                    id: body
                        .get("id")
                        .and_then(|v| if v.is_null() { None } else { Some(v.clone()) }),
                    result: None,
                    error: Some(crate::tools::mcp::protocol::McpError {
                        code: -32600,
                        message: "Invalid request: missing jsonrpc 2.0".to_string(),
                        data: None,
                    }),
                };
                return (StatusCode::OK, Json(resp)).into_response();
            }

            // Detect notifications early — if this is a notification, process
            // it but return 204 No Content (JSON-RPC 2.0 forbids replies).
            let is_notification = body
                .get("method")
                .and_then(|v| v.as_str())
                .map(handler::is_notification)
                .unwrap_or(false);

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx.send((body, reply_tx)).await.is_err() {
                let resp = McpResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(crate::tools::mcp::protocol::McpError {
                        code: -32603,
                        message: "Internal error: processor unavailable".to_string(),
                        data: None,
                    }),
                };
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(resp)).into_response();
            }

            // For notifications, consume the reply but return 204 No Content.
            if is_notification {
                let _ = reply_rx.await;
                return StatusCode::NO_CONTENT.into_response();
            }

            match reply_rx.await {
                Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
                Err(_) => {
                    let resp = McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(crate::tools::mcp::protocol::McpError {
                            code: -32603,
                            message: "Internal error: response channel dropped".to_string(),
                            data: None,
                        }),
                    };
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(resp)).into_response()
                }
            }
        }

        let app = Router::new().route("/", post(mcp_handler)).with_state(tx);

        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        info!(addr = %bind_addr, "MCP HTTP server listening");

        axum::serve(listener, app).await?;
        Ok(())
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

    fn test_kernel() -> Arc<ZeptoKernel> {
        let config = Config::default();
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));

        Arc::new(ZeptoKernel {
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
        })
    }

    #[test]
    fn test_mcp_server_construction() {
        let kernel = test_kernel();
        let server = McpServer::new(kernel);
        // Server should be constructible and hold the kernel
        assert!(!server.kernel.tools.is_empty());
    }

    #[test]
    fn test_mcp_server_kernel_access() {
        let kernel = test_kernel();
        let server = McpServer::new(kernel);
        // Verify the kernel's tools are accessible through the server
        let defs = server.kernel.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
    }
}
