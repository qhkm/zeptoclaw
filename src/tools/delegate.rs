//! Agent delegation tool for multi-agent swarms.
//!
//! The `DelegateTool` creates a temporary `AgentLoop` with a role-specific
//! system prompt and tool whitelist, runs it to completion, and returns
//! the result to the calling (lead) agent.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::agent::{AgentLoop, ContextBuilder, SwarmScratchpad};
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

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Tool to delegate a task to a specialist sub-agent.
///
/// Creates a new `AgentLoop` with a role-specific system prompt and optional
/// tool whitelist, runs it to completion, and returns the result. Sub-agents
/// are prevented from delegating further to avoid recursion.
///
/// Supports two actions:
/// - `run` (default) — delegates a single task to one sub-agent.
/// - `aggregate` — fans out multiple tasks (each with its own role) and merges
///   the results using a configurable merge strategy.
///
/// Concurrency is bounded by a `Semaphore` whose capacity comes from
/// `config.swarm.max_concurrent`.
pub struct DelegateTool {
    config: Config,
    provider: Arc<dyn LLMProvider>,
    bus: Arc<MessageBus>,
    /// Semaphore limiting concurrent sub-agent executions.
    semaphore: Arc<Semaphore>,
    /// Shared scratchpad for passing context between sub-agents in a swarm session.
    scratchpad: SwarmScratchpad,
}

impl DelegateTool {
    /// Create a new delegate tool.
    ///
    /// # Arguments
    /// * `config` - Agent configuration (cloned for each sub-agent)
    /// * `provider` - Shared LLM provider (wrapped via `ProviderRef`)
    /// * `bus` - Message bus (a fresh bus is created for each sub-agent)
    pub fn new(config: Config, provider: Arc<dyn LLMProvider>, bus: Arc<MessageBus>) -> Self {
        let max_concurrent = config.swarm.max_concurrent as usize;
        // Guard against a zero-capacity semaphore which would deadlock every acquire.
        let capacity = if max_concurrent == 0 {
            1
        } else {
            max_concurrent
        };
        let semaphore = Arc::new(Semaphore::new(capacity));
        Self {
            config,
            provider,
            bus,
            semaphore,
            scratchpad: SwarmScratchpad::new(),
        }
    }

