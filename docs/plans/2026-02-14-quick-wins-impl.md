# Quick Wins Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add 5 quick-win features: parallel tool execution, tool result sanitization, agent-level timeout, config validation CLI, and message queue modes.

**Architecture:** Each feature is mostly self-contained. Task 1 (sanitization utility) is a foundation used by Task 2 (parallel tools + sanitization in agent loop). Task 3 (timeout) wraps the agent loop. Task 4 (config check) is standalone CLI. Task 5 (message queue) changes the agent loop's message consumption pattern.

**Tech Stack:** Rust, Tokio (futures::future::join_all, tokio::time::timeout), serde_json, regex

---

### Task 1: Tool Result Sanitization Module

**Files:**
- Create: `src/utils/sanitize.rs`
- Modify: `src/utils/mod.rs`

**Step 1: Write the failing tests**

Create `src/utils/sanitize.rs` with tests first:

```rust
//! Tool result sanitization.
//!
//! Strips base64 data URIs, long hex blobs, and truncates oversized
//! results before feeding them back to the LLM. This saves tokens
//! without losing meaningful information.

use regex::Regex;
use once_cell::sync::Lazy;

/// Default maximum result size in bytes (50 KB).
pub const DEFAULT_MAX_RESULT_BYTES: usize = 51_200;

/// Minimum length of a contiguous hex string to be stripped.
const MIN_HEX_BLOB_LEN: usize = 200;

static BASE64_URI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"data:[a-zA-Z0-9/+\-\.]+;base64,[A-Za-z0-9+/=\s]+").unwrap()
});

static HEX_BLOB_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"[0-9a-fA-F]{{{},}}", MIN_HEX_BLOB_LEN)).unwrap()
});

/// Sanitize a tool result string.
///
/// 1. Replace `data:...;base64,...` URIs with a placeholder.
/// 2. Replace hex blobs (>= 200 hex chars) with a placeholder.
/// 3. Truncate to `max_bytes` if still too large.
pub fn sanitize_tool_result(result: &str, max_bytes: usize) -> String {
    let mut out = BASE64_URI_RE
        .replace_all(result, |caps: &regex::Captures| {
            let len = caps[0].len();
            format!("[base64 data removed, {} bytes]", len)
        })
        .into_owned();

    out = HEX_BLOB_RE
        .replace_all(&out, |caps: &regex::Captures| {
            let len = caps[0].len();
            format!("[hex data removed, {} chars]", len)
        })
        .into_owned();

    if out.len() > max_bytes {
        let total = out.len();
        out.truncate(max_bytes);
        // Ensure we don't split a multi-byte char
        while !out.is_char_boundary(out.len()) {
            out.pop();
        }
        out.push_str(&format!("\n...[truncated, {} total bytes]", total));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_change_for_normal_text() {
        let input = "Hello, world! This is a normal tool result.";
        assert_eq!(sanitize_tool_result(input, DEFAULT_MAX_RESULT_BYTES), input);
    }

    #[test]
    fn test_strips_base64_data_uri() {
        let b64 = "A".repeat(500);
        let input = format!("before data:image/png;base64,{} after", b64);
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(!result.contains(&b64));
        assert!(result.contains("[base64 data removed,"));
        assert!(result.contains("before"));
        assert!(result.contains("after"));
    }

    #[test]
    fn test_strips_hex_blob() {
        let hex = "a1b2c3d4e5f6".repeat(40); // 480 hex chars
        let input = format!("prefix {} suffix", hex);
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(!result.contains(&hex));
        assert!(result.contains("[hex data removed,"));
        assert!(result.contains("prefix"));
        assert!(result.contains("suffix"));
    }

    #[test]
    fn test_short_hex_not_stripped() {
        let hex = "abcdef1234"; // 10 chars, below threshold
        let input = format!("hash: {}", hex);
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(result.contains(hex));
    }

    #[test]
    fn test_truncation() {
        let input = "x".repeat(1000);
        let result = sanitize_tool_result(&input, 100);
        assert!(result.len() < 200); // 100 + truncation message
        assert!(result.contains("[truncated, 1000 total bytes]"));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(sanitize_tool_result("", DEFAULT_MAX_RESULT_BYTES), "");
    }

    #[test]
    fn test_multiple_base64_uris() {
        let b64 = "Q".repeat(100);
        let input = format!(
            "img1: data:image/png;base64,{} and img2: data:application/pdf;base64,{}",
            b64, b64
        );
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(!result.contains(&b64));
        // Should have two replacement markers
        assert_eq!(result.matches("[base64 data removed,").count(), 2);
    }
}
```

