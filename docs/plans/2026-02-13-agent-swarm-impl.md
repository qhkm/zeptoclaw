# Agent Swarm (DelegateTool) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `delegate` tool that enables the lead agent to spawn specialist sub-agents with role-specific system prompts and configurable tool whitelists.

**Architecture:** A single `DelegateTool` implements the `Tool` trait. On each invocation it creates a temporary `AgentLoop` with a custom `ContextBuilder` (role prompt), registers a filtered subset of tools, calls `process_message()`, and returns the result string. Sub-agents can also message the user directly via `MessageTool`.

**Tech Stack:** Rust, async-trait, serde_json, tokio, uuid

---

### Task 1: Add SwarmConfig to config types

**Files:**
- Modify: `src/config/types.rs`

**Step 1: Write the test**

Add to the existing test module in `src/config/types.rs`:

```rust
#[test]
fn test_swarm_config_defaults() {
    let config = SwarmConfig::default();
    assert!(config.enabled);
    assert_eq!(config.max_depth, 1);
    assert_eq!(config.max_concurrent, 3);
    assert!(config.roles.is_empty());
}

#[test]
fn test_swarm_config_deserialize() {
    let json = r#"{
        "enabled": true,
        "roles": {
            "researcher": {
                "system_prompt": "You are a researcher.",
                "tools": ["web_search", "web_fetch"]
            }
        }
    }"#;
    let config: SwarmConfig = serde_json::from_str(json).unwrap();
    assert!(config.enabled);
    assert_eq!(config.roles.len(), 1);
    let role = config.roles.get("researcher").unwrap();
    assert_eq!(role.tools, vec!["web_search", "web_fetch"]);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_swarm_config -- --nocapture`
Expected: FAIL — `SwarmConfig` not found

**Step 3: Implement SwarmConfig and SwarmRole**

Add to `src/config/types.rs` after the existing `SkillsConfig` block:

```rust
/// Swarm / multi-agent delegation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SwarmConfig {
    /// Whether delegation is enabled
    pub enabled: bool,
    /// Maximum delegation depth (1 = no sub-sub-agents)
    pub max_depth: u32,
    /// Maximum concurrent sub-agents (for future parallel mode)
    pub max_concurrent: u32,
    /// Pre-defined role presets with tool whitelists
    pub roles: std::collections::HashMap<String, SwarmRole>,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 1,
            max_concurrent: 3,
            roles: std::collections::HashMap::new(),
        }
    }
}

/// A pre-defined sub-agent role with system prompt and tool whitelist
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SwarmRole {
    /// System prompt for this role
    pub system_prompt: String,
    /// Allowed tool names (empty = all minus delegate/spawn)
    pub tools: Vec<String>,
}

impl Default for SwarmRole {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            tools: Vec::new(),
        }
    }
}
```

Add `swarm` field to the `Config` struct:

```rust
pub struct Config {
    // ... existing fields ...
    /// Swarm / multi-agent delegation configuration
    pub swarm: SwarmConfig,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_swarm_config -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/config/types.rs
git commit -m "feat(swarm): add SwarmConfig and SwarmRole to config types"
```

---

### Task 2: Create DelegateTool

**Files:**
- Create: `src/tools/delegate.rs`
- Modify: `src/tools/mod.rs`

**Step 1: Write the test (recursion blocking)**

At the bottom of `src/tools/delegate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::EchoTool;
    use serde_json::json;

    #[tokio::test]
    async fn test_delegate_blocked_from_subagent() {
        let config = Config::default();
        let bus = Arc::new(MessageBus::new());
        let provider: Arc<dyn LLMProvider> = Arc::new(crate::providers::claude::ClaudeProvider::new("fake-key"));
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool)];

        let tool = DelegateTool::new(config, Arc::clone(&provider), bus, tools);
        let ctx = ToolContext::new().with_channel("delegate", "sub-123");

        let result = tool.execute(json!({"role": "test", "task": "hello"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("recursion"));
    }

    #[tokio::test]
    async fn test_delegate_requires_role_and_task() {
        let config = Config::default();
        let bus = Arc::new(MessageBus::new());
        let provider: Arc<dyn LLMProvider> = Arc::new(crate::providers::claude::ClaudeProvider::new("fake-key"));
        let tools: Vec<Box<dyn Tool>> = vec![];

        let tool = DelegateTool::new(config, Arc::clone(&provider), bus, tools);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("role"));
    }

    #[tokio::test]
    async fn test_delegate_disabled_in_config() {
        let mut config = Config::default();
        config.swarm.enabled = false;
        let bus = Arc::new(MessageBus::new());
        let provider: Arc<dyn LLMProvider> = Arc::new(crate::providers::claude::ClaudeProvider::new("fake-key"));
        let tools: Vec<Box<dyn Tool>> = vec![];

        let tool = DelegateTool::new(config, Arc::clone(&provider), bus, tools);
        let ctx = ToolContext::new().with_channel("telegram", "chat-1");

        let result = tool.execute(json!({"role": "test", "task": "hello"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_delegate -- --nocapture`