    /// Create a delegate tool with an explicit semaphore.
    ///
    /// Primarily useful in tests where callers want to control the semaphore
    /// capacity independently of `config.swarm.max_concurrent`.
    pub fn with_semaphore(
        config: Config,
        provider: Arc<dyn LLMProvider>,
        bus: Arc<MessageBus>,
        semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            config,
            provider,
            bus,
            semaphore,
            scratchpad: SwarmScratchpad::new(),
        }
    }

    /// Return a reference to the shared swarm scratchpad.
    ///
    /// Primarily useful in tests to inspect scratchpad state after delegating.
    pub fn scratchpad(&self) -> &SwarmScratchpad {
        &self.scratchpad
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

    /// Run a single delegated sub-agent and return its raw result string.
    ///
    /// This is the shared implementation used by both the `run` and `aggregate`
    /// actions. It acquires a semaphore permit before creating the sub-agent,
    /// so concurrent calls are bounded by `config.swarm.max_concurrent`.
    ///
    /// The returned string does **not** include the `[role]:` prefix; callers
    /// are responsible for any formatting.
    async fn run_single_delegate(
        &self,
        role: &str,
        task: &str,
        tools: Option<&[String]>,
        _ctx: &ToolContext,
    ) -> Result<String> {
        let role_lower = role.to_lowercase();
        let role_config = self.config.swarm.roles.get(&role_lower);

        // Build system prompt from role config or generate a default
        let mut system_prompt = match role_config {
            Some(rc) if !rc.system_prompt.is_empty() => rc.system_prompt.clone(),
            _ => format!(
                "You are a specialist with the role: {}. \
                 Complete the task given to you thoroughly and return your findings. \
                 You can send interim updates to the user via the message tool.",
                role
            ),
        };

        // Inject previous agent outputs from the scratchpad so this sub-agent
        // can build on what earlier agents produced.
        if let Some(context) = self.scratchpad.format_for_prompt().await {
            system_prompt = format!("{}\n\n{}", system_prompt, context);
        }

        // Determine allowed tools: explicit override > role config > all
        let allowed_tool_names: Option<Vec<String>> = tools.map(|t| t.to_vec()).or_else(|| {
            role_config
                .filter(|rc| !rc.tools.is_empty())
                .map(|rc| rc.tools.clone())
        });

        info!(role = %role, task_len = task.len(), "Delegating task to sub-agent");

        // Acquire semaphore permit before creating the sub-agent.
        // The permit is held for the duration of this function and released
        // automatically when `_permit` drops at the end of the scope.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| ZeptoError::Tool("Swarm semaphore closed".into()))?;

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
        let sub_tools = self.create_sub_agent_tools(allowed_tool_names.as_deref());
        for tool in sub_tools {
            sub_agent.register_tool(tool).await;
        }

        // Create the inbound message for the sub-agent
        let delegate_id = uuid::Uuid::new_v4()
            .to_string()
            .chars()
            .take(8)
            .collect::<String>();
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
                Ok(result)
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

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate a task to a specialist sub-agent with a specific role. \
         The sub-agent runs to completion and returns its result. \
         Use this to decompose complex tasks into specialist subtasks. \
         Use action='aggregate' with a 'tasks' array to fan out multiple tasks \
         and collect their results."
    }

    fn compact_description(&self) -> &str {
        "Delegate agent"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run", "aggregate"],
                    "description": "Action to perform. 'run' (default) delegates a single task. \
                                    'aggregate' fans out multiple tasks and merges results."
                },
                "role": {
                    "type": "string",
                    "description": "The specialist role (e.g., 'researcher', 'writer', 'analyst'). \
                                    Required for action='run'."
                },
                "task": {
                    "type": "string",
                    "description": "The task for the sub-agent to complete. Required for action='run'."
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tool whitelist for action='run'. \
                                    If omitted, uses role preset or all available tools."
                },
                "tasks": {
                    "type": "array",
                    "description": "For action='aggregate': array of task specs, \
                                    each with 'role', 'task', and optional 'tools'.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "role": { "type": "string" },
                            "task": { "type": "string" },
                            "tools": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["role", "task"]
                    }
                },
                "merge_strategy": {
                    "type": "string",
                    "enum": ["concatenate", "summarize"],
                    "description": "For action='aggregate': how to merge results. \
                                    'concatenate' (default) joins results as '[Role]: result'. \
                                    'summarize' produces a structured markdown document."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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

        // Default action is "run" for backwards compatibility.
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("run");

        match action {
            "run" => {
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

                let result = self
                    .run_single_delegate(role, task, tool_override.as_deref(), ctx)
                    .await?;
                // Write the result to the scratchpad so subsequent sub-agents can
                // see what this agent produced.
                self.scratchpad.write(role, &result).await;
                // Preserve the original output format: "[role]: result"
                Ok(ToolOutput::user_visible(format!("[{}]: {}", role, result)))
            }

            "aggregate" => {
                let tasks = args
                    .get("tasks")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ZeptoError::Tool("'aggregate' requires 'tasks' array".into()))?;

                let mut results: Vec<(String, String)> = Vec::new();
                for task_spec in tasks {
                    let role = task_spec
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("assistant");
                    let task_text =
                        task_spec
                            .get("task")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                ZeptoError::Tool(
                                    "Each task in aggregate must have 'task' field".into(),
                                )
                            })?;
                    let tools: Option<Vec<String>> =
                        task_spec.get("tools").and_then(Value::as_array).map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        });

                    let result = self
                        .run_single_delegate(role, task_text, tools.as_deref(), ctx)
                        .await?;
                    // Write each result to the scratchpad so subsequent sub-agents
                    // in this aggregate batch can see prior outputs.
                    self.scratchpad.write(role, &result).await;
                    results.push((role.to_string(), result));
                }

                let merge = args
                    .get("merge_strategy")
                    .and_then(Value::as_str)
                    .unwrap_or("concatenate");
                Ok(ToolOutput::user_visible(format_results(&results, merge)))
            }

            other => Err(ZeptoError::Tool(format!(
                "Unknown action '{}'. Valid actions are: run, aggregate",
                other
            ))),
        }
    }
}