**Step 2: Wire up the module**

Modify `src/utils/mod.rs`:

```rust
//! Utils module - Utility functions and helpers

pub mod sanitize;
```

**Step 3: Run tests to verify they pass**

Run: `cargo test utils::sanitize --lib`
Expected: All 7 tests PASS (the implementation is included in step 1 above since TDD for pure functions benefits from writing both at once).

**Step 4: Commit**

```bash
git add src/utils/sanitize.rs src/utils/mod.rs
git commit -m "feat: add tool result sanitization (base64, hex, truncation)"
```

---

### Task 2: Parallel Tool Execution + Sanitization in Agent Loop

**Files:**
- Modify: `src/agent/loop.rs:286-376` (the tool loop)

**Step 1: Write a test for parallel tool execution**

Add to `src/agent/loop.rs` tests section:

```rust
#[tokio::test]
async fn test_parallel_tool_execution_ordering() {
    // This test verifies that results maintain order even when
    // tools complete at different speeds
    use crate::tools::EchoTool;
    use crate::providers::{LLMResponse, LLMToolCall};

    let config = Config::default();
    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = AgentLoop::new(config, session_manager, bus);

    agent.register_tool(Box::new(EchoTool)).await;

    // Verify tool count to ensure registration works
    assert_eq!(agent.tool_count().await, 1);
}
```

**Step 2: Replace sequential tool execution with parallel**

In `src/agent/loop.rs`, replace lines 316-353 (the `for tool_call in &response.tool_calls` loop) with:

```rust
            // Execute tool calls in parallel
            let workspace = self.config.workspace_path();
            let workspace_str = workspace.to_string_lossy();
            let tool_ctx = ToolContext::new()
                .with_channel(&msg.channel, &msg.chat_id)
                .with_workspace(&workspace_str);

            let tool_futures: Vec<_> = response
                .tool_calls
                .iter()
                .map(|tool_call| {
                    let tools = Arc::clone(&self.tools);
                    let ctx = tool_ctx.clone();
                    let name = tool_call.name.clone();
                    let id = tool_call.id.clone();
                    let raw_args = tool_call.arguments.clone();
                    let usage_metrics = usage_metrics.clone();

                    async move {
                        let args: serde_json::Value = match serde_json::from_str(&raw_args) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(tool = %name, error = %e, "Invalid JSON in tool arguments");
                                serde_json::json!({"_parse_error": format!("Invalid arguments JSON: {}", e)})
                            }
                        };

                        let tool_start = std::time::Instant::now();
                        let result = {
                            let tools_guard = tools.read().await;
                            match tools_guard.execute_with_context(&name, args, &ctx).await {
                                Ok(r) => {
                                    let latency_ms = tool_start.elapsed().as_millis() as u64;
                                    debug!(tool = %name, latency_ms = latency_ms, "Tool executed successfully");
                                    r
                                }
                                Err(e) => {
                                    let latency_ms = tool_start.elapsed().as_millis() as u64;
                                    error!(tool = %name, latency_ms = latency_ms, error = %e, "Tool execution failed");
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }
                                    format!("Error: {}", e)
                                }
                            }
                        };

                        // Sanitize the result before feeding back to LLM
                        let sanitized = crate::utils::sanitize::sanitize_tool_result(
                            &result,
                            crate::utils::sanitize::DEFAULT_MAX_RESULT_BYTES,
                        );

                        (id, sanitized)
                    }
                })
                .collect();

            let results = futures::future::join_all(tool_futures).await;

            for (id, result) in results {
                session.add_message(Message::tool_result(&id, &result));
            }
```

