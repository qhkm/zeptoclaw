//! Agent delegation tool for multi-agent swarms.
//!
//! The `DelegateTool` creates a temporary `AgentLoop` with a role-specific
//! system prompt and tool whitelist, runs it to completion, and returns
//! the result to the calling (lead) agent.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::agent::{AgentLoop, ContextBuilder};
use crate::bus::{InboundMessage, MessageBus};
use crate::config::Config;
use crate::error::{Result, ZeptoError};
use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
use crate::runtime::NativeRuntime;
use crate::session::{Message, SessionManager};
use crate::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
use crate::tools::memory::{MemoryGetTool, MemorySearchTool};
use crate::tools::message::MessageTool;
use crate::tools::shell::ShellTool;
use crate::tools::web::WebFetchTool;
use crate::tools::EchoTool;

use super::{Tool, ToolContext};

/// Tool to delegate a task to a specialist sub-agent.
///
/// Creates a new `AgentLoop` with a role-specific system prompt and optional
/// tool whitelist, runs it to completion, and returns the result. Sub-agents
/// are prevented from delegating further to avoid recursion.
pub struct DelegateTool {
    config: Config,
    provider: Arc<dyn LLMProvider>,
    bus: Arc<MessageBus>,
}

impl DelegateTool {
    /// Create a new delegate tool.
    ///
    /// # Arguments
    /// * `config` - Agent configuration (cloned for each sub-agent)
    /// * `provider` - Shared LLM provider (wrapped via `ProviderRef`)
    /// * `bus` - Message bus (a fresh bus is created for each sub-agent)
    pub fn new(config: Config, provider: Arc<dyn LLMProvider>, bus: Arc<MessageBus>) -> Self {
        Self {
            config,
            provider,
            bus,
        }
    }

    /// Create a standard set of tools for a sub-agent.
    ///
    /// Always excludes `delegate` and `spawn` to prevent recursion.
    /// If a whitelist is provided, only tools matching those names are included.
    fn create_sub_agent_tools(&self, whitelist: Option<&[String]>) -> Vec<Box<dyn Tool>> {
        let mut all_tools: Vec<Box<dyn Tool>> = vec![
            Box::new(EchoTool),
            Box::new(ReadFileTool),
            Box::new(WriteFileTool),
            Box::new(ListDirTool),
            Box::new(EditFileTool),
            Box::new(ShellTool::with_runtime(Arc::new(NativeRuntime::new()))),
            Box::new(WebFetchTool::new()),
            Box::new(MessageTool::new(self.bus.clone())),
        ];

        // Add memory tools if enabled
        match &self.config.memory.backend {
            crate::config::MemoryBackend::Disabled => {}
            _ => {
                all_tools.push(Box::new(MemorySearchTool::new(self.config.memory.clone())));
                all_tools.push(Box::new(MemoryGetTool::new(self.config.memory.clone())));
            }
        }

        match whitelist {
            Some(names) => all_tools
                .into_iter()
                .filter(|t| names.iter().any(|n| n == t.name()))
                .collect(),
            None => all_tools,
        }
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a task to a specialist sub-agent with a specific role. \
         The sub-agent runs to completion and returns its result. \
         Use this to decompose complex tasks into specialist subtasks."
    }