/// Merge aggregated sub-agent results using the specified strategy.
///
/// - `"concatenate"` (default) — joins each result as `[Role]: result` separated
///   by blank lines.
/// - `"summarize"` — produces a structured markdown document with `##`/`###`
///   headings. (A real LLM summarization call can be added in a future iteration.)
/// - any other value falls back to `"concatenate"`.
fn format_results(results: &[(String, String)], strategy: &str) -> String {
    match strategy {
        "summarize" => {
            let mut out = String::from("## Aggregated Results\n\n");
            for (role, result) in results {
                out.push_str(&format!("### {}\n{}\n\n", role, result));
            }
            out
        }
        _ => {
            // concatenate (default)
            results
                .iter()
                .map(|(role, result)| format!("[{}]: {}", role, result))
                .collect::<Vec<_>>()
                .join("\n\n")
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

    // -------------------------------------------------------------------------
    // Existing tests (preserved verbatim)
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // Task 15: Semaphore tests
    // -------------------------------------------------------------------------

    /// Default SwarmConfig.max_concurrent is 3, so the semaphore should start
    /// with 3 available permits.
    #[test]
    fn test_semaphore_default_capacity() {
        let config = Config::default();
        assert_eq!(config.swarm.max_concurrent, 3);
        let bus = Arc::new(MessageBus::new());
        let provider: Arc<dyn LLMProvider> =
            Arc::new(crate::providers::claude::ClaudeProvider::new("fake-key"));
        let tool = DelegateTool::new(config, provider, bus);
        assert_eq!(tool.semaphore.available_permits(), 3);
    }

    /// Verify that once all permits are taken, `try_acquire` fails (the second
    /// concurrent call would block / be queued).
    #[tokio::test]
    async fn test_semaphore_limits_concurrency() {
        let sem = Arc::new(Semaphore::new(1));
        // Grab the single available permit.
        let _permit = sem.acquire().await.unwrap();
        // A non-blocking attempt to grab another permit must fail.
        assert!(
            sem.try_acquire().is_err(),
            "Semaphore should be exhausted after one permit is held"
        );
    }

    /// A zero max_concurrent value must not produce a zero-capacity semaphore
    /// (which would deadlock every acquire). We clamp it to at least 1.
    #[test]
    fn test_semaphore_zero_max_concurrent_defaults_to_one() {
        let mut config = Config::default();
        config.swarm.max_concurrent = 0;
        let bus = Arc::new(MessageBus::new());
        let provider: Arc<dyn LLMProvider> =
            Arc::new(crate::providers::claude::ClaudeProvider::new("fake-key"));
        let tool = DelegateTool::new(config, provider, bus);
        assert!(
            tool.semaphore.available_permits() >= 1,
            "Zero max_concurrent should clamp to at least 1 permit, got {}",
            tool.semaphore.available_permits()
        );
    }

    // -------------------------------------------------------------------------
    // Task 16: format_results unit tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_format_results_concatenate() {
        let results = vec![
            ("researcher".to_string(), "Found data A".to_string()),
            ("writer".to_string(), "Wrote summary B".to_string()),
        ];
        let out = format_results(&results, "concatenate");
        assert!(
            out.contains("[researcher]: Found data A"),
            "Missing researcher result: {}",
            out
        );
        assert!(
            out.contains("[writer]: Wrote summary B"),
            "Missing writer result: {}",
            out
        );
        // The two blocks must be separated by a blank line.
        assert!(
            out.contains("\n\n"),
            "Expected blank-line separator: {}",
            out
        );
    }

    #[test]
    fn test_format_results_summarize() {
        let results = vec![
            ("analyst".to_string(), "Analysis result".to_string()),
            ("coder".to_string(), "Code review done".to_string()),
        ];
        let out = format_results(&results, "summarize");
        assert!(
            out.starts_with("## Aggregated Results"),
            "Expected h2 header: {}",
            out
        );
        assert!(
            out.contains("### analyst"),
            "Missing analyst header: {}",
            out
        );
        assert!(
            out.contains("Analysis result"),
            "Missing analyst body: {}",
            out
        );
        assert!(out.contains("### coder"), "Missing coder header: {}", out);
        assert!(
            out.contains("Code review done"),
            "Missing coder body: {}",
            out
        );
    }

    #[test]
    fn test_format_results_empty() {
        let results: Vec<(String, String)> = vec![];

        let concat = format_results(&results, "concatenate");
        assert_eq!(concat, "", "Empty concatenate should be empty string");

        let summarize = format_results(&results, "summarize");
        assert!(
            summarize.starts_with("## Aggregated Results"),
            "Empty summarize should still have header: {}",
            summarize
        );
    }

    #[test]
    fn test_format_results_unknown_strategy_falls_back_to_concatenate() {
        let results = vec![("role".to_string(), "result".to_string())];
        let out = format_results(&results, "unknown_strategy");
        assert!(
            out.contains("[role]: result"),
            "Unknown strategy should fall back to concatenate: {}",
            out
        );
    }

    // -------------------------------------------------------------------------
    // Task 16: aggregate action dispatch tests
    // -------------------------------------------------------------------------

    /// aggregate without a 'tasks' key must return an error mentioning "tasks".
    #[tokio::test]
    async fn test_aggregate_requires_tasks() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool.execute(json!({"action": "aggregate"}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("tasks"),
            "Expected error mentioning 'tasks', got: {}",
            err_msg
        );
    }

    /// A task entry with a role but no 'task' field must error.
    #[tokio::test]
    async fn test_aggregate_task_requires_task_field() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool
            .execute(
                json!({
                    "action": "aggregate",
                    "tasks": [{"role": "analyst"}]
                }),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("task"),
            "Expected error mentioning 'task' field, got: {}",
            err_msg
        );
    }

    /// When 'action' is absent the tool should default to "run" and validate
    /// role/task — NOT emit an "unknown action" error.
    #[tokio::test]
    async fn test_action_default_is_run() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        // No 'action' key — should route to the "run" path and fail on missing role.
        let result = tool.execute(json!({"task": "hello"}), &ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("role"),
            "Missing 'action' should default to 'run' and fail on missing role, got: {}",
            err_msg
        );
        // Must NOT be an "Unknown action" error.
        assert!(
            !err_msg.contains("Unknown action"),
            "Should not get unknown-action error when action is absent: {}",
            err_msg
        );
    }

    // -------------------------------------------------------------------------
    // Task 17: SwarmScratchpad integration test
    // -------------------------------------------------------------------------

    #[test]
    fn test_delegate_has_scratchpad() {
        let tool = test_delegate_tool(true);
        // Scratchpad starts empty
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(tool.scratchpad().is_empty()));
    }

    /// An unrecognised action value must produce an error containing the bad value.
    #[tokio::test]
    async fn test_unknown_action_errors() {
        let tool = test_delegate_tool(true);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool
            .execute(json!({"action": "foo", "role": "r", "task": "t"}), &ctx)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("foo") || err_msg.contains("Unknown action"),
            "Expected unknown-action error, got: {}",
            err_msg
        );
    }

    /// The JSON schema returned by `parameters()` must expose all new fields
    /// (`action`, `tasks`, `merge_strategy`) alongside the original ones.
    #[test]
    fn test_parameters_include_aggregate_fields() {
        let tool = test_delegate_tool(true);
        let params = tool.parameters();
        let props = &params["properties"];

        // New fields
        assert!(
            props["action"].is_object(),
            "action field missing from schema"
        );
        assert!(
            props["tasks"].is_object(),
            "tasks field missing from schema"
        );
        assert!(
            props["merge_strategy"].is_object(),
            "merge_strategy field missing from schema"
        );

        // Existing fields must still be present
        assert!(props["role"].is_object(), "role field missing from schema");
        assert!(props["task"].is_object(), "task field missing from schema");
        assert!(
            props["tools"].is_object(),
            "tools field missing from schema"
        );
    }
}