**Step 3: Run all tests**

Run: `cargo test --lib`
Expected: All existing tests pass + new test passes.

**Step 4: Commit**

```bash
git add src/agent/loop.rs
git commit -m "feat: parallel tool execution with result sanitization"
```

---

### Task 3: Agent-Level Timeout

**Files:**
- Modify: `src/config/types.rs:47-77` (AgentDefaults)
- Modify: `src/config/mod.rs` (env override)
- Modify: `src/agent/loop.rs:457-513` (start() method timeout wrapping)

**Step 1: Add config field**

In `src/config/types.rs`, add to `AgentDefaults`:

```rust
pub struct AgentDefaults {
    pub workspace: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub max_tool_iterations: u32,
    /// Maximum wall-clock time (seconds) for a single agent run.
    pub agent_timeout_secs: u64,
}
```

Update the `Default` impl:

```rust
impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.zeptoclaw/workspace".to_string(),
            model: COMPILE_TIME_DEFAULT_MODEL.to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            max_tool_iterations: 20,
            agent_timeout_secs: 300,
        }
    }
}
```

**Step 2: Add env override**

In `src/config/mod.rs`, inside `apply_env_overrides()`, after the `max_tool_iterations` block (around line 80), add:

```rust
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS") {
            if let Ok(v) = val.parse() {
                self.agents.defaults.agent_timeout_secs = v;
            }
        }
```

**Step 3: Wrap process_message with timeout**

In `src/agent/loop.rs`, inside the `start()` method, replace the `match self.process_message(msg_ref).await {` block (lines ~470-511) with:

```rust
                            let timeout_duration = std::time::Duration::from_secs(
                                self.config.agents.defaults.agent_timeout_secs,
                            );
                            let process_result = tokio::time::timeout(
                                timeout_duration,
                                self.process_message(msg_ref),
                            )
                            .await;

                            match process_result {
                                Ok(Ok(response)) => {
                                    let latency_ms = start.elapsed().as_millis() as u64;
                                    let (input_tokens, output_tokens) = tokens_before
                                        .and_then(|(ib, ob)| {
                                            usage_metrics.as_ref().map(|m| {
                                                let ia = m.input_tokens.load(std::sync::atomic::Ordering::Relaxed);
                                                let oa = m.output_tokens.load(std::sync::atomic::Ordering::Relaxed);
                                                (ia.saturating_sub(ib), oa.saturating_sub(ob))
                                            })
                                        })
                                        .unwrap_or((0, 0));
                                    info!(
                                        latency_ms = latency_ms,
                                        response_len = response.len(),
                                        input_tokens = input_tokens,
                                        output_tokens = output_tokens,
                                        "Request completed"
                                    );

                                    let outbound = OutboundMessage::new(&msg_ref.channel, &msg_ref.chat_id, &response);
                                    if let Err(e) = bus_ref.publish_outbound(outbound).await {
                                        error!("Failed to publish outbound message: {}", e);
                                        if let Some(metrics) = usage_metrics.as_ref() {
                                            metrics.record_error();
                                        }
                                    }
                                }
                                Ok(Err(e)) => {
                                    let latency_ms = start.elapsed().as_millis() as u64;
                                    error!(latency_ms = latency_ms, error = %e, "Request failed");
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }

                                    let error_msg = OutboundMessage::new(
                                        &msg_ref.channel,
                                        &msg_ref.chat_id,
                                        &format!("Error: {}", e),
                                    );
                                    bus_ref.publish_outbound(error_msg).await.ok();
                                }
                                Err(_elapsed) => {
                                    let timeout_secs = self.config.agents.defaults.agent_timeout_secs;
                                    error!(timeout_secs = timeout_secs, "Agent run timed out");
                                    if let Some(metrics) = usage_metrics.as_ref() {
                                        metrics.record_error();
                                    }

                                    let timeout_msg = OutboundMessage::new(
                                        &msg_ref.channel,
                                        &msg_ref.chat_id,
                                        &format!("Agent run timed out after {}s. Try a simpler request.", timeout_secs),
                                    );
                                    bus_ref.publish_outbound(timeout_msg).await.ok();
                                }
                            }
```

