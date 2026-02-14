//! Hook system for ZeptoClaw agent loop.
//!
//! Config-driven hooks that fire at specific points in the agent loop:
//!
//! - `before_tool` — before tool execution (can log or block)
//! - `after_tool` — after tool execution (can log)
//! - `on_error` — when a tool fails (can log)
//!
//! # Configuration
//!
//! ```json
//! {
//!     "hooks": {
//!         "enabled": true,
//!         "before_tool": [
//!             { "action": "log", "tools": ["shell"], "level": "warn" },
//!             { "action": "block", "tools": ["shell"], "channels": ["telegram"], "message": "Shell disabled on Telegram" }
//!         ],
//!         "after_tool": [
//!             { "action": "log", "tools": ["*"], "level": "info" }
//!         ],
//!         "on_error": [
//!             { "action": "log", "level": "error" }
//!         ]
//!     }
//! }
//! ```
//!
//! # Example
//!
//! ```rust
//! use zeptoclaw::hooks::{HooksConfig, HookEngine, HookResult, HookAction, HookRule};
//!
//! let config = HooksConfig {
//!     enabled: true,
//!     before_tool: vec![HookRule {
//!         action: HookAction::Block,
//!         tools: vec!["shell".to_string()],
//!         channels: vec!["telegram".to_string()],
//!         message: Some("Shell disabled on Telegram".to_string()),
//!         ..Default::default()
//!     }],
//!     ..Default::default()
//! };
//! let engine = HookEngine::new(config);
//! let result = engine.before_tool("shell", &serde_json::json!({}), "telegram");
//! assert!(matches!(result, HookResult::Block(_)));
//! ```

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Hook action enum
// ---------------------------------------------------------------------------

/// What a hook rule does when triggered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAction {
    /// Log the event via tracing.
    Log,
    /// Block the tool from executing (before_tool only).
    Block,
    /// Send a notification to a channel (logged for now).
    Notify,
}

// ---------------------------------------------------------------------------
// Hook rule
// ---------------------------------------------------------------------------

/// A single hook rule that matches tool calls and performs an action.
///
/// Rules are evaluated in order. For `before_tool`, the first `Block` rule
/// that matches wins. `Log` rules always execute (no short-circuit).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookRule {
    /// Action to perform.
    pub action: HookAction,
    /// Tool names to match. `["*"]` matches all tools. Empty = match none.
    pub tools: Vec<String>,
    /// Channel names to match. Empty = match all channels.
    pub channels: Vec<String>,
    /// Log level for `Log` action (trace/debug/info/warn/error).
    pub level: Option<String>,
    /// Custom message for `Block` action.
    pub message: Option<String>,
    /// Target channel name for `Notify` action (future use).
    pub channel: Option<String>,
    /// Target chat ID for `Notify` action (future use).
    pub chat_id: Option<String>,
}

impl Default for HookRule {
    fn default() -> Self {
        Self {
            action: HookAction::Log,
            tools: vec![],
            channels: vec![],
            level: None,
            message: None,
            channel: None,
            chat_id: None,
        }
    }
}

impl HookRule {
    /// Check if this rule matches the given tool name.
    pub fn matches_tool(&self, tool_name: &str) -> bool {
        self.tools.iter().any(|t| t == "*" || t == tool_name)
    }

    /// Check if this rule matches the given channel name.
    /// Empty channels list means match all.
    pub fn matches_channel(&self, channel_name: &str) -> bool {
        self.channels.is_empty() || self.channels.iter().any(|c| c == "*" || c == channel_name)
    }
}

// ---------------------------------------------------------------------------
// Hooks config
// ---------------------------------------------------------------------------

/// Hooks configuration for `config.json`.
///
/// Controls the hook system that fires at specific points in the agent loop.
///
/// # Defaults
///
/// - `enabled`: `false`
/// - All rule lists: empty
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Master switch for hooks.
    pub enabled: bool,
    /// Rules evaluated before each tool execution.
    pub before_tool: Vec<HookRule>,
    /// Rules evaluated after each tool execution.
    pub after_tool: Vec<HookRule>,
    /// Rules evaluated when a tool returns an error.
    pub on_error: Vec<HookRule>,
}

