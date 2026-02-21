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
    "stripe",
    "custom_tools",
    "tool_profiles",
];

/// Known fields for each section. Nested as section.field.
const KNOWN_AGENTS_DEFAULTS: &[&str] = &[
    "workspace",
    "model",
    "max_tokens",
    "temperature",
    "max_tool_iterations",
    "agent_timeout_secs",
    "message_queue_mode",
    "streaming",
    "token_budget",
    "compact_tools",
    "tool_profile",
];

#[allow(dead_code)]
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
            }
        }
    }

    diagnostics
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
}