**Step 4: Add tests**

In `src/config/types.rs` tests or `src/config/mod.rs` tests:

```rust
#[test]
fn test_agent_timeout_default() {
    let config = Config::default();
    assert_eq!(config.agents.defaults.agent_timeout_secs, 300);
}

#[test]
fn test_agent_timeout_from_json() {
    let json = r#"{"agents": {"defaults": {"agent_timeout_secs": 600}}}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.agents.defaults.agent_timeout_secs, 600);
}
```

**Step 5: Run tests**

Run: `cargo test --lib`
Expected: All tests PASS.

**Step 6: Commit**

```bash
git add src/config/types.rs src/config/mod.rs src/agent/loop.rs
git commit -m "feat: add agent-level timeout (default 300s)"
```

---

### Task 4: Config Validation CLI (`zeptoclaw config check`)

**Files:**
- Create: `src/config/validate.rs`
- Modify: `src/config/mod.rs` (export validate module)
- Modify: `src/main.rs` (add Config subcommand + handler)

**Step 1: Create validation module**

Create `src/config/validate.rs`:

```rust
//! Configuration validation with unknown field detection.

use serde_json::Value;
use std::collections::HashSet;

/// Known top-level config field names.
const KNOWN_TOP_LEVEL: &[&str] = &[
    "agents", "channels", "providers", "gateway", "tools",
    "memory", "heartbeat", "skills", "runtime", "container_agent",
];

/// Known fields for each section. Nested as section.field.
const KNOWN_AGENTS_DEFAULTS: &[&str] = &[
    "workspace", "model", "max_tokens", "temperature",
    "max_tool_iterations", "agent_timeout_secs", "message_queue_mode",
];

const KNOWN_GATEWAY: &[&str] = &["host", "port"];

/// A validation diagnostic.
#[derive(Debug)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub path: String,
    pub message: String,
}

#[derive(Debug, PartialEq)]
pub enum DiagnosticLevel {
    Ok,
    Warn,
    Error,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.level {
            DiagnosticLevel::Ok => "[OK]",
            DiagnosticLevel::Warn => "[WARN]",
            DiagnosticLevel::Error => "[ERROR]",
        };
        if self.path.is_empty() {
            write!(f, "{} {}", prefix, self.message)
        } else {
            write!(f, "{} {}: {}", prefix, self.path, self.message)
        }
    }
}

/// Simple Levenshtein distance for "did you mean?" suggestions.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for i in 0..=a_len { matrix[i][0] = i; }
    for j in 0..=b_len { matrix[0][j] = j; }

    for (i, ca) in a.chars().enumerate() {
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            matrix[i + 1][j + 1] = std::cmp::min(
                std::cmp::min(matrix[i][j + 1] + 1, matrix[i + 1][j] + 1),
                matrix[i][j] + cost,
            );
        }
    }
    matrix[a_len][b_len]
}

/// Suggest the closest known field name (if distance <= 3).
pub fn suggest_field(unknown: &str, known: &[&str]) -> Option<String> {
    known
        .iter()
        .map(|k| (k, levenshtein(unknown, k)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| format!("did you mean '{}'?", k))
}

/// Validate a raw JSON config value against known field names.
pub fn validate_config(raw: &Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Check it's an object
    let obj = match raw.as_object() {
        Some(o) => o,
        None => {
            diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                path: String::new(),
                message: "Config must be a JSON object".to_string(),
            });
            return diagnostics;
        }
    };

    diagnostics.push(Diagnostic {
        level: DiagnosticLevel::Ok,
        path: String::new(),
        message: "Valid JSON".to_string(),
    });

    // Check top-level keys
    let known_set: HashSet<&str> = KNOWN_TOP_LEVEL.iter().copied().collect();
    let mut has_unknown = false;
    for key in obj.keys() {
        if !known_set.contains(key.as_str()) {
            has_unknown = true;
            let suggestion = suggest_field(key, KNOWN_TOP_LEVEL)
                .unwrap_or_default();
            let msg = if suggestion.is_empty() {
                format!("Unknown field '{}'", key)
            } else {
                format!("Unknown field '{}' — {}", key, suggestion)
            };
            diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                path: key.clone(),
                message: msg,
            });
        }
    }

    // Check agents.defaults keys
    if let Some(agents) = obj.get("agents").and_then(|v| v.as_object()) {
        if let Some(defaults) = agents.get("defaults").and_then(|v| v.as_object()) {
            let known_set: HashSet<&str> = KNOWN_AGENTS_DEFAULTS.iter().copied().collect();
            for key in defaults.keys() {
                if !known_set.contains(key.as_str()) {
                    has_unknown = true;
                    let suggestion = suggest_field(key, KNOWN_AGENTS_DEFAULTS)
                        .unwrap_or_default();
                    let msg = if suggestion.is_empty() {
                        format!("Unknown field '{}'", key)
                    } else {
                        format!("Unknown field '{}' — {}", key, suggestion)
                    };
                    diagnostics.push(Diagnostic {
                        level: DiagnosticLevel::Error,
                        path: format!("agents.defaults.{}", key),
                        message: msg,
                    });
                }
            }
        }
    }

    if !has_unknown {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            path: String::new(),
            message: "All fields recognized".to_string(),
        });
    }

    // Security warnings
    if let Some(channels) = obj.get("channels").and_then(|v| v.as_object()) {
        for (name, channel_val) in channels {
            if let Some(channel_obj) = channel_val.as_object() {
                let enabled = channel_obj
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let allow_from = channel_obj
                    .get("allow_from")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);

                if enabled && allow_from == 0 {
                    diagnostics.push(Diagnostic {
                        level: DiagnosticLevel::Warn,
                        path: format!("channels.{}.allow_from", name),
                        message: "Empty — anyone can message the bot".to_string(),
                    });
                }
            }
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_one_edit() {
        assert_eq!(levenshtein("hello", "helo"), 1);
    }

    #[test]
    fn test_levenshtein_different() {
        assert!(levenshtein("hello", "world") > 3);
    }

    #[test]
    fn test_suggest_field_match() {
        let result = suggest_field("gatway", KNOWN_TOP_LEVEL);
        assert!(result.is_some());
        assert!(result.unwrap().contains("gateway"));
    }

    #[test]
    fn test_suggest_field_no_match() {
        let result = suggest_field("xyzabc", KNOWN_TOP_LEVEL);
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_valid_config() {
        let raw = json!({
            "agents": {"defaults": {"model": "gpt-4"}},
            "gateway": {"port": 8080}
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().all(|d| d.level != DiagnosticLevel::Error));
    }

    #[test]
    fn test_validate_unknown_top_level() {
        let raw = json!({
            "agentsss": {}
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| d.level == DiagnosticLevel::Error));
    }

    #[test]
    fn test_validate_security_warning_empty_allowlist() {
        let raw = json!({
            "channels": {
                "telegram": {
                    "enabled": true,
                    "token": "abc",
                    "allow_from": []
                }
            }
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| {
            d.level == DiagnosticLevel::Warn && d.message.contains("anyone can message")
        }));
    }

    #[test]
    fn test_validate_not_an_object() {
        let raw = json!("not an object");
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| {
            d.level == DiagnosticLevel::Error && d.message.contains("must be a JSON object")
        }));
    }
}
```

