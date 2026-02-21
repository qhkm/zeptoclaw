//! Tool types for ZeptoClaw
//!
//! This module defines the core types for tool execution, including the `Tool` trait
//! that all tools must implement, and the `ToolContext` struct that provides
//! execution context to tools.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// Category for agent mode enforcement.
///
/// Each tool is assigned a category that determines whether it is allowed,
/// requires approval, or is blocked under a given agent mode (Observer,
/// Assistant, Autonomous).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Read-only filesystem operations (read, list, glob).
    FilesystemRead,
    /// Write/modify filesystem operations (write, edit, delete).
    FilesystemWrite,
    /// Read-only network operations (web search, fetch).
    NetworkRead,
    /// Network operations that modify external state (HTTP POST, API calls).
    NetworkWrite,
    /// Shell command execution and process spawning.
    Shell,
    /// Hardware/peripheral operations (USB, serial, GPIO).
    Hardware,
    /// Memory read/write operations (workspace memory, long-term memory).
    Memory,
    /// Messaging operations (send messages via channels).
    Messaging,
    /// Destructive or high-risk operations (cron delete, etc.).
    Destructive,
}

impl ToolCategory {
    /// Return an array of all category variants.
    pub fn all() -> [ToolCategory; 9] {
        [
            ToolCategory::FilesystemRead,
            ToolCategory::FilesystemWrite,
            ToolCategory::NetworkRead,
            ToolCategory::NetworkWrite,
            ToolCategory::Shell,
            ToolCategory::Hardware,
            ToolCategory::Memory,
            ToolCategory::Messaging,
            ToolCategory::Destructive,
        ]
    }
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FilesystemRead => write!(f, "filesystem_read"),
            Self::FilesystemWrite => write!(f, "filesystem_write"),
            Self::NetworkRead => write!(f, "network_read"),
            Self::NetworkWrite => write!(f, "network_write"),
            Self::Shell => write!(f, "shell"),
            Self::Hardware => write!(f, "hardware"),
            Self::Memory => write!(f, "memory"),
            Self::Messaging => write!(f, "messaging"),
            Self::Destructive => write!(f, "destructive"),
        }
    }
}

/// Dual-audience tool result.
///
/// Separates what the LLM sees (`for_llm`) from what the user sees (`for_user`).
/// Tools that should be silent to the user (file reads, memory ops) set `for_user: None`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Content sent to the LLM as the tool result. Always required.
    pub for_llm: String,
    /// Content sent to the user. `None` = silent (user sees nothing).
    pub for_user: Option<String>,
    /// Whether this result represents an error condition.
    pub is_error: bool,
    /// Whether the tool is running asynchronously (result will arrive later).
    /// TODO: wire into agent loop to skip hooks and metrics for background tasks.
    pub is_async: bool,
}

impl ToolOutput {
    /// LLM-only result. User sees nothing.
    pub fn llm_only(content: impl Into<String>) -> Self {
        Self {
            for_llm: content.into(),
            for_user: None,
            is_error: false,
            is_async: false,
        }
    }

    /// Both LLM and user see the same content.
    pub fn user_visible(content: impl Into<String>) -> Self {
        let s = content.into();
        Self {
            for_llm: s.clone(),
            for_user: Some(s),
            is_error: false,
            is_async: false,
        }
    }

    /// Error result. LLM sees the error; user sees nothing by default.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            for_llm: content.into(),
            for_user: None,
            is_error: true,
            is_async: false,
        }
    }

    /// Async task launched. LLM is informed; user sees nothing until callback.
    pub fn async_task(content: impl Into<String>) -> Self {
        Self {
            for_llm: content.into(),
            for_user: None,
            is_error: false,
            is_async: true,
        }
    }

    /// Different content for LLM vs user.
    ///
    /// Use when the LLM needs verbose context (JSON blob, raw data) but the user
    /// should see a concise summary. Currently unused — wire up in WebFetchTool
    /// to give LLM full HTML and user a short excerpt.
    pub fn split(for_llm: impl Into<String>, for_user: impl Into<String>) -> Self {
        Self {
            for_llm: for_llm.into(),
            for_user: Some(for_user.into()),
            is_error: false,
            is_async: false,
        }
    }
}

