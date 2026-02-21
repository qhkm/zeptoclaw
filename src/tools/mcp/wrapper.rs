//! MCP tool wrapper — adapts MCP server tools to the ZeptoClaw Tool trait.

use async_trait::async_trait;
use std::sync::Arc;

use super::client::McpClient;
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Wraps a single MCP tool as a ZeptoClaw `Tool` implementation.
pub struct McpToolWrapper {
    /// Tool name as exposed to the agent (prefixed with server name).
    tool_name: String,
    /// Tool description.
    description: String,
    /// JSON schema for input parameters.
    input_schema: serde_json::Value,
    /// The original tool name on the MCP server (without prefix).
    remote_name: String,
    /// Shared reference to the MCP client.
    client: Arc<McpClient>,
}

impl McpToolWrapper {
    /// Create a new wrapper for an MCP tool.
    ///
    /// Tool names are prefixed with the server name: `{server}_{tool}`.
    pub fn new(
        server_name: &str,
        remote_name: &str,
        description: &str,
        input_schema: serde_json::Value,
        client: Arc<McpClient>,
    ) -> Self {
        Self {
            tool_name: format!("{}_{}", server_name, remote_name),
            description: description.to_string(),
            input_schema,
            remote_name: remote_name.to_string(),
            client,
        }
    }

    /// Get the prefixed tool name.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Get the remote (unprefixed) tool name.
    pub fn remote_name(&self) -> &str {
        &self.remote_name
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn compact_description(&self) -> &str {
        self.description()
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::NetworkWrite
    }

    fn parameters(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<ToolOutput, crate::error::ZeptoError> {
        let result = self
            .client
            .call_tool(&self.remote_name, args)
            .await
            .map_err(crate::error::ZeptoError::Mcp)?;

        // Extract text from content blocks
        let text: String = result
            .content
            .iter()
            .filter_map(|block| block.as_text())
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error {
            Ok(ToolOutput::error(if text.is_empty() {
                "MCP tool returned error".to_string()
            } else {
                text
            }))
        } else {
            Ok(ToolOutput::llm_only(if text.is_empty() {
                "(no output)".to_string()
            } else {
                text
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_client() -> Arc<McpClient> {
        Arc::new(McpClient::new("testserver", "http://127.0.0.1:1", 5))
    }

    #[test]
    fn test_wrapper_tool_name_prefixed() {
        let client = make_client();
        let wrapper = McpToolWrapper::new(
            "myserver",
            "read_file",
            "Read a file",
            json!({"type": "object"}),
            client,
        );
        assert_eq!(wrapper.tool_name(), "myserver_read_file");
    }

    #[test]
    fn test_wrapper_description() {
        let client = make_client();
        let wrapper = McpToolWrapper::new("srv", "tool1", "A useful tool", json!({}), client);
        assert_eq!(wrapper.description(), "A useful tool");
    }

    #[test]
    fn test_wrapper_parameters() {
        let client = make_client();
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        });
        let wrapper = McpToolWrapper::new("srv", "tool1", "desc", schema.clone(), client);
        assert_eq!(wrapper.parameters(), schema);
    }

    #[test]
    fn test_wrapper_remote_name() {
        let client = make_client();
        let wrapper = McpToolWrapper::new("prefix", "original_name", "desc", json!({}), client);
        assert_eq!(wrapper.remote_name(), "original_name");
    }

    #[test]
    fn test_wrapper_name_trait_method() {
        let client = make_client();
        let wrapper = McpToolWrapper::new("server", "tool", "desc", json!({}), client);
        // The Tool trait's name() should return the same as tool_name()
        assert_eq!(Tool::name(&wrapper), wrapper.tool_name());
    }

    #[test]
    fn test_wrapper_tool_name_special_chars() {
        let client = make_client();
        let wrapper = McpToolWrapper::new("my-server.v2", "read-file", "desc", json!({}), client);
        assert_eq!(wrapper.tool_name(), "my-server.v2_read-file");
    }

    #[test]
    fn test_wrapper_empty_description() {
        let client = make_client();
        let wrapper = McpToolWrapper::new("srv", "tool", "", json!({}), client);
        assert_eq!(wrapper.description(), "");
    }

    #[test]
    fn test_wrapper_complex_schema() {
        let client = make_client();
        let schema = json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command"},
                "timeout": {"type": "integer", "minimum": 1, "maximum": 600},
                "env": {
                    "type": "object",
                    "additionalProperties": {"type": "string"}
                }
            },
            "required": ["command"]
        });
        let wrapper = McpToolWrapper::new(
            "srv",
            "shell",
            "Run a shell command",
            schema.clone(),
            client,
        );
        assert_eq!(wrapper.parameters(), schema);
        assert_eq!(wrapper.parameters()["properties"]["timeout"]["minimum"], 1);
    }

    #[tokio::test]
    async fn test_wrapper_execute_no_server() {
        let client = make_client();
        let wrapper = McpToolWrapper::new(
            "srv",
            "some_tool",
            "desc",
            json!({"type": "object"}),
            client,
        );
        let ctx = ToolContext::new();
        let result = wrapper.execute(json!({"key": "value"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::error::ZeptoError::Mcp(_)),
            "Expected Mcp error variant, got: {:?}",
            err
        );
    }

    #[test]
    fn test_wrapper_tool_definition_matches() {
        let client = make_client();
        let schema = json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        });
        let wrapper = McpToolWrapper::new(
            "files",
            "read",
            "Read a file from disk",
            schema.clone(),
            client,
        );

        // Verify ToolDefinition can be built from wrapper fields
        let def = crate::providers::ToolDefinition {
            name: wrapper.name().to_string(),
            description: wrapper.description().to_string(),
            parameters: wrapper.parameters(),
        };

        assert_eq!(def.name, "files_read");
        assert_eq!(def.description, "Read a file from disk");
        assert_eq!(def.parameters, schema);
    }
}