Expected: FAIL — module not found

**Step 3: Implement DelegateTool**

Create `src/tools/delegate.rs`:

```rust
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
use crate::providers::LLMProvider;
use crate::session::SessionManager;
use crate::tools::message::MessageTool;

use super::{Tool, ToolContext, ToolRegistry};

/// Tool to delegate a task to a specialist sub-agent.
pub struct DelegateTool {
    config: Config,
    provider: Arc<dyn LLMProvider>,
    bus: Arc<MessageBus>,
    /// Snapshot of available tools for sub-agents (name → tool).
    /// Excludes delegate and spawn to prevent recursion.
    available_tools: Vec<Box<dyn Tool>>,
}

impl DelegateTool {
    /// Create a new delegate tool.
    ///
    /// `available_tools` should contain the tools that sub-agents may use.
    /// The DelegateTool itself and SpawnTool should NOT be included.
    pub fn new(
        config: Config,
        provider: Arc<dyn LLMProvider>,
        bus: Arc<MessageBus>,
        available_tools: Vec<Box<dyn Tool>>,
    ) -> Self {
        Self {
            config,
            provider,
            bus,
            available_tools,
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

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "role": {
                    "type": "string",
                    "description": "The specialist role (e.g., 'Researcher', 'Writer', 'Analyst')"
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
            .ok_or_else(|| ZeptoError::Tool("Missing 'role' argument".into()))?;
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'task' argument".into()))?;
        let tool_override: Option<Vec<String>> = args
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let role_lower = role.to_lowercase();
        let role_config = self.config.swarm.roles.get(&role_lower);

        // Build system prompt
        let system_prompt = match role_config {
            Some(rc) if !rc.system_prompt.is_empty() => rc.system_prompt.clone(),
            _ => format!(
                "You are a specialist with the role: {}. \
                 Complete the task given to you thoroughly and return your findings. \
                 You can send interim updates to the user via the message tool.",
                role
            ),
        };

        // Determine allowed tools
        let allowed_tool_names: Option<Vec<String>> = tool_override.or_else(|| {
            role_config
                .filter(|rc| !rc.tools.is_empty())
                .map(|rc| rc.tools.clone())
        });

        info!(role = %role, task_len = task.len(), "Delegating task to sub-agent");

        // Create sub-agent
        let session_manager = SessionManager::new_in_memory();
        let sub_bus = Arc::new(MessageBus::new());
        let context_builder = ContextBuilder::new().with_system_prompt(&system_prompt);

        let sub_agent = AgentLoop::with_context_builder(
            self.config.clone(),
            session_manager,
            Arc::clone(&sub_bus),
            context_builder,
        );
        sub_agent.set_provider(Arc::clone(&self.provider)).await;

        // Register tools (filtered by whitelist)
        // Always add MessageTool for direct user messaging
        sub_agent
            .register_tool(Box::new(MessageTool::new(self.bus.clone())))
            .await;

        // NOTE: available_tools is consumed at construction time, so we
        // need a different approach. The DelegateTool holds tool factories
        // or we re-register from the parent's registry.
        //
        // For v1: DelegateTool receives a list of tool-creation closures
        // or we clone the tool registry. Since Tool is not Clone, we use
        // a registry snapshot approach — the parent passes tool names and
        // the DelegateTool creates fresh instances.
        //
        // Implementation detail handled in Task 3 (wiring in main.rs).

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
```

Add to `src/tools/mod.rs`:

```rust
pub mod delegate;
```

And add to the `pub use` section:

```rust
pub use delegate::DelegateTool;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test test_delegate -- --nocapture`
Expected: All 3 tests PASS

**Step 5: Commit**

```bash
git add src/tools/delegate.rs src/tools/mod.rs
git commit -m "feat(swarm): add DelegateTool with recursion blocking"
```

---

### Task 3: Wire DelegateTool into create_agent()

**Files:**
- Modify: `src/main.rs` (in `create_agent()` function, around line 729)

**Step 1: Register DelegateTool after all other tools**

The challenge: `DelegateTool` needs access to the same provider and a way to
create tools for sub-agents. Since `Tool` is not `Clone`, we need to create
fresh tool instances for each delegation.

Approach: Pass a tool factory function (closure) to DelegateTool, or more
simply, pass the Config + provider and let DelegateTool create tools itself
using a `create_sub_agent_tools()` helper.

Add a helper function in `src/tools/delegate.rs`:

