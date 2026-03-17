//! Configuration validation with unknown field detection.

use serde_json::Value;
use std::collections::HashSet;

/// Known top-level config field names.
const KNOWN_TOP_LEVEL: &[&str] = &[
    "agents",
    "channels",
    "providers",
    "gateway",
    "tools",
    "memory",
    "heartbeat",
    "skills",
    "runtime",
    "container_agent",
    "swarm",
    "approval",
    "plugins",
    "telemetry",
    "cost",
    "batch",
    "hooks",
    "safety",
    "compaction",
    "mcp",
    "routines",
    "tunnel",
    "stripe",
    "custom_tools",
    "transcription",
    "tool_profiles",
    "project",
    "cache",
    "agent_mode",
    "pairing",
    "session",
    "panel",
    "health",
    "devices",
    "logging",
    "r8r_bridge",
];

/// Known fields for each section. Nested as section.field.
const KNOWN_AGENTS_DEFAULTS: &[&str] = &[
    "workspace",
    "model",
    "max_tokens",
    "temperature",
    "max_tool_iterations",
    "agent_timeout_secs",
    "tool_timeout_secs",
    "message_queue_mode",
    "streaming",
    "token_budget",
    "compact_tools",
    "tool_profile",
    "active_hand",
    "timezone",
    "loop_guard",
    "max_tool_result_bytes",
    "max_tool_calls",
    "system_prompt",
];

const KNOWN_LOOP_GUARD: &[&str] = &[
    "enabled",
    "warn_threshold",
    "block_threshold",
    "global_circuit_breaker",
    "ping_pong_min_repeats",
    "poll_multiplier",
    "outcome_warn_threshold",
    "outcome_block_threshold",
    "window_size",
];

#[allow(dead_code)]
const KNOWN_GATEWAY: &[&str] = &["host", "port", "rate_limit", "startup_guard"];

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

    for (i, row) in matrix.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for (j, val) in matrix[0].iter_mut().enumerate().take(b_len + 1) {
        *val = j;
    }

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
            let suggestion = suggest_field(key, KNOWN_TOP_LEVEL).unwrap_or_default();
            let msg = if suggestion.is_empty() {
                format!("Unknown field '{}'", key)
            } else {
                format!("Unknown field '{}' \u{2014} {}", key, suggestion)
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
                    let suggestion = suggest_field(key, KNOWN_AGENTS_DEFAULTS).unwrap_or_default();
                    let msg = if suggestion.is_empty() {
                        format!("Unknown field '{}'", key)
                    } else {
                        format!("Unknown field '{}' \u{2014} {}", key, suggestion)
                    };
                    diagnostics.push(Diagnostic {
                        level: DiagnosticLevel::Error,
                        path: format!("agents.defaults.{}", key),
                        message: msg,
                    });
                }
            }

            // Check agents.defaults.loop_guard nested keys
            if let Some(lg) = defaults.get("loop_guard").and_then(|v| v.as_object()) {
                let lg_known: HashSet<&str> = KNOWN_LOOP_GUARD.iter().copied().collect();
                for key in lg.keys() {
                    if !lg_known.contains(key.as_str()) {
                        has_unknown = true;
                        let suggestion = suggest_field(key, KNOWN_LOOP_GUARD).unwrap_or_default();
                        let msg = if suggestion.is_empty() {
                            format!("Unknown field '{}'", key)
                        } else {
                            format!("Unknown field '{}' \u{2014} {}", key, suggestion)
                        };
                        diagnostics.push(Diagnostic {
                            level: DiagnosticLevel::Warn,
                            path: format!("agents.defaults.loop_guard.{}", key),
                            message: msg,
                        });
                    }
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
                        message: "Empty \u{2014} anyone can message the bot".to_string(),
                    });
                }

                if name == "email"
                    && enabled
                    && channel_obj
                        .get("allowed_senders")
                        .and_then(|v| v.as_array())
                        .is_some_and(|senders| !senders.is_empty())
                {
                    diagnostics.push(Diagnostic {
                        level: DiagnosticLevel::Warn,
                        path: "channels.email.allowed_senders".to_string(),
                        message: "Email sender allowlists trust the parsed From header only; enforce SPF/DKIM/DMARC upstream if sender authenticity matters".to_string(),
                    });
                }

                if name == "telegram" {
                    let allow_usernames = channel_obj
                        .get("allow_usernames")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    let has_username_entries = channel_obj
                        .get("allow_from")
                        .and_then(|v| v.as_array())
                        .map(|entries| {
                            entries
                                .iter()
                                .filter_map(|entry| entry.as_str())
                                .any(|entry| {
                                    let trimmed = entry.trim();
                                    trimmed.is_empty()
                                        || !trimmed.bytes().all(|b| b.is_ascii_digit())
                                })
                        })
                        .unwrap_or(false);

                    if has_username_entries {
                        diagnostics.push(Diagnostic {
                            level: DiagnosticLevel::Warn,
                            path: "channels.telegram.allow_from".to_string(),
                            message: if allow_usernames {
                                "Telegram username allowlist entries are legacy and can be reassigned; prefer numeric user IDs and set allow_usernames=false after migration".to_string()
                            } else {
                                "Telegram allow_from contains non-numeric entries, but allow_usernames=false so they will never match".to_string()
                            },
                        });
                    }
                }
            }
        }
    }

    // r8r bridge warnings
    if let Some(r8r) = obj.get("r8r_bridge").and_then(|v| v.as_object()) {
        let enabled = r8r
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if enabled {
            if let Some(endpoint) = r8r.get("endpoint").and_then(|v| v.as_str()) {
                if endpoint.starts_with("ws://")
                    && !endpoint.contains("localhost")
                    && !endpoint.contains("127.0.0.1")
                    && !endpoint.contains("[::1]")
                {
                    diagnostics.push(Diagnostic {
                        level: DiagnosticLevel::Warn,
                        path: "r8r_bridge.endpoint".to_string(),
                        message: "Plaintext ws:// on non-loopback address \u{2014} bearer token will be sent in cleartext; use wss://".to_string(),
                    });
                }
            }
        }
    }

    diagnostics
}