/// Trait that all tools must implement.
///
/// Tools are executable functions that the LLM can call to perform actions
/// like searching the web, reading files, or interacting with external services.
///
/// # Example
///
/// ```rust
/// use async_trait::async_trait;
/// use serde_json::Value;
/// use zeptoclaw::tools::{Tool, ToolContext, ToolOutput};
/// use zeptoclaw::error::Result;
///
/// struct MyTool;
///
/// #[async_trait]
/// impl Tool for MyTool {
///     fn name(&self) -> &str { "my_tool" }
///     fn description(&self) -> &str { "Does something useful" }
///     fn parameters(&self) -> Value {
///         serde_json::json!({
///             "type": "object",
///             "properties": {},
///             "required": []
///         })
///     }
///     async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
///         Ok(ToolOutput::llm_only("Done!"))
///     }
/// }
/// ```
#[async_trait]
pub trait Tool: Send + Sync {
    /// Get the tool name.
    ///
    /// This name is used to identify the tool when the LLM requests it.
    /// It should be unique within a registry.
    fn name(&self) -> &str;

    /// Get the tool description.
    ///
    /// This description is sent to the LLM to help it understand
    /// when and how to use the tool.
    fn description(&self) -> &str;

    /// Get the JSON schema for the tool's parameters.
    ///
    /// This schema describes what arguments the tool accepts.
    /// It follows the JSON Schema specification.
    fn parameters(&self) -> Value;

    /// Execute the tool with the given arguments.
    ///
    /// # Arguments
    /// * `args` - The JSON arguments passed by the LLM
    /// * `ctx` - The execution context (channel, chat_id, workspace, etc.)
    ///
    /// # Returns
    /// A `ToolOutput` with dual-audience content (LLM vs user).
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput>;

    /// Get a compact (shorter) description for token-constrained environments.
    ///
    /// Defaults to the full description. Override in individual tools for
    /// shorter versions (~40% token savings).
    fn compact_description(&self) -> &str {
        self.description()
    }

    /// Tool category for agent mode enforcement.
    ///
    /// The agent mode system uses this to determine whether a tool is allowed,
    /// requires approval, or is blocked under the current mode. **Every tool
    /// implementation MUST override this** to return the correct category.
    ///
    /// Defaults to `ToolCategory::Shell` (fail-closed). If a tool forgets to
    /// override this, it will require approval in Assistant mode and be blocked
    /// in Observer mode — a safe default that prevents accidental over-permission.
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }
}

/// Context provided to tools during execution.
///
/// This struct contains information about the execution environment,
/// such as which channel/chat the request came from and the workspace.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    /// The channel name (e.g., "telegram", "discord", "cli")
    pub channel: Option<String>,
    /// The chat/conversation ID within the channel
    pub chat_id: Option<String>,
    /// The workspace directory for file operations
    pub workspace: Option<String>,
}

impl ToolContext {
    /// Create a new empty tool context.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::ToolContext;
    ///
    /// let ctx = ToolContext::new();
    /// assert!(ctx.channel.is_none());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the channel and chat ID.
    ///
    /// # Arguments
    /// * `channel` - The channel name (e.g., "telegram")
    /// * `chat_id` - The chat/conversation ID
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::ToolContext;
    ///
    /// let ctx = ToolContext::new()
    ///     .with_channel("telegram", "123456");
    /// assert_eq!(ctx.channel.as_deref(), Some("telegram"));
    /// assert_eq!(ctx.chat_id.as_deref(), Some("123456"));
    /// ```
    pub fn with_channel(mut self, channel: &str, chat_id: &str) -> Self {
        self.channel = Some(channel.to_string());
        self.chat_id = Some(chat_id.to_string());
        self
    }

