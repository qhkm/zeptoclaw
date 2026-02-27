//! MCP client — transport-agnostic JSON-RPC 2.0 client.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;

use super::protocol::*;
use super::transport::{HttpTransport, McpTransport, StdioTransport};

/// MCP client for communicating with MCP servers over any transport.
pub struct McpClient {
    /// Transport layer (HTTP, stdio, etc.).
    transport: Arc<dyn McpTransport>,
    /// Atomic request ID counter.
    next_id: AtomicU64,
    /// Cached tool definitions.
    tools_cache: Arc<RwLock<Option<Vec<McpTool>>>>,
    /// Server name for logging and tool prefixing.
    server_name: String,
}

impl McpClient {
    /// Create a new MCP client with HTTP transport (backward-compatible).
    pub fn new(name: &str, url: &str, timeout_secs: u64) -> Self {
        Self::new_http(name, url, timeout_secs)
    }

    /// Create a new MCP client with HTTP transport.
    pub fn new_http(name: &str, url: &str, timeout_secs: u64) -> Self {
        let transport = Arc::new(HttpTransport::new(url, timeout_secs));
        Self::with_transport(name, transport)
    }

    /// Create a new MCP client with stdio transport (spawns child process).
    pub async fn new_stdio(
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        timeout_secs: u64,
    ) -> Result<Self, String> {
        let transport = Arc::new(StdioTransport::spawn(command, args, env, timeout_secs).await?);
        Ok(Self::with_transport(name, transport))
    }

    /// Create a new MCP client with a custom transport.
    pub fn with_transport(name: &str, transport: Arc<dyn McpTransport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
            tools_cache: Arc::new(RwLock::new(None)),
            server_name: name.to_string(),
        }
    }

    /// Get the next unique request ID.
    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Get the server name.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get the underlying transport type.
    pub fn transport_type(&self) -> &str {
        self.transport.transport_type()
    }

    /// Send a JSON-RPC request via transport and return the response.
    async fn send_request(&self, request: &McpRequest) -> Result<McpResponse, String> {
        self.transport.send(request).await
    }

    /// Send the initialize handshake.
    pub async fn initialize(&self) -> Result<serde_json::Value, String> {
        let params = InitializeParams::default();
        let request = McpRequest::new(
            self.next_request_id(),
            "initialize",
            Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
        );

        let response = self.send_request(&request).await?;
        if let Some(error) = response.error {
            return Err(format!("MCP initialize error: {}", error.message));
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// List available tools (cached after first call).
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        {
            let cache = self.tools_cache.read().await;
            if let Some(ref tools) = *cache {
                return Ok(tools.clone());
            }
        }

        let request = McpRequest::new(self.next_request_id(), "tools/list", None);
        let response = self.send_request(&request).await?;

        if let Some(error) = response.error {
            return Err(format!("MCP tools/list error: {}", error.message));
        }

        let result: ListToolsResult =
            serde_json::from_value(response.result.ok_or("No result in tools/list response")?)
                .map_err(|e| format!("Failed to parse tools list: {}", e))?;

        let tools = result.tools;
        {
            let mut cache = self.tools_cache.write().await;
            *cache = Some(tools.clone());
        }

        Ok(tools)
    }

    /// Call a tool by name with arguments.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult, String> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let request = McpRequest::new(self.next_request_id(), "tools/call", Some(params));
        let response = self.send_request(&request).await?;

        if let Some(error) = response.error {
            return Err(format!("MCP tools/call error: {}", error.message));
        }

        let result: CallToolResult =
            serde_json::from_value(response.result.ok_or("No result in tools/call response")?)
                .map_err(|e| format!("Failed to parse tool call result: {}", e))?;

        Ok(result)
    }

    /// Invalidate the tools cache (force re-fetch on next list_tools).
    pub async fn invalidate_cache(&self) {
        let mut cache = self.tools_cache.write().await;
        *cache = None;
    }

    /// Shut down the transport (kills stdio child process if applicable).
    pub async fn shutdown(&self) -> Result<(), String> {
        self.transport.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = McpClient::new("test-server", "http://localhost:8080", 30);
        assert_eq!(client.server_name(), "test-server");
        assert_eq!(client.transport_type(), "http");
    }

    #[test]
    fn test_client_new_http() {
        let client = McpClient::new_http("test-server", "http://localhost:8080", 30);
        assert_eq!(client.server_name(), "test-server");
        assert_eq!(client.transport_type(), "http");
    }

    #[test]
    fn test_client_new_with_transport() {
        let transport = Arc::new(HttpTransport::new("http://localhost:8080", 30));
        let client = McpClient::with_transport("custom", transport);
        assert_eq!(client.server_name(), "custom");
        assert_eq!(client.transport_type(), "http");
    }

    #[tokio::test]
    async fn test_client_new_stdio_with_cat() {
        let client = McpClient::new_stdio("test-stdio", "cat", &[], &HashMap::new(), 10).await;
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.server_name(), "test-stdio");
        assert_eq!(client.transport_type(), "stdio");
        let _ = client.shutdown().await;
    }

    #[tokio::test]
    async fn test_client_new_stdio_bad_command() {
        let result =
            McpClient::new_stdio("bad", "/nonexistent/binary", &[], &HashMap::new(), 5).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_request_id_increments() {
        let client = McpClient::new("test", "http://localhost:8080", 30);
        let id1 = client.next_request_id();
        let id2 = client.next_request_id();
        let id3 = client.next_request_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[tokio::test]
    async fn test_invalidate_cache() {
        let client = McpClient::new("test", "http://localhost:8080", 30);

        {
            let mut cache = client.tools_cache.write().await;
            *cache = Some(vec![McpTool {
                name: "test_tool".to_string(),
                description: Some("A test tool".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            }]);
        }

        {
            let cache = client.tools_cache.read().await;
            assert!(cache.is_some());
        }

        client.invalidate_cache().await;

        {
            let cache = client.tools_cache.read().await;
            assert!(cache.is_none());
        }
    }

    #[test]
    fn test_client_default_timeout() {
        let _c1 = McpClient::new("fast", "http://localhost:8080", 5);
        let _c2 = McpClient::new("slow", "http://localhost:8080", 120);
        let _c3 = McpClient::new("very-slow", "http://localhost:8080", 600);
    }

    #[test]
    fn test_server_name_accessor() {
        let client = McpClient::new("my-mcp-server", "http://example.com", 30);
        assert_eq!(client.server_name(), "my-mcp-server");
    }

    #[test]
    fn test_transport_type_accessor() {
        let client = McpClient::new("test", "https://mcp.example.com/rpc", 30);
        assert_eq!(client.transport_type(), "http");
    }

    #[tokio::test]
    async fn test_call_tool_no_server() {
        let client = McpClient::new("test", "http://127.0.0.1:1", 5);
        let result = client
            .call_tool("some_tool", serde_json::json!({"key": "value"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("HTTP request failed"),
            "Expected connection error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_initialize_no_server() {
        let client = McpClient::new("test", "http://127.0.0.1:1", 5);
        let result = client.initialize().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("HTTP request failed"),
            "Expected connection error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_list_tools_no_server() {
        let client = McpClient::new("test", "http://127.0.0.1:1", 5);
        let result = client.list_tools().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("HTTP request failed"),
            "Expected connection error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_cache_starts_empty() {
        let client = McpClient::new("test", "http://localhost:8080", 30);
        let cache = client.tools_cache.read().await;
        assert!(cache.is_none(), "Cache should start as None");
    }
}