// ---------------------------------------------------------------------------
// Hook result
// ---------------------------------------------------------------------------

/// Result of evaluating before_tool hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResult {
    /// Allow the tool to execute.
    Continue,
    /// Block the tool with the given message.
    Block(String),
}

// ---------------------------------------------------------------------------
// Hook engine
// ---------------------------------------------------------------------------

/// Runtime hook engine that evaluates rules from HooksConfig.
///
/// Created once per agent loop iteration and called at 3 points:
/// 1. `before_tool` — before approval gate + tool execution
/// 2. `after_tool` — after successful tool execution
/// 3. `on_error` — after failed tool execution
pub struct HookEngine {
    config: HooksConfig,
}

impl HookEngine {
    /// Create a new HookEngine from configuration.
    pub fn new(config: HooksConfig) -> Self {
        Self { config }
    }

    /// Evaluate before_tool hooks. Returns Block if any matching rule blocks.
    ///
    /// Rules are evaluated in order. `Log` rules execute without stopping.
    /// The first `Block` rule that matches returns immediately.
    pub fn before_tool(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
        channel: &str,
    ) -> HookResult {
        if !self.config.enabled {
            return HookResult::Continue;
        }

        for rule in &self.config.before_tool {
            if !rule.matches_tool(tool_name) || !rule.matches_channel(channel) {
                continue;
            }

            match rule.action {
                HookAction::Log => {
                    let level = rule.level.as_deref().unwrap_or("info");
                    match level {
                        "error" => tracing::error!(
                            hook = "before_tool",
                            tool = tool_name,
                            channel = channel,
                            "Hook: tool call"
                        ),
                        "warn" => tracing::warn!(
                            hook = "before_tool",
                            tool = tool_name,
                            channel = channel,
                            "Hook: tool call"
                        ),
                        "debug" => tracing::debug!(
                            hook = "before_tool",
                            tool = tool_name,
                            channel = channel,
                            "Hook: tool call"
                        ),
                        "trace" => tracing::trace!(
                            hook = "before_tool",
                            tool = tool_name,
                            channel = channel,
                            "Hook: tool call"
                        ),
                        _ => tracing::info!(
                            hook = "before_tool",
                            tool = tool_name,
                            channel = channel,
                            "Hook: tool call"
                        ),
                    }
                }
                HookAction::Block => {
                    let msg = rule
                        .message
                        .clone()
                        .unwrap_or_else(|| format!("Tool '{}' blocked by hook", tool_name));
                    tracing::info!(
                        hook = "before_tool",
                        tool = tool_name,
                        channel = channel,
                        "Hook: blocking tool"
                    );
                    return HookResult::Block(msg);
                }
                HookAction::Notify => {
                    tracing::info!(
                        hook = "before_tool",
                        tool = tool_name,
                        channel = channel,
                        "Hook: notify (logged)"
                    );
                }
            }
        }

        HookResult::Continue
    }

    /// Evaluate after_tool hooks (logging only, no blocking).
    pub fn after_tool(
        &self,
        tool_name: &str,
        _result: &str,
        elapsed: std::time::Duration,
        channel: &str,
    ) {
        if !self.config.enabled {
            return;
        }

        for rule in &self.config.after_tool {
            if !rule.matches_tool(tool_name) || !rule.matches_channel(channel) {
                continue;
            }

            match rule.action {
                HookAction::Log => {
                    let ms = elapsed.as_millis();
                    let level = rule.level.as_deref().unwrap_or("info");
                    match level {
                        "error" => {
                            tracing::error!(hook = "after_tool", tool = tool_name, latency_ms = %ms, "Hook: tool completed")
                        }
                        "warn" => {
                            tracing::warn!(hook = "after_tool", tool = tool_name, latency_ms = %ms, "Hook: tool completed")
                        }
                        "debug" => {
                            tracing::debug!(hook = "after_tool", tool = tool_name, latency_ms = %ms, "Hook: tool completed")
                        }
                        _ => {
                            tracing::info!(hook = "after_tool", tool = tool_name, latency_ms = %ms, "Hook: tool completed")
                        }
                    }
                }
                HookAction::Block => {} // Block is a no-op in after_tool
                HookAction::Notify => {
                    tracing::info!(
                        hook = "after_tool",
                        tool = tool_name,
                        "Hook: notify (logged)"
                    );
                }
            }
        }
    }