    fn compact_description(&self) -> &str {
        "Delegate agent"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "role": {
                    "type": "string",
                    "description": "The specialist role (e.g., 'researcher', 'writer', 'analyst')"
                },
                "task": {
                    "type": "string",
                    "description": "The task for the sub-agent to complete"
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tool whitelist. If omitted, uses role preset or all available tools."
                }
            },
            "required": ["role", "task"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        // Block recursion: sub-agents cannot delegate further
        if ctx.channel.as_deref() == Some("delegate") {
            return Err(ZeptoError::Tool(
                "Cannot delegate from within a delegated task (recursion limit)".to_string(),
            ));
        }

        // Check if swarm is enabled
        if !self.config.swarm.enabled {
            return Err(ZeptoError::Tool(
                "Delegation is disabled in configuration".to_string(),
            ));
        }

        let role = args
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required 'role' argument".into()))?;
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required 'task' argument".into()))?;
        let tool_override: Option<Vec<String>> =
            args.get("tools").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let role_lower = role.to_lowercase();
        let role_config = self.config.swarm.roles.get(&role_lower);

        // Build system prompt from role config or generate a default
        let system_prompt = match role_config {
            Some(rc) if !rc.system_prompt.is_empty() => rc.system_prompt.clone(),
            _ => format!(
                "You are a specialist with the role: {}. \
                 Complete the task given to you thoroughly and return your findings. \
                 You can send interim updates to the user via the message tool.",
                role
            ),
        };

        // Determine allowed tools: explicit override > role config > all
        let allowed_tool_names: Option<Vec<String>> = tool_override.or_else(|| {
            role_config
                .filter(|rc| !rc.tools.is_empty())
                .map(|rc| rc.tools.clone())
        });

        info!(role = %role, task_len = task.len(), "Delegating task to sub-agent");

        // Create sub-agent with role-specific context
        let session_manager = SessionManager::new_memory();
        let sub_bus = Arc::new(MessageBus::new());
        let context_builder = ContextBuilder::new().with_system_prompt(&system_prompt);

        let sub_agent = AgentLoop::with_context_builder(
            self.config.clone(),
            session_manager,
            sub_bus,
            context_builder,
        );

        // Set the same LLM provider via the ProviderRef wrapper
        sub_agent
            .set_provider(Box::new(ProviderRef(Arc::clone(&self.provider))))
            .await;

        // Register tools (filtered by whitelist)
        let tools = self.create_sub_agent_tools(allowed_tool_names.as_deref());
        for tool in tools {
            sub_agent.register_tool(tool).await;
        }

        // Create the inbound message for the sub-agent
        let delegate_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let inbound = InboundMessage::new(
            "delegate",
            &format!("delegate:{}", delegate_id),
            &format!("delegate:{}", delegate_id),
            task,
        );

        // Run the sub-agent to completion
        match sub_agent.process_message(&inbound).await {
            Ok(result) => {
                info!(role = %role, result_len = result.len(), "Sub-agent completed");
                Ok(format!("[{}]: {}", role, result))
            }
            Err(e) => {
                warn!(role = %role, error = %e, "Sub-agent failed");
                Err(ZeptoError::Tool(format!(
                    "Sub-agent '{}' failed: {}",
                    role, e
                )))
            }
        }
    }
}

/// Wrapper to convert `Arc<dyn LLMProvider>` into `Box<dyn LLMProvider>`.
///
/// Since `set_provider()` takes `Box<dyn LLMProvider>`, we need this thin wrapper
/// to share the same provider instance via Arc without cloning the provider itself.
struct ProviderRef(Arc<dyn LLMProvider>);

#[async_trait]
impl LLMProvider for ProviderRef {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn default_model(&self) -> &str {
        self.0.default_model()
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        self.0.chat(messages, tools, model, options).await
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> crate::error::Result<tokio::sync::mpsc::Receiver<crate::providers::StreamEvent>> {
        self.0.chat_stream(messages, tools, model, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helper to create a DelegateTool for testing
    fn test_delegate_tool(swarm_enabled: bool) -> DelegateTool {
        let mut config = Config::default();
        config.swarm.enabled = swarm_enabled;
        let bus = Arc::new(MessageBus::new());
        let provider: Arc<dyn LLMProvider> =
            Arc::new(crate::providers::claude::ClaudeProvider::new("fake-key"));

        DelegateTool::new(config, provider, bus)
    }

    #[tokio::test]
    async fn test_delegate_blocked_from_subagent() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("delegate", "sub-123");

        let result = tool
            .execute(json!({"role": "test", "task": "hello"}), &ctx)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("recursion"),
            "Expected recursion error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_delegate_requires_role() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool.execute(json!({"task": "hello"}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("role"),
            "Expected role error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_delegate_requires_task() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool.execute(json!({"role": "test"}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("task"),
            "Expected task error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_delegate_disabled_in_config() {
        let tool = test_delegate_tool(false);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool
            .execute(json!({"role": "test", "task": "hello"}), &ctx)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("disabled"),
            "Expected disabled error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_delegate_tool_name() {
        let tool = test_delegate_tool(true);
        assert_eq!(tool.name(), "delegate");
    }

    #[test]
    fn test_delegate_tool_parameters() {
        let tool = test_delegate_tool(true);
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["role"].is_object());
        assert!(params["properties"]["task"].is_object());
        assert!(params["properties"]["tools"].is_object());
    }

    #[test]
    fn test_create_sub_agent_tools_no_whitelist() {
        let tool = test_delegate_tool(true);
        let tools = tool.create_sub_agent_tools(None);
        // Should have basic tools (echo, read, write, list, edit, shell, web_fetch, message)
        // plus memory tools (memory_search, memory_get) since default config enables builtin memory
        assert!(tools.len() >= 8);
        // Should NOT include delegate or spawn
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(!names.contains(&"delegate"));
        assert!(!names.contains(&"spawn"));
    }

    #[test]
    fn test_create_sub_agent_tools_with_whitelist() {
        let tool = test_delegate_tool(true);
        let whitelist = vec!["echo".to_string(), "read_file".to_string()];
        let tools = tool.create_sub_agent_tools(Some(&whitelist));
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"read_file"));
    }
}