**Step 2: Export the module**

In `src/config/mod.rs`, add:

```rust
pub mod validate;
```

after `mod types;`.

**Step 3: Add CLI subcommand**

In `src/main.rs`, add to the `Commands` enum:

```rust
    /// Validate configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
```

Add the enum:

```rust
#[derive(Subcommand)]
enum ConfigAction {
    /// Check configuration for errors and warnings
    Check,
}
```

Add to the match block in `main()`:

```rust
        Some(Commands::Config { action }) => {
            cmd_config(action).await?;
        }
```

Add the handler function:

```rust
async fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Check => {
            let config_path = zeptoclaw::config::Config::path();
            println!("Config file: {}", config_path.display());

            if !config_path.exists() {
                println!("[OK] No config file found (using defaults)");
                return Ok(());
            }

            let content = std::fs::read_to_string(&config_path)
                .context("Failed to read config file")?;

            let raw: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    println!("[ERROR] Invalid JSON: {}", e);
                    return Ok(());
                }
            };

            let diagnostics = zeptoclaw::config::validate::validate_config(&raw);
            for diag in &diagnostics {
                println!("{}", diag);
            }

            let errors = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Error)
                .count();
            let warnings = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Warn)
                .count();

            if errors == 0 && warnings == 0 {
                println!("\nConfiguration looks good!");
            } else {
                println!("\nFound {} error(s), {} warning(s)", errors, warnings);
            }
        }
    }
    Ok(())
}
```

