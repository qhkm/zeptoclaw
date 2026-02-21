//! Tools module - Tool definitions and execution for LLM function calling
//!
//! This module provides the infrastructure for defining and executing tools
//! that can be called by LLMs during conversations. Tools allow the AI to
//! perform actions like searching the web, reading files, or interacting
//! with external services.
//!
//! # Overview
//!
//! - `Tool` trait: The interface that all tools must implement
//! - `ToolContext`: Execution context (channel, chat_id, workspace)
//! - `ToolRegistry`: Central registry for managing and executing tools
//!
//! # Built-in Tools
//!
//! - `EchoTool`: Simple echo tool for testing
//! - `ReadFileTool`: Read file contents
//! - `WriteFileTool`: Write content to a file
//! - `ListDirTool`: List directory contents
//! - `EditFileTool`: Edit a file by replacing text
//! - `ShellTool`: Execute shell commands
//! - `WebSearchTool`: Search the web via Brave Search API
//! - `WebFetchTool`: Fetch URL content and extract text
//! - `MessageTool`: Send proactive outbound chat messages
//! - `MemorySearchTool`: Search workspace markdown memory files
//! - `MemoryGetTool`: Read memory files with line windows
//! - `WhatsAppTool`: Send WhatsApp Cloud API messages
//! - `GoogleSheetsTool`: Read and write Google Sheets ranges
//! - `R8rTool`: Execute r8r workflows for deterministic automation
//!
//! # Example
//!
//! ```rust
//! use zeptoclaw::tools::{Tool, ToolContext, ToolRegistry, EchoTool};
//! use zeptoclaw::tools::filesystem::ReadFileTool;
//! use zeptoclaw::tools::shell::ShellTool;
//! use serde_json::json;
//!
//! # tokio_test::block_on(async {
//! // Create a registry and register tools
//! let mut registry = ToolRegistry::new();
//! registry.register(Box::new(EchoTool));
//! registry.register(Box::new(ReadFileTool));
//! registry.register(Box::new(ShellTool::new()));
//!
//! // Execute a tool
//! let result = registry.execute("echo", json!({"message": "Hello!"})).await;
//! assert_eq!(result.unwrap(), "Hello!");
//!
//! // Get tool definitions for LLM
//! let definitions = registry.definitions();
//! assert!(definitions.len() >= 3);
//! # });
//! ```

pub mod approval;
pub mod binary_plugin;
pub mod cron;
pub mod custom;
pub mod delegate;
pub mod filesystem;
pub mod gsheets;
pub mod longterm_memory;
pub mod mcp;
pub mod memory;
pub mod message;
pub mod plugin;
pub mod r8r;
mod registry;
pub mod reminder;
#[cfg(feature = "screenshot")]
pub mod screenshot;
pub mod shell;
pub mod spawn;
pub mod stripe;
mod types;
pub mod web;
pub mod whatsapp;

pub use binary_plugin::BinaryPluginTool;
pub use custom::CustomTool;
pub use delegate::DelegateTool;
pub use gsheets::GoogleSheetsTool;
pub use longterm_memory::LongTermMemoryTool;
pub use memory::{MemoryGetTool, MemorySearchTool};
pub use message::MessageTool;
pub use r8r::R8rTool;
pub use registry::ToolRegistry;
pub use reminder::ReminderTool;
#[cfg(feature = "screenshot")]
pub use screenshot::WebScreenshotTool;
pub use stripe::StripeTool;
pub use types::{Tool, ToolContext};
pub use web::{is_blocked_host, resolve_and_check_host, WebFetchTool, WebSearchTool};
pub use whatsapp::WhatsAppTool;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

/// A simple echo tool for testing purposes.
///
/// This tool echoes back any message provided to it.
/// Useful for testing the tool infrastructure.
///
/// # Example
///
/// ```rust
/// use zeptoclaw::tools::{Tool, ToolContext, EchoTool};
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let tool = EchoTool;
/// let ctx = ToolContext::new();
/// let result = tool.execute(json!({"message": "Hello"}), &ctx).await;
/// assert_eq!(result.unwrap(), "Hello");
/// # });
/// ```
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes back the provided message"
    }

    fn compact_description(&self) -> &str {
        "Echo message"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to echo"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("(no message)");
        Ok(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_echo_tool_name() {
        let tool = EchoTool;
        assert_eq!(tool.name(), "echo");
    }

    #[test]
    fn test_echo_tool_description() {
        let tool = EchoTool;
        assert_eq!(tool.description(), "Echoes back the provided message");
    }

    #[test]
    fn test_echo_tool_parameters() {
        let tool = EchoTool;
        let params = tool.parameters();

        assert!(params.is_object());
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["message"].is_object());
        assert_eq!(params["properties"]["message"]["type"], "string");
    }

    #[tokio::test]
    async fn test_echo_tool_execute() {
        let tool = EchoTool;
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"message": "Hello, World!"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello, World!");
    }

    #[tokio::test]
    async fn test_echo_tool_execute_no_message() {
        let tool = EchoTool;
        let ctx = ToolContext::new();

        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "(no message)");
    }

    #[tokio::test]
    async fn test_echo_tool_execute_empty_string() {
        let tool = EchoTool;
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"message": ""}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[tokio::test]
    async fn test_echo_tool_execute_with_context() {
        let tool = EchoTool;
        let ctx = ToolContext::new()
            .with_channel("telegram", "123")
            .with_workspace("/tmp");

        // Context should be ignored by EchoTool, but execution should still work
        let result = tool.execute(json!({"message": "test"}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test");
    }

    #[tokio::test]
    async fn test_echo_tool_execute_unicode() {
        let tool = EchoTool;
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"message": "Hello World"}), &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello World");
    }

    #[tokio::test]
    async fn test_echo_tool_execute_special_chars() {
        let tool = EchoTool;
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"message": "Line1\nLine2\tTab"}), &ctx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Line1\nLine2\tTab");
    }

    #[test]
    fn test_echo_compact_description() {
        let tool = EchoTool;
        assert_eq!(tool.compact_description(), "Echo message");
        // Verify compact is shorter than full
        assert!(tool.compact_description().len() < tool.description().len());
    }
}