    /// Set the workspace directory.
    ///
    /// # Arguments
    /// * `workspace` - The workspace directory path
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::ToolContext;
    ///
    /// let ctx = ToolContext::new()
    ///     .with_workspace("/home/user/project");
    /// assert_eq!(ctx.workspace.as_deref(), Some("/home/user/project"));
    /// ```
    pub fn with_workspace(mut self, workspace: &str) -> Self {
        self.workspace = Some(workspace.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_context_new() {
        let ctx = ToolContext::new();
        assert!(ctx.channel.is_none());
        assert!(ctx.chat_id.is_none());
        assert!(ctx.workspace.is_none());
    }

    #[test]
    fn test_tool_context_default() {
        let ctx = ToolContext::default();
        assert!(ctx.channel.is_none());
        assert!(ctx.chat_id.is_none());
        assert!(ctx.workspace.is_none());
    }

    #[test]
    fn test_tool_context_with_channel() {
        let ctx = ToolContext::new().with_channel("telegram", "123456");
        assert_eq!(ctx.channel.as_deref(), Some("telegram"));
        assert_eq!(ctx.chat_id.as_deref(), Some("123456"));
        assert!(ctx.workspace.is_none());
    }

    #[test]
    fn test_tool_context_with_workspace() {
        let ctx = ToolContext::new().with_workspace("/home/user/project");
        assert!(ctx.channel.is_none());
        assert!(ctx.chat_id.is_none());
        assert_eq!(ctx.workspace.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn test_tool_context_builder_chain() {
        let ctx = ToolContext::new()
            .with_channel("discord", "abc123")
            .with_workspace("/tmp/workspace");

        assert_eq!(ctx.channel.as_deref(), Some("discord"));
        assert_eq!(ctx.chat_id.as_deref(), Some("abc123"));
        assert_eq!(ctx.workspace.as_deref(), Some("/tmp/workspace"));
    }

    #[test]
    fn test_tool_context_debug() {
        let ctx = ToolContext::new().with_channel("cli", "test");
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("ToolContext"));
        assert!(debug_str.contains("cli"));
    }

    #[test]
    fn test_tool_context_clone() {
        let ctx1 = ToolContext::new()
            .with_channel("telegram", "123")
            .with_workspace("/test");
        let ctx2 = ctx1.clone();

        assert_eq!(ctx1.channel, ctx2.channel);
        assert_eq!(ctx1.chat_id, ctx2.chat_id);
        assert_eq!(ctx1.workspace, ctx2.workspace);
    }

    #[test]
    fn test_tool_category_display() {
        assert_eq!(ToolCategory::FilesystemRead.to_string(), "filesystem_read");
        assert_eq!(ToolCategory::Shell.to_string(), "shell");
        assert_eq!(ToolCategory::Hardware.to_string(), "hardware");
        assert_eq!(ToolCategory::Destructive.to_string(), "destructive");
    }

    #[test]
    fn test_tool_category_serde_roundtrip() {
        let cat = ToolCategory::NetworkWrite;
        let json = serde_json::to_string(&cat).unwrap();
        assert_eq!(json, "\"network_write\"");
        let back: ToolCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cat);
    }

    #[test]
    fn test_tool_category_all_variants() {
        use std::collections::HashSet;
        let all = vec![
            ToolCategory::FilesystemRead,
            ToolCategory::FilesystemWrite,
            ToolCategory::NetworkRead,
            ToolCategory::NetworkWrite,
            ToolCategory::Shell,
            ToolCategory::Hardware,
            ToolCategory::Memory,
            ToolCategory::Messaging,
            ToolCategory::Destructive,
        ];
        let set: HashSet<_> = all.iter().collect();
        assert_eq!(set.len(), 9);
    }

    #[test]
    fn test_tool_default_category() {
        // EchoTool uses the default category() implementation (fail-closed: Shell)
        let tool = super::super::EchoTool;
        assert_eq!(tool.category(), ToolCategory::Shell);
    }

    #[test]
    fn test_tool_output_llm_only() {
        let out = ToolOutput::llm_only("internal");
        assert_eq!(out.for_llm, "internal");
        assert!(out.for_user.is_none());
        assert!(!out.is_error);
        assert!(!out.is_async);
    }

    #[test]
    fn test_tool_output_user_visible() {
        let out = ToolOutput::user_visible("hello");
        assert_eq!(out.for_llm, "hello");
        assert_eq!(out.for_user.as_deref(), Some("hello"));
    }

    #[test]
    fn test_tool_output_error() {
        let out = ToolOutput::error("something broke");
        assert!(out.is_error);
        assert!(out.for_user.is_none());
    }

    #[test]
    fn test_tool_output_async_task() {
        let out = ToolOutput::async_task("running in background");
        assert!(out.is_async);
        assert!(!out.is_error);
    }

    #[test]
    fn test_tool_output_split() {
        let out = ToolOutput::split("llm sees this", "user sees that");
        assert_eq!(out.for_llm, "llm sees this");
        assert_eq!(out.for_user.as_deref(), Some("user sees that"));
    }
}