/// Check if a model name looks compatible with a provider backend.
///
/// Returns `None` when the combination is fine, or `Some(message)` describing
/// the mismatch.
fn check_model_backend_compat(model: &str, provider_name: &str, backend: &str) -> Option<String> {
    let m = model.to_lowercase();

    match backend {
        "anthropic" => {
            // Claude models always start with "claude-"
            if !m.starts_with("claude-") {
                Some(format!(
                    "model '{}' does not look like a Claude model (expected 'claude-*') \
                     but provider '{}' uses the Anthropic API",
                    model, provider_name,
                ))
            } else {
                None
            }
        }
        "openai" => {
            // OpenAI-compatible providers accept many model formats, but a
            // `claude-*` model is almost certainly wrong for the OpenAI backend.
            if m.starts_with("claude-") && provider_name == "openai" {
                Some(format!(
                    "model '{}' looks like a Claude model but provider '{}' uses the OpenAI API",
                    model, provider_name,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Validate that the configured default model is compatible with the
/// resolved primary provider.
///
/// Also validates per-provider model overrides.
pub fn validate_model_provider_compat(config: &crate::config::Config) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let default_model = &config.agents.defaults.model;

    // Use the provider registry to figure out which providers are actually
    // resolved at runtime (have credentials).
    let selections = crate::providers::resolve_runtime_providers(config);

    if selections.is_empty() {
        diags.push(Diagnostic {
            level: DiagnosticLevel::Warn,
            path: "providers".to_string(),
            message: "No AI provider resolved \u{2014} set an API key or run 'zeptoclaw onboard'"
                .to_string(),
        });
        return diags;
    }

    // Check default model against primary (first resolved) provider.
    let primary = &selections[0];
    if let Some(msg) = check_model_backend_compat(default_model, primary.name, primary.backend) {
        // If the provider has a per-provider model override, the default
        // model mismatch is just a warning (per-provider model takes
        // precedence at runtime).
        let level = if primary.model.is_some() {
            DiagnosticLevel::Warn
        } else {
            DiagnosticLevel::Error
        };

        let mut message = msg;
        if let Some(ref per_model) = primary.model {
            message.push_str(&format!(
                " (per-provider model '{}' will be used instead)",
                per_model
            ));
        } else {
            message.push_str(
                ". Fix: set 'agents.defaults.model' to a compatible model, \
                 or add 'providers.<name>.model' override",
            );
        }

        diags.push(Diagnostic {
            level,
            path: "agents.defaults.model".to_string(),
            message,
        });
    }

    // Check per-provider model overrides against their own backend.
    for sel in &selections {
        if let Some(ref per_model) = sel.model {
            if let Some(msg) = check_model_backend_compat(per_model, sel.name, sel.backend) {
                diags.push(Diagnostic {
                    level: DiagnosticLevel::Error,
                    path: format!("providers.{}.model", sel.name),
                    message: msg,
                });
            }
        }
    }

    if diags.is_empty() {
        diags.push(Diagnostic {
            level: DiagnosticLevel::Ok,
            path: String::new(),
            message: format!(
                "Model '{}' compatible with primary provider '{}'",
                default_model, primary.name
            ),
        });
    }

    diags
}

/// Validate custom tool definitions.
pub fn validate_custom_tools(config: &crate::config::Config) -> Vec<String> {
    let mut warnings = Vec::new();
    let name_re = regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9_]*$").unwrap();

    // Built-in tool names to check for conflicts
    let builtin_names: HashSet<&str> = [
        "echo",
        "read_file",
        "write_file",
        "list_dir",
        "edit_file",
        "shell",
        "web_search",
        "web_fetch",
        "message",
        "memory_search",
        "memory_get",
        "longterm_memory",
        "whatsapp_send",
        "google_sheets",
        "cron",
        "spawn",
        "delegate",
        "r8r",
    ]
    .iter()
    .copied()
    .collect();

    for (i, tool) in config.custom_tools.iter().enumerate() {
        if !name_re.is_match(&tool.name) {
            warnings.push(format!(
                "custom_tools[{}]: name '{}' invalid — must start with a letter and contain only letters, digits, and underscores",
                i, tool.name
            ));
        }
        if builtin_names.contains(tool.name.as_str()) {
            warnings.push(format!(
                "custom_tools[{}]: name '{}' conflicts with built-in tool",
                i, tool.name
            ));
        }
        if tool.command.trim().is_empty() {
            warnings.push(format!("custom_tools[{}]: command must not be empty", i));
        }
        if tool.description.len() > 60 {
            warnings.push(format!(
                "custom_tools[{}]: description exceeds 60 chars ({}). Shorter descriptions save tokens.",
                i,
                tool.description.len()
            ));
        }
    }
    warnings
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, CustomToolDef};
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
    fn test_validate_accepts_tunnel_r8r_bridge_and_agent_default_fields() {
        let raw = json!({
            "agents": {
                "defaults": {
                    "model": "gpt-4",
                    "timezone": "Asia/Kuala_Lumpur",
                    "tool_timeout_secs": 30,
                    "system_prompt": "You are a workflow assistant."
                }
            },
            "r8r_bridge": {
                "enabled": true,
                "endpoint": "ws://localhost:8080/api/ws/events"
            },
            "tunnel": {
                "provider": "cloudflare"
            }
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
    fn test_validate_email_allowed_senders_warns_header_only_trust() {
        let raw = json!({
            "channels": {
                "email": {
                    "enabled": true,
                    "imap_host": "imap.example.com",
                    "smtp_host": "smtp.example.com",
                    "username": "bot@example.com",
                    "password": "secret",
                    "allowed_senders": ["@example.com"]
                }
            }
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| {
            d.level == DiagnosticLevel::Warn
                && d.path == "channels.email.allowed_senders"
                && d.message.contains("From header")
        }));
    }

    #[test]
    fn test_validate_telegram_username_allowlist_warns() {
        let raw = json!({
            "channels": {
                "telegram": {
                    "enabled": true,
                    "token": "abc",
                    "allow_from": ["alice"]
                }
            }
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| {
            d.level == DiagnosticLevel::Warn
                && d.path == "channels.telegram.allow_from"
                && d.message.contains("legacy")
        }));
    }

    #[test]
    fn test_validate_telegram_username_allowlist_disabled_warns_non_matching() {
        let raw = json!({
            "channels": {
                "telegram": {
                    "enabled": true,
                    "token": "abc",
                    "allow_from": ["alice"],
                    "allow_usernames": false
                }
            }
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| {
            d.level == DiagnosticLevel::Warn
                && d.path == "channels.telegram.allow_from"
                && d.message.contains("never match")
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

    #[test]
    fn test_validate_custom_tools_valid() {
        let mut config = Config::default();
        config.custom_tools = vec![CustomToolDef {
            name: "cpu_temp".to_string(),
            description: "Read CPU temp".to_string(),
            command: "cat /sys/class/thermal/thermal_zone0/temp".to_string(),
            parameters: None,
            working_dir: None,
            timeout_secs: None,
            env: None,
        }];
        let warnings = validate_custom_tools(&config);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_validate_custom_tool_name_invalid() {
        let mut config = Config::default();
        config.custom_tools = vec![CustomToolDef {
            name: "123bad".to_string(),
            description: "Bad".to_string(),
            command: "echo".to_string(),
            parameters: None,
            working_dir: None,
            timeout_secs: None,
            env: None,
        }];
        let warnings = validate_custom_tools(&config);
        assert!(warnings.iter().any(|w| w.contains("invalid")));
    }

    #[test]
    fn test_validate_custom_tool_name_builtin_conflict() {
        let mut config = Config::default();
        config.custom_tools = vec![CustomToolDef {
            name: "shell".to_string(),
            description: "Conflict".to_string(),
            command: "echo".to_string(),
            parameters: None,
            working_dir: None,
            timeout_secs: None,
            env: None,
        }];
        let warnings = validate_custom_tools(&config);
        assert!(warnings.iter().any(|w| w.contains("conflicts")));
    }

    #[test]
    fn test_validate_custom_tool_empty_command() {
        let mut config = Config::default();
        config.custom_tools = vec![CustomToolDef {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            command: "  ".to_string(),
            parameters: None,
            working_dir: None,
            timeout_secs: None,
            env: None,
        }];
        let warnings = validate_custom_tools(&config);
        assert!(warnings.iter().any(|w| w.contains("empty")));
    }

    #[test]
    fn test_validate_custom_tool_long_description() {
        let mut config = Config::default();
        config.custom_tools = vec![CustomToolDef {
            name: "verbose_tool".to_string(),
            description: "A".repeat(61),
            command: "echo hi".to_string(),
            parameters: None,
            working_dir: None,
            timeout_secs: None,
            env: None,
        }];
        let warnings = validate_custom_tools(&config);
        assert!(warnings.iter().any(|w| w.contains("60 chars")));
    }

    #[test]
    fn test_validate_compact_tools_known() {
        let json = json!({"agents": {"defaults": {"compact_tools": true}}});
        let diags = validate_config(&json);
        assert!(!diags.iter().any(|d| d.message.contains("compact_tools")));
    }

    #[test]
    fn test_validate_loop_guard_known_keys() {
        let json = json!({
            "agents": {"defaults": {"loop_guard": {
                "enabled": true,
                "warn_threshold": 3,
                "block_threshold": 5,
                "global_circuit_breaker": 30,
                "ping_pong_min_repeats": 3,
                "poll_multiplier": 3,
                "outcome_warn_threshold": 2,
                "outcome_block_threshold": 3
            }}}
        });
        let diags = validate_config(&json);
        assert!(
            !diags.iter().any(|d| d.path.contains("loop_guard")),
            "Expected no loop_guard diagnostics, got: {:?}",
            diags
                .iter()
                .filter(|d| d.path.contains("loop_guard"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_validate_loop_guard_unknown_key() {
        let json = json!({
            "agents": {"defaults": {"loop_guard": {
                "enabled": true,
                "max_retries": 5
            }}}
        });
        let diags = validate_config(&json);
        assert!(
            diags.iter().any(|d| {
                d.path.contains("loop_guard.max_retries") && d.level == DiagnosticLevel::Warn
            }),
            "Expected warning for unknown loop_guard key 'max_retries'"
        );
    }

    #[test]
    fn test_validate_loop_guard_typo_suggestion() {
        let json = json!({
            "agents": {"defaults": {"loop_guard": {
                "enbled": true
            }}}
        });
        let diags = validate_config(&json);
        assert!(
            diags
                .iter()
                .any(|d| { d.path.contains("loop_guard.enbled") && d.message.contains("enabled") }),
            "Expected suggestion for typo 'enbled'"
        );
    }

    // --- Model-provider compatibility unit tests ---

    #[test]
    fn test_compat_claude_model_on_anthropic() {
        let result =
            check_model_backend_compat("claude-sonnet-4-5-20250929", "anthropic", "anthropic");
        assert!(result.is_none(), "Claude model should be fine on anthropic");
    }

    #[test]
    fn test_compat_gpt_model_on_anthropic() {
        let result = check_model_backend_compat("gpt-5.1-2025-11-13", "anthropic", "anthropic");
        assert!(
            result.is_some(),
            "GPT model should NOT be compatible with anthropic"
        );
        assert!(result
            .unwrap()
            .contains("does not look like a Claude model"));
    }

    #[test]
    fn test_compat_gpt_model_on_openai() {
        let result = check_model_backend_compat("gpt-5.1", "openai", "openai");
        assert!(result.is_none(), "GPT model should be fine on openai");
    }

    #[test]
    fn test_compat_claude_model_on_openai() {
        let result = check_model_backend_compat("claude-sonnet-4-5-20250929", "openai", "openai");
        assert!(
            result.is_some(),
            "Claude model should NOT be compatible with openai"
        );
    }

    #[test]
    fn test_compat_custom_model_on_groq() {
        // Groq uses openai backend but has custom model names
        let result = check_model_backend_compat("llama-3.3-70b", "groq", "openai");
        assert!(
            result.is_none(),
            "Custom model should be fine on OpenAI-compat provider"
        );
    }

    #[test]
    fn test_compat_case_insensitive() {
        let result = check_model_backend_compat("Claude-Sonnet-4-5", "anthropic", "anthropic");
        assert!(result.is_none(), "Case-insensitive match should work");
    }

    #[test]
    fn test_compat_openrouter_auto_on_openai() {
        let result = check_model_backend_compat("openrouter/auto", "openrouter", "openai");
        assert!(result.is_none(), "openrouter/auto should be fine");
    }

    #[test]
    fn test_validate_r8r_bridge_ws_plaintext_non_loopback_warns() {
        let raw = json!({
            "r8r_bridge": {
                "enabled": true,
                "endpoint": "ws://remote-server:8080/api/ws/events"
            }
        });
        let diags = validate_config(&raw);
        assert!(diags.iter().any(|d| {
            d.level == DiagnosticLevel::Warn
                && d.path == "r8r_bridge.endpoint"
                && d.message.contains("cleartext")
        }));
    }

    #[test]
    fn test_validate_r8r_bridge_ws_localhost_no_warn() {
        let raw = json!({
            "r8r_bridge": {
                "enabled": true,
                "endpoint": "ws://localhost:8080/api/ws/events"
            }
        });
        let diags = validate_config(&raw);
        assert!(!diags.iter().any(|d| d.path == "r8r_bridge.endpoint"));
    }

    #[test]
    fn test_validate_r8r_bridge_wss_no_warn() {
        let raw = json!({
            "r8r_bridge": {
                "enabled": true,
                "endpoint": "wss://remote-server:8080/api/ws/events"
            }
        });
        let diags = validate_config(&raw);
        assert!(!diags.iter().any(|d| d.path == "r8r_bridge.endpoint"));
    }

    #[test]
    fn test_validate_r8r_bridge_disabled_no_warn() {
        let raw = json!({
            "r8r_bridge": {
                "enabled": false,
                "endpoint": "ws://remote-server:8080/api/ws/events"
            }
        });
        let diags = validate_config(&raw);
        assert!(!diags.iter().any(|d| d.path == "r8r_bridge.endpoint"));
    }
}