    /// Evaluate on_error hooks (logging only, no blocking).
    pub fn on_error(&self, tool_name: &str, error: &str, channel: &str) {
        if !self.config.enabled {
            return;
        }

        for rule in &self.config.on_error {
            if !rule.matches_tool(tool_name) || !rule.matches_channel(channel) {
                continue;
            }

            match rule.action {
                HookAction::Log => {
                    let level = rule.level.as_deref().unwrap_or("error");
                    match level {
                        "warn" => tracing::warn!(
                            hook = "on_error",
                            tool = tool_name,
                            error = error,
                            "Hook: tool error"
                        ),
                        "debug" => tracing::debug!(
                            hook = "on_error",
                            tool = tool_name,
                            error = error,
                            "Hook: tool error"
                        ),
                        _ => tracing::error!(
                            hook = "on_error",
                            tool = tool_name,
                            error = error,
                            "Hook: tool error"
                        ),
                    }
                }
                HookAction::Block => {} // Block is a no-op in on_error
                HookAction::Notify => {
                    tracing::info!(
                        hook = "on_error",
                        tool = tool_name,
                        error = error,
                        "Hook: error notify (logged)"
                    );
                }
            }
        }
    }

    /// Whether hooks are enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- HooksConfig defaults ----

    #[test]
    fn test_hooks_config_default() {
        let config = HooksConfig::default();
        assert!(!config.enabled);
        assert!(config.before_tool.is_empty());
        assert!(config.after_tool.is_empty());
        assert!(config.on_error.is_empty());
    }

