//! Tool registry for ZeptoClaw
//!
//! This module provides the `ToolRegistry` struct for managing and executing tools.
//! Tools can be registered, looked up by name, and executed with context.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;
use tracing::{error, info};

use crate::error::Result;
use crate::providers::ToolDefinition;

use super::{Tool, ToolContext, ToolOutput};

/// A registry that holds and manages tools.
///
/// The registry allows tools to be registered, looked up by name,
/// and executed with proper logging and error handling.
///
/// # Example
///
/// ```rust
/// use zeptoclaw::tools::{ToolRegistry, EchoTool};
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let mut registry = ToolRegistry::new();
/// registry.register(Box::new(EchoTool));
///
/// assert!(registry.has("echo"));
///
/// let result = registry.execute("echo", json!({"message": "hello"})).await;
/// assert!(result.is_ok());
/// # });
/// ```
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::ToolRegistry;
    ///
    /// let registry = ToolRegistry::new();
    /// assert_eq!(registry.names().len(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a new tool in the registry.
    ///
    /// If a tool with the same name already exists, it will be replaced.
    ///
    /// # Arguments
    /// * `tool` - The tool to register
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    /// assert!(registry.has("echo"));
    /// ```
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        info!(tool = %name, "Registering tool");
        self.tools.insert(name, tool);
    }

    /// Get a tool by name.
    ///
    /// # Arguments
    /// * `name` - The name of the tool to retrieve
    ///
    /// # Returns
    /// A reference to the tool if found, or `None` if not found.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    ///
    /// let tool = registry.get("echo");
    /// assert!(tool.is_some());
    /// assert_eq!(tool.unwrap().name(), "echo");
    /// ```
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Execute a tool by name with default context.
    ///
    /// # Arguments
    /// * `name` - The name of the tool to execute
    /// * `args` - The JSON arguments for the tool
    ///
    /// # Returns
    /// A `ToolOutput` with dual-audience content, or an error if execution fails.
    /// Tool-not-found returns `Ok(ToolOutput::error(...))`.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    /// use serde_json::json;
    ///
    /// # tokio_test::block_on(async {
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    ///
    /// let result = registry.execute("echo", json!({"message": "hello"})).await;
    /// assert!(result.is_ok());
    /// assert_eq!(result.unwrap().for_llm, "hello");
    /// # });
    /// ```
    pub async fn execute(&self, name: &str, args: Value) -> Result<ToolOutput> {
        self.execute_with_context(name, args, &ToolContext::default())
            .await
    }

    /// Execute a tool by name with a specific context.
    ///
    /// # Arguments
    /// * `name` - The name of the tool to execute
    /// * `args` - The JSON arguments for the tool
    /// * `ctx` - The execution context
    ///
    /// # Returns
    /// A `ToolOutput` with dual-audience content, or an error if execution fails.
    /// Tool-not-found returns `Ok(ToolOutput::error(...))`.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, ToolContext, EchoTool};
    /// use serde_json::json;
    ///
    /// # tokio_test::block_on(async {
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    ///
    /// let ctx = ToolContext::new().with_channel("telegram", "123");
    /// let result = registry.execute_with_context("echo", json!({"message": "hi"}), &ctx).await;
    /// assert!(result.is_ok());
    /// # });
    /// ```
    pub async fn execute_with_context(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let tool = match self.tools.get(name) {
            Some(t) => t,
            None => {
                return Ok(ToolOutput::error(format!("Tool not found: {}", name)));
            }
        };

        let start = Instant::now();

        match tool.execute(args, ctx).await {
            Ok(output) => {
                info!(
                    tool = name,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Tool executed successfully"
                );
                Ok(output)
            }
            Err(e) => {
                error!(
                    tool = name,
                    error = %e,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Tool execution failed"
                );
                Err(e)
            }
        }
    }

    /// Get all tool definitions for use with LLM providers.
    ///
    /// This returns a list of `ToolDefinition` structs that can be passed
    /// to an LLM provider's chat method.
    ///
    /// # Returns
    /// A vector of tool definitions.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    ///
    /// let definitions = registry.definitions();
    /// assert_eq!(definitions.len(), 1);
    /// assert_eq!(definitions[0].name, "echo");
    /// ```
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect()
    }

    /// Get tool definitions, optionally using compact descriptions.
    ///
    /// When `compact` is true, tools that override `compact_description()`
    /// will use their shorter descriptions, saving tokens for constrained contexts.
    pub fn definitions_with_options(&self, compact: bool) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: if compact {
                    t.compact_description().to_string()
                } else {
                    t.description().to_string()
                },
                parameters: t.parameters(),
            })
            .collect()
    }

    /// Get tool definitions for specific tool names only.
    ///
    /// Returns definitions only for tools whose names are in the provided list.
    /// Tools not found in the registry are silently skipped.
    ///
    /// # Arguments
    /// * `names` - Slice of tool names to include
    ///
    /// # Returns
    /// A vector of tool definitions for the matching tools.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    ///
    /// let defs = registry.definitions_for_tools(&["echo"]);
    /// assert_eq!(defs.len(), 1);
    /// assert_eq!(defs[0].name, "echo");
    ///
    /// let empty = registry.definitions_for_tools(&["nonexistent"]);
    /// assert!(empty.is_empty());
    /// ```
    pub fn definitions_for_tools(&self, names: &[&str]) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|(key, _)| names.contains(&key.as_str()))
            .map(|(_, t)| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect()
    }

    /// Get the names of all registered tools.
    ///
    /// # Returns
    /// A vector of tool names.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// registry.register(Box::new(EchoTool));
    ///
    /// let names = registry.names();
    /// assert!(names.contains(&"echo"));
    /// ```
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a tool exists in the registry.
    ///
    /// # Arguments
    /// * `name` - The name of the tool to check
    ///
    /// # Returns
    /// `true` if the tool exists, `false` otherwise.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// assert!(!registry.has("echo"));
    ///
    /// registry.register(Box::new(EchoTool));
    /// assert!(registry.has("echo"));
    /// ```
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get the number of registered tools.
    ///
    /// # Returns
    /// The number of tools in the registry.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// assert_eq!(registry.len(), 0);
    ///
    /// registry.register(Box::new(EchoTool));
    /// assert_eq!(registry.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty.
    ///
    /// # Returns
    /// `true` if no tools are registered, `false` otherwise.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::tools::{ToolRegistry, EchoTool};
    ///
    /// let mut registry = ToolRegistry::new();
    /// assert!(registry.is_empty());
    ///
    /// registry.register(Box::new(EchoTool));
    /// assert!(!registry.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::EchoTool;
    use serde_json::json;

    #[test]
    fn test_registry_new() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_default() {
        let registry = ToolRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_register() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        assert!(registry.has("echo"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_registry_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let tool = registry.get("echo");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "echo");

        let missing = registry.get("nonexistent");
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_registry_register_and_execute() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        assert!(registry.has("echo"));

        let result = registry.execute("echo", json!({"message": "hello"})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "hello");
    }

    #[tokio::test]
    async fn test_registry_execute_with_context() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let ctx = ToolContext::new()
            .with_channel("telegram", "123456")
            .with_workspace("/tmp/test");

        let result = registry
            .execute_with_context("echo", json!({"message": "world"}), &ctx)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "world");
    }

    #[test]
    fn test_registry_definitions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let definitions = registry.definitions();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "echo");
        assert_eq!(
            definitions[0].description,
            "Echoes back the provided message"
        );
        assert!(definitions[0].parameters.is_object());
    }

    #[test]
    fn test_registry_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let names = registry.names();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"echo"));
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", json!({})).await;

        // Tool-not-found returns Ok(ToolOutput::error(...))
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.is_error);
        assert!(output.for_llm.contains("Tool not found: nonexistent"));
    }

    #[tokio::test]
    async fn test_registry_execute_missing_message() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        // Execute without message argument - should return default
        let result = registry.execute("echo", json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "(no message)");
    }

    #[tokio::test]
    async fn test_registry_execute_null_message() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        // Execute with null message - should return default
        let result = registry.execute("echo", json!({"message": null})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "(no message)");
    }

    #[test]
    fn test_registry_replace_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(EchoTool)); // Register again

        // Should still have only one tool
        assert_eq!(registry.len(), 1);
        assert!(registry.has("echo"));
    }

    #[test]
    fn test_definitions_for_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let defs = registry.definitions_for_tools(&["echo"]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");

        let empty = registry.definitions_for_tools(&["nonexistent"]);
        assert!(empty.is_empty());
    }

    #[test]
    fn test_definitions_for_tools_multiple() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let defs = registry.definitions_for_tools(&["echo", "nonexistent"]);
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_definitions_with_options_normal() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let defs = registry.definitions_with_options(false);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].description, "Echoes back the provided message");
    }

    #[test]
    fn test_definitions_with_options_compact() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let defs = registry.definitions_with_options(true);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].description, "Echo message");
    }
}