```rust
/// Create the standard tool set for a sub-agent, filtered by an optional whitelist.
pub fn create_sub_agent_tools(
    config: &Config,
    bus: &Arc<MessageBus>,
    whitelist: Option<&[String]>,
) -> Vec<Box<dyn Tool>> {
    use crate::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
    use crate::tools::shell::ShellTool;
    use crate::tools::memory::{MemorySearchTool, MemoryGetTool};
    use crate::tools::web::{WebSearchTool, WebFetchTool};
    use crate::tools::message::MessageTool;
    use crate::tools::EchoTool;

    let all_tools: Vec<Box<dyn Tool>> = vec![
        Box::new(EchoTool),
        Box::new(ReadFileTool),
        Box::new(WriteFileTool),
        Box::new(ListDirTool),
        Box::new(EditFileTool),
        Box::new(ShellTool::new()),
        Box::new(WebFetchTool::new()),
        Box::new(MessageTool::new(bus.clone())),
        Box::new(MemorySearchTool::new(config.memory.clone())),
        Box::new(MemoryGetTool::new(config.memory.clone())),
    ];

    // Add WebSearchTool if configured
    // (omit for brevity — check config.tools.web.search.api_key)

    match whitelist {
        Some(names) => all_tools
            .into_iter()
            .filter(|t| names.iter().any(|n| n == t.name()))
            .collect(),
        None => all_tools,
    }
}
```

Then refactor `DelegateTool` to hold Config + provider + bus and call
`create_sub_agent_tools()` inside `execute()` instead of taking a pre-built
tool list.

In `src/main.rs` `create_agent()`, register after SpawnTool:

```rust
if config.swarm.enabled {
    agent
        .register_tool(Box::new(DelegateTool::new(
            config.clone(),
            provider.clone(),  // the resolved provider Arc
            bus.clone(),
        )))
        .await;
}
```

**Step 2: Build and run tests**

Run: `cargo build && cargo test`
Expected: Compiles, all tests pass

**Step 3: Commit**

```bash
git add src/tools/delegate.rs src/main.rs
git commit -m "feat(swarm): wire DelegateTool into create_agent()"
```

---

### Task 4: Integration test — delegate with EchoTool

**Files:**
- Modify: `tests/integration.rs`

**Step 1: Write the integration test**

```rust
#[tokio::test]
async fn test_delegate_tool_with_echo() {
    use zeptoclaw::tools::delegate::DelegateTool;
    use zeptoclaw::tools::{EchoTool, Tool, ToolContext};
    use zeptoclaw::config::Config;
    use zeptoclaw::bus::MessageBus;
    use std::sync::Arc;
    use serde_json::json;

    // This test verifies that DelegateTool blocks recursion
    // (full e2e delegation requires a real LLM provider)
    let config = Config::default();
    let bus = Arc::new(MessageBus::new());
    let provider: Arc<dyn zeptoclaw::providers::LLMProvider> =
        Arc::new(zeptoclaw::providers::claude::ClaudeProvider::new("fake"));

    let tool = DelegateTool::new(config, provider, bus);

    // Should block when called from delegate context
    let ctx = ToolContext::new().with_channel("delegate", "sub-1");
    let result = tool.execute(json!({"role": "Test", "task": "hello"}), &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("recursion"));

    // Should require role
    let ctx = ToolContext::new().with_channel("telegram", "chat-1");
    let result = tool.execute(json!({"task": "hello"}), &ctx).await;
    assert!(result.is_err());

    // Should require task
    let result = tool.execute(json!({"role": "Test"}), &ctx).await;
    assert!(result.is_err());
}
```

**Step 2: Run test**

Run: `cargo test test_delegate_tool_with_echo -- --nocapture`
Expected: PASS

**Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test(swarm): add DelegateTool integration tests"
```

---

### Task 5: Add swarm config to config.example.json and docs

**Files:**
- Modify: `scripts/add-tenant.sh` (add swarm section to generated config)
- Modify: `docs/MULTI-TENANT.md` (mention swarm config)

**Step 1: Add swarm section to add-tenant.sh config template**

In the python3 JSON generation, add:

```python
"swarm": {
    "enabled": True,
    "max_depth": 1,
    "max_concurrent": 3,
    "roles": {}
}
```

**Step 2: Update MULTI-TENANT.md**

Add a "Swarm Configuration" subsection under the configuration docs explaining
how to define role presets.

**Step 3: Commit**

```bash
git add scripts/add-tenant.sh docs/MULTI-TENANT.md
git commit -m "docs(swarm): add swarm config to tenant template and docs"
```

---

### Task 6: Final build, test, clippy

**Step 1: Full build**

Run: `cargo build --release`
Expected: Compiles with 0 warnings

**Step 2: Full test suite**

Run: `cargo test`
Expected: All tests pass (previous 589 + new swarm tests)

**Step 3: Clippy**

Run: `cargo clippy -- -D warnings`
Expected: 0 warnings

**Step 4: Commit any fixups**

```bash
git add -A
git commit -m "chore(swarm): clippy and test fixups"
```

---

## Task Dependency Graph

```
Task 1 (config types)
    ↓
Task 2 (DelegateTool)
    ↓
Task 3 (wire into main.rs)
    ↓
Task 4 (integration test)
    ↓
Task 5 (docs + tenant config)
    ↓
Task 6 (final validation)
```

All tasks are sequential — each depends on the previous.