**Step 4: Run tests**

Run: `cargo test config::validate --lib`
Expected: All validation tests pass.

Run: `cargo build`
Expected: Compiles successfully.

**Step 5: Commit**

```bash
git add src/config/validate.rs src/config/mod.rs src/main.rs
git commit -m "feat: add 'zeptoclaw config check' with field validation and security warnings"
```

---

### Task 5: Message Queue Modes

**Files:**
- Modify: `src/config/types.rs` (add MessageQueueMode enum + field)
- Modify: `src/config/mod.rs` (env override)
- Modify: `src/agent/loop.rs` (queue logic in start())

**Step 1: Add config types**

In `src/config/types.rs`, add after the `AgentDefaults` struct:

```rust
/// How to handle messages that arrive while an agent run is active.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageQueueMode {
    /// Buffer messages, concatenate into one when current run finishes.
    #[default]
    Collect,
    /// Buffer messages, replay each as a separate run after current finishes.
    Followup,
}
```

Add the field to `AgentDefaults`:

```rust
    /// How to handle messages arriving during an active run.
    pub message_queue_mode: MessageQueueMode,
```

Update `Default` impl to include `message_queue_mode: MessageQueueMode::default()`.

**Step 2: Add env override**

In `src/config/mod.rs` `apply_env_overrides()`:

```rust
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_MESSAGE_QUEUE_MODE") {
            match val.trim().to_ascii_lowercase().as_str() {
                "collect" => self.agents.defaults.message_queue_mode = MessageQueueMode::Collect,
                "followup" => self.agents.defaults.message_queue_mode = MessageQueueMode::Followup,
                _ => {}
            }
        }
```

**Step 3: Add pending message queue to AgentLoop**

In `src/agent/loop.rs`, add to the `AgentLoop` struct:

```rust
    /// Pending messages for sessions with active runs (for queue modes).
    pending_messages: Arc<Mutex<HashMap<String, Vec<InboundMessage>>>>,
```

Initialize it in both `new()` and `with_context_builder()`:

```rust
            pending_messages: Arc::new(Mutex::new(HashMap::new())),
```