    #[test]
    fn test_hooks_config_deserialize() {
        let json = r#"{
            "enabled": true,
            "before_tool": [
                { "action": "log", "tools": ["shell"], "level": "warn" }
            ]
        }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.before_tool.len(), 1);
        assert_eq!(config.before_tool[0].action, HookAction::Log);
    }

    #[test]
    fn test_hooks_config_serialization_roundtrip() {
        let config = HooksConfig {
            enabled: true,
            before_tool: vec![HookRule {
                action: HookAction::Block,
                tools: vec!["shell".to_string()],
                channels: vec!["telegram".to_string()],
                message: Some("blocked".to_string()),
                ..Default::default()
            }],
            after_tool: vec![HookRule {
                action: HookAction::Log,
                tools: vec!["*".to_string()],
                ..Default::default()
            }],
            on_error: vec![],
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: HooksConfig = serde_json::from_str(&json).unwrap();
        assert!(deserialized.enabled);
        assert_eq!(deserialized.before_tool.len(), 1);
        assert_eq!(deserialized.after_tool.len(), 1);
    }

    // ---- HookRule matching ----

    #[test]
    fn test_hook_rule_matches_tool() {
        let rule = HookRule {
            tools: vec!["shell".to_string()],
            ..Default::default()
        };
        assert!(rule.matches_tool("shell"));
        assert!(!rule.matches_tool("echo"));
    }

    #[test]
    fn test_hook_rule_wildcard_matches_all() {
        let rule = HookRule {
            tools: vec!["*".to_string()],
            ..Default::default()
        };
        assert!(rule.matches_tool("shell"));
        assert!(rule.matches_tool("echo"));
        assert!(rule.matches_tool("anything"));
    }

    #[test]
    fn test_hook_rule_empty_tools_matches_none() {
        let rule = HookRule::default();
        assert!(!rule.matches_tool("shell"));
        assert!(!rule.matches_tool("anything"));
    }

    #[test]
    fn test_hook_rule_matches_channel() {
        let rule = HookRule {
            channels: vec!["telegram".to_string()],
            ..Default::default()
        };
        assert!(rule.matches_channel("telegram"));
        assert!(!rule.matches_channel("discord"));
    }

    #[test]
    fn test_hook_rule_empty_channels_matches_all() {
        let rule = HookRule::default();
        assert!(rule.matches_channel("telegram"));
        assert!(rule.matches_channel("discord"));
        assert!(rule.matches_channel("cli"));
    }

    #[test]
    fn test_hook_rule_channel_wildcard() {
        let rule = HookRule {
            channels: vec!["*".to_string()],
            ..Default::default()
        };
        assert!(rule.matches_channel("telegram"));
        assert!(rule.matches_channel("cli"));
    }

    // ---- HookEngine ----

    #[test]
    fn test_hook_engine_disabled_does_nothing() {
        let config = HooksConfig::default();
        let engine = HookEngine::new(config);
        let result = engine.before_tool("shell", &serde_json::json!({}), "telegram");
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_hook_engine_before_tool_log() {
        let config = HooksConfig {
            enabled: true,
            before_tool: vec![HookRule {
                action: HookAction::Log,
                tools: vec!["shell".to_string()],
                level: Some("warn".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let engine = HookEngine::new(config);
        let result = engine.before_tool("shell", &serde_json::json!({"cmd": "ls"}), "cli");
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_hook_engine_before_tool_block() {
        let config = HooksConfig {
            enabled: true,
            before_tool: vec![HookRule {
                action: HookAction::Block,
                tools: vec!["shell".to_string()],
                channels: vec!["telegram".to_string()],
                message: Some("Shell disabled on Telegram".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let engine = HookEngine::new(config);

        // Should block shell on telegram
        let result = engine.before_tool("shell", &serde_json::json!({}), "telegram");
        assert!(matches!(result, HookResult::Block(_)));
        if let HookResult::Block(msg) = result {
            assert_eq!(msg, "Shell disabled on Telegram");
        }

        // Should NOT block shell on CLI
        let result = engine.before_tool("shell", &serde_json::json!({}), "cli");
        assert_eq!(result, HookResult::Continue);

        // Should NOT block echo on telegram
        let result = engine.before_tool("echo", &serde_json::json!({}), "telegram");
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_hook_engine_before_tool_block_default_message() {
        let config = HooksConfig {
            enabled: true,
            before_tool: vec![HookRule {
                action: HookAction::Block,
                tools: vec!["shell".to_string()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let engine = HookEngine::new(config);
        let result = engine.before_tool("shell", &serde_json::json!({}), "cli");
        if let HookResult::Block(msg) = result {
            assert!(msg.contains("shell"));
            assert!(msg.contains("blocked by hook"));
        } else {
            panic!("Expected Block");
        }
    }

    #[test]
    fn test_hook_engine_multiple_rules_first_block_wins() {
        let config = HooksConfig {
            enabled: true,
            before_tool: vec![
                HookRule {
                    action: HookAction::Log,
                    tools: vec!["*".to_string()],
                    level: Some("info".to_string()),
                    ..Default::default()
                },
                HookRule {
                    action: HookAction::Block,
                    tools: vec!["shell".to_string()],
                    message: Some("blocked".to_string()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let engine = HookEngine::new(config);
        let result = engine.before_tool("shell", &serde_json::json!({}), "cli");
        assert!(matches!(result, HookResult::Block(_)));
    }

    #[test]
    fn test_hook_engine_after_tool() {
        let config = HooksConfig {
            enabled: true,
            after_tool: vec![HookRule {
                action: HookAction::Log,
                tools: vec!["*".to_string()],
                level: Some("info".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let engine = HookEngine::new(config);
        engine.after_tool(
            "shell",
            "result text",
            std::time::Duration::from_millis(50),
            "cli",
        );
    }

    #[test]
    fn test_hook_engine_on_error() {
        let config = HooksConfig {
            enabled: true,
            on_error: vec![HookRule {
                action: HookAction::Log,
                tools: vec!["*".to_string()],
                level: Some("error".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let engine = HookEngine::new(config);
        engine.on_error("shell", "command not found", "cli");
    }

    #[test]
    fn test_hook_engine_is_enabled() {
        let enabled = HookEngine::new(HooksConfig {
            enabled: true,
            ..Default::default()
        });
        assert!(enabled.is_enabled());

        let disabled = HookEngine::new(HooksConfig::default());
        assert!(!disabled.is_enabled());
    }
}
