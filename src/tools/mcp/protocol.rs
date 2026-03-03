//! MCP (Model Context Protocol) JSON-RPC 2.0 types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl McpRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response.
///
/// `id` is a `serde_json::Value` to preserve the original type sent by the
/// client (number, string, or null) as required by the JSON-RPC 2.0 spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

impl McpResponse {
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    pub fn error_message(&self) -> Option<String> {
        self.error.as_ref().map(|e| e.message.clone())
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// MCP tool definition returned by tools/list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    /// Tool input schema — supports both camelCase and snake_case.
    #[serde(alias = "input_schema", default)]
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Content block returned by tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource {
        uri: String,
        #[serde(rename = "mimeType")]
        mime_type: Option<String>,
        text: Option<String>,
    },
}

impl ContentBlock {
    /// Extract text content, if any.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            ContentBlock::Resource { text, .. } => text.as_deref(),
            ContentBlock::Image { .. } => None,
        }
    }
}

/// Result of tools/list method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
}

/// Result of tools/call method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    #[serde(rename = "isError")]
    pub is_error: bool,
}

/// Initialize request params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: HashMap<String, serde_json::Value>,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

/// Client info sent during initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: "2024-11-05".to_string(),
            capabilities: HashMap::new(),
            client_info: ClientInfo {
                name: "zeptoclaw".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mcp_request_serialization() {
        let req = McpRequest::new(1, "tools/list", None);
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "tools/list");
        assert!(json.get("params").is_none());
    }

    #[test]
    fn test_mcp_request_with_params() {
        let params = json!({"name": "shell", "arguments": {"command": "ls"}});
        let req = McpRequest::new(42, "tools/call", Some(params.clone()));
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 42);
        assert_eq!(json["method"], "tools/call");
        assert_eq!(json["params"], params);
    }

    #[test]
    fn test_mcp_response_success() {
        let raw = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"tools": []}
        });
        let resp: McpResponse = serde_json::from_value(raw).unwrap();

        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_mcp_response_error() {
        let raw = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        });
        let resp: McpResponse = serde_json::from_value(raw).unwrap();

        assert_eq!(resp.id, Some(json!(2)));
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_mcp_response_is_error() {
        let success = McpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            result: Some(json!({})),
            error: None,
        };
        assert!(!success.is_error());
        assert!(success.error_message().is_none());

        let failure = McpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            result: None,
            error: Some(McpError {
                code: -32600,
                message: "Invalid request".to_string(),
                data: None,
            }),
        };
        assert!(failure.is_error());
        assert_eq!(failure.error_message(), Some("Invalid request".to_string()));
    }

    #[test]
    fn test_mcp_tool_deserialization() {
        let raw = json!({
            "name": "read_file",
            "description": "Read a file",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }
        });
        let tool: McpTool = serde_json::from_value(raw).unwrap();

        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description, Some("Read a file".to_string()));
        assert_eq!(tool.input_schema["type"], "object");
        assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
    }

    #[test]
    fn test_mcp_tool_schema_alias() {
        let raw = json!({
            "name": "shell",
            "description": "Run a command",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                }
            }
        });
        let tool: McpTool = serde_json::from_value(raw).unwrap();

        assert_eq!(tool.name, "shell");
        assert_eq!(tool.input_schema["type"], "object");
    }

    #[test]
    fn test_mcp_tool_default_empty_schema() {
        let raw = json!({
            "name": "ping",
            "description": null
        });
        let tool: McpTool = serde_json::from_value(raw).unwrap();

        assert_eq!(tool.name, "ping");
        assert!(tool.description.is_none());
        assert!(tool.input_schema.is_null());
    }

    #[test]
    fn test_content_block_text() {
        let raw = json!({"type": "text", "text": "Hello, world!"});
        let block: ContentBlock = serde_json::from_value(raw).unwrap();

        assert_eq!(block.as_text(), Some("Hello, world!"));
    }

    #[test]
    fn test_content_block_image() {
        let raw = json!({
            "type": "image",
            "data": "iVBORw0KGgo=",
            "mimeType": "image/png"
        });
        let block: ContentBlock = serde_json::from_value(raw).unwrap();

        assert!(block.as_text().is_none());
        if let ContentBlock::Image { data, mime_type } = &block {
            assert_eq!(data, "iVBORw0KGgo=");
            assert_eq!(mime_type, "image/png");
        } else {
            panic!("Expected Image variant");
        }
    }

    #[test]
    fn test_content_block_resource() {
        let raw = json!({
            "type": "resource",
            "uri": "file:///tmp/out.txt",
            "mimeType": "text/plain",
            "text": "file contents here"
        });
        let block: ContentBlock = serde_json::from_value(raw).unwrap();

        assert_eq!(block.as_text(), Some("file contents here"));
        if let ContentBlock::Resource {
            uri,
            mime_type,
            text,
        } = &block
        {
            assert_eq!(uri, "file:///tmp/out.txt");
            assert_eq!(mime_type.as_deref(), Some("text/plain"));
            assert_eq!(text.as_deref(), Some("file contents here"));
        } else {
            panic!("Expected Resource variant");
        }
    }

    #[test]
    fn test_call_tool_result() {
        let raw = json!({
            "content": [
                {"type": "text", "text": "line 1"},
                {"type": "text", "text": "line 2"}
            ],
            "isError": false
        });
        let result: CallToolResult = serde_json::from_value(raw).unwrap();

        assert_eq!(result.content.len(), 2);
        assert!(!result.is_error);
        assert_eq!(result.content[0].as_text(), Some("line 1"));
        assert_eq!(result.content[1].as_text(), Some("line 2"));
    }

    #[test]
    fn test_list_tools_result() {
        let raw = json!({
            "tools": [
                {
                    "name": "echo",
                    "description": "Echo back",
                    "inputSchema": {"type": "object"}
                },
                {
                    "name": "ping",
                    "description": null
                }
            ]
        });
        let result: ListToolsResult = serde_json::from_value(raw).unwrap();

        assert_eq!(result.tools.len(), 2);
        assert_eq!(result.tools[0].name, "echo");
        assert_eq!(result.tools[1].name, "ping");
        assert!(result.tools[1].description.is_none());
    }

    #[test]
    fn test_initialize_params_default() {
        let params = InitializeParams::default();

        assert_eq!(params.protocol_version, "2024-11-05");
        assert!(params.capabilities.is_empty());
        assert_eq!(params.client_info.name, "zeptoclaw");
        assert!(!params.client_info.version.is_empty());
    }
}