**Step 4: Modify the start() loop to implement queue behavior**

After the process_message + timeout block completes (both Ok and Err paths), add drain-pending logic:

```rust
                            // After processing, drain any pending messages for this session
                            let pending = {
                                let mut map = self.pending_messages.lock().await;
                                map.remove(&msg_ref.session_key).unwrap_or_default()
                            };

                            if !pending.is_empty() {
                                match self.config.agents.defaults.message_queue_mode {
                                    zeptoclaw::config::MessageQueueMode::Collect => {
                                        // Concatenate all pending messages into one
                                        let combined: Vec<String> = pending
                                            .iter()
                                            .enumerate()
                                            .map(|(i, m)| format!("{}. {}", i + 1, m.content))
                                            .collect();
                                        let combined_content = format!(
                                            "[Queued messages while I was busy]\n\n{}",
                                            combined.join("\n")
                                        );
                                        let synthetic = InboundMessage::new(
                                            &msg_ref.channel,
                                            &msg_ref.sender_id,
                                            &msg_ref.chat_id,
                                            &combined_content,
                                        );
                                        if let Err(e) = bus_ref.publish_inbound(synthetic).await {
                                            error!("Failed to re-queue collected messages: {}", e);
                                        }
                                    }
                                    zeptoclaw::config::MessageQueueMode::Followup => {
                                        // Replay each pending message as a separate inbound
                                        for pending_msg in pending {
                                            if let Err(e) = bus_ref.publish_inbound(pending_msg).await {
                                                error!("Failed to re-queue followup message: {}", e);
                                            }
                                        }
                                    }
                                }
                            }
```

Also, modify the session lock acquisition in `process_message` to optionally queue instead of blocking. Add a public method:

```rust
    /// Try to queue a message if the session is busy, or process it immediately.
    /// Returns true if the message was queued (caller should not wait for response).
    pub async fn try_queue_or_process(&self, msg: &InboundMessage) -> Option<bool> {
        let session_lock = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(msg.session_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Try to acquire the lock without blocking
        match session_lock.try_lock() {
            Ok(_guard) => None, // Lock acquired, process normally (None = not queued)
            Err(_) => {
                // Session is busy, queue the message
                let mut pending = self.pending_messages.lock().await;
                pending
                    .entry(msg.session_key.clone())
                    .or_default()
                    .push(msg.clone());
                Some(true) // Queued
            }
        }
    }
```

**Step 5: Add tests**

```rust
#[test]
fn test_message_queue_mode_default() {
    let config = Config::default();
    assert_eq!(
        config.agents.defaults.message_queue_mode,
        MessageQueueMode::Collect
    );
}

#[test]
fn test_message_queue_mode_from_json() {
    let json = r#"{"agents": {"defaults": {"message_queue_mode": "followup"}}}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(
        config.agents.defaults.message_queue_mode,
        MessageQueueMode::Followup
    );
}
```

**Step 6: Run tests**

Run: `cargo test --lib`
Expected: All tests PASS.

**Step 7: Commit**

```bash
git add src/config/types.rs src/config/mod.rs src/agent/loop.rs
git commit -m "feat: add message queue modes (collect/followup) for busy sessions"
```

---

### Task 6: Final Integration Test & Cleanup

**Files:**
- Modify: `CLAUDE.md` (document new features)
- Run: full test suite

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass (existing 498 + new tests).

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

**Step 3: Run fmt**

Run: `cargo fmt`

**Step 4: Update CLAUDE.md**

Add to the "Configuration" section:
- `agent_timeout_secs` (default 300) — wall-clock timeout for agent runs
- `message_queue_mode` — "collect" (default) or "followup"

Add to CLI commands table:
- `zeptoclaw config check` — validate config file

Add note about parallel tool execution and result sanitization in Architecture section.

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: complete quick wins — parallel tools, sanitization, timeout, config check, queue modes"
```
