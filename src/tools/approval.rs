//! Tool approval system for ZeptoClaw
//!
//! This module provides a configurable approval gate that can require user
//! confirmation before certain tools are executed. When enabled, the approval
//! system checks each tool invocation against a policy to determine whether
//! the user must explicitly approve execution.
//!
//! # Policies
//!
//! - `AlwaysAllow` - All tools execute without approval (default)
//! - `AlwaysRequire` - Every tool invocation requires approval
//! - `RequireForTools` - Only named tools require approval
//! - `RequireForDangerous` - Tools tagged as "dangerous" require approval
//!
//! # Configuration
//!
//! The approval system is configured via `ApprovalConfig` in `config.json`:
//!
//! ```json
//! {
//!     "approval": {
//!         "enabled": true,
//!         "policy": "require_for_dangerous",
//!         "dangerous_tools": ["shell", "write_file", "edit_file"]
//!     }
//! }
//! ```
//!
//! # Example
//!
//! ```rust
//! use zeptoclaw::tools::approval::{ApprovalConfig, ApprovalGate};
//!
//! let config = ApprovalConfig::default();
//! let gate = ApprovalGate::new(config);
//!
//! // Default policy is AlwaysAllow with enabled=false
//! assert!(!gate.requires_approval("shell"));
//! ```

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Approval policy (runtime enum, not serialized directly)
// ---------------------------------------------------------------------------

/// Runtime approval policy that determines which tools require user approval.
///
/// This enum is constructed from `ApprovalConfig` and used by `ApprovalGate`
/// at runtime. For serialization, see `ApprovalPolicyConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// All tools execute without approval.
    AlwaysAllow,
    /// Every tool invocation requires approval.
    AlwaysRequire,
    /// Only the listed tools require approval.
    RequireForTools(Vec<String>),
    /// Tools tagged as "dangerous" require approval.
    RequireForDangerous,
}

// ---------------------------------------------------------------------------
// Serde-friendly policy config enum
// ---------------------------------------------------------------------------

/// Serializable approval policy selector for `config.json`.
///
/// Maps to the runtime `ApprovalPolicy` via `ApprovalGate::new()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicyConfig {
    /// All tools execute without approval.
    #[default]
    AlwaysAllow,
    /// Every tool invocation requires approval.
    AlwaysRequire,
    /// Only tools listed in `require_for` need approval.
    RequireForTools,
    /// Tools in the `dangerous_tools` list need approval.
    RequireForDangerous,
}

// ---------------------------------------------------------------------------
// Approval config (serde-deserializable)
// ---------------------------------------------------------------------------

/// Persistent approval configuration stored in `config.json`.
///
/// Controls whether tool approval is active and which policy governs
/// approval checks.
///
/// # Defaults
///
/// - `enabled`: `false`
/// - `policy`: `AlwaysAllow`
/// - `require_for`: empty
/// - `dangerous_tools`: `["shell", "write_file", "edit_file"]`
/// - `auto_approve_timeout_secs`: `0` (disabled)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    /// Master switch. When `false`, all tools execute without approval
    /// regardless of the policy setting.
    pub enabled: bool,

    /// Which approval policy to apply.
    pub policy: ApprovalPolicyConfig,

    /// Tool names that require approval when `policy` is `RequireForTools`.
    pub require_for: Vec<String>,

    /// Tool names considered dangerous. Used when `policy` is
    /// `RequireForDangerous`.
    pub dangerous_tools: Vec<String>,

    /// If greater than zero, auto-approve after this many seconds without
    /// a response. `0` means no auto-approve (wait indefinitely).
    pub auto_approve_timeout_secs: u64,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            policy: ApprovalPolicyConfig::AlwaysAllow,
            require_for: Vec::new(),
            dangerous_tools: ApprovalGate::default_dangerous_tools(),
            auto_approve_timeout_secs: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Approval request / response
// ---------------------------------------------------------------------------

/// A pending approval request describing a tool invocation that needs
/// user confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Name of the tool awaiting approval.
    pub tool_name: String,
    /// Arguments the tool would be called with.
    pub arguments: Value,
    /// When the request was created.
    pub timestamp: DateTime<Utc>,
    /// If auto-approve is enabled, the deadline after which the request
    /// is automatically approved. `None` means wait indefinitely.
    pub auto_approve_at: Option<DateTime<Utc>>,
}

impl ApprovalRequest {
    /// Create a new approval request.
    ///
    /// If `auto_approve_timeout_secs` is greater than zero, `auto_approve_at`
    /// is set to `timestamp + timeout`.
    pub fn new(tool_name: String, arguments: Value, auto_approve_timeout_secs: u64) -> Self {
        let timestamp = Utc::now();
        let auto_approve_at = if auto_approve_timeout_secs > 0 {
            Some(timestamp + Duration::seconds(auto_approve_timeout_secs as i64))
        } else {
            None
        };
        Self {
            tool_name,
            arguments,
            timestamp,
            auto_approve_at,
        }
    }

    /// Check whether this request has passed its auto-approve deadline.
    pub fn is_auto_approved(&self) -> bool {
        match self.auto_approve_at {
            Some(deadline) => Utc::now() >= deadline,
            None => false,
        }
    }
}

/// The outcome of an approval decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalResponse {
    /// The user approved the tool execution.
    Approved,
    /// The user denied the tool execution with an optional reason.
    Denied(String),
    /// The approval request timed out without a response and auto-approve
    /// was not configured.
    TimedOut,
}

// ---------------------------------------------------------------------------
// Approval gate (runtime checker)
// ---------------------------------------------------------------------------

/// Runtime approval checker that evaluates tool invocations against the
/// configured policy.
///
/// Constructed from `ApprovalConfig` and used by the agent loop to decide
/// whether to pause for user confirmation before executing a tool.
///
/// # Example
///
/// ```rust
/// use zeptoclaw::tools::approval::{ApprovalConfig, ApprovalPolicyConfig, ApprovalGate};
///
/// let config = ApprovalConfig {
///     enabled: true,
///     policy: ApprovalPolicyConfig::RequireForDangerous,
///     ..Default::default()
/// };
/// let gate = ApprovalGate::new(config);
///
/// assert!(gate.requires_approval("shell"));
/// assert!(!gate.requires_approval("echo"));
/// ```
pub struct ApprovalGate {
    /// Whether approval checking is enabled.
    enabled: bool,
    /// The resolved runtime policy.
    policy: ApprovalPolicy,
    /// Auto-approve timeout in seconds (0 = disabled).
    auto_approve_timeout_secs: u64,
}

impl ApprovalGate {
    /// Create a new `ApprovalGate` from the given configuration.
    ///
    /// The `ApprovalPolicyConfig` is resolved into a runtime
    /// `ApprovalPolicy`, incorporating the `require_for` and
    /// `dangerous_tools` lists as needed.
    pub fn new(config: ApprovalConfig) -> Self {
        let policy = match config.policy {
            ApprovalPolicyConfig::AlwaysAllow => ApprovalPolicy::AlwaysAllow,
            ApprovalPolicyConfig::AlwaysRequire => ApprovalPolicy::AlwaysRequire,
            ApprovalPolicyConfig::RequireForTools => {
                ApprovalPolicy::RequireForTools(config.require_for)
            }
            ApprovalPolicyConfig::RequireForDangerous => ApprovalPolicy::RequireForDangerous,
        };

        // When policy is RequireForDangerous we need the dangerous list at
        // check time, so we store it inside RequireForTools-like matching.
        // However, to keep the enum clean we handle it in `requires_approval`
        // by referencing `dangerous_tools` from config. We therefore store
        // the dangerous list separately.
        //
        // Actually, for simplicity we convert RequireForDangerous into
        // RequireForTools at construction time so the check is uniform.
        let policy = if policy == ApprovalPolicy::RequireForDangerous {
            ApprovalPolicy::RequireForTools(config.dangerous_tools)
        } else {
            policy
        };

        Self {
            enabled: config.enabled,
            policy,
            auto_approve_timeout_secs: config.auto_approve_timeout_secs,
        }
    }

    /// Check whether a tool with the given name requires user approval.
    ///
    /// Returns `false` if the approval system is disabled or if the policy
    /// does not require approval for this tool.
    pub fn requires_approval(&self, tool_name: &str) -> bool {
        if !self.enabled {
            return false;
        }

        match &self.policy {
            ApprovalPolicy::AlwaysAllow => false,
            ApprovalPolicy::AlwaysRequire => true,
            ApprovalPolicy::RequireForTools(tools) => tools
                .iter()
                .any(|pattern| matches_tool_pattern(pattern, tool_name)),
            // RequireForDangerous is converted to RequireForTools at
            // construction time, but we handle it here for completeness.
            ApprovalPolicy::RequireForDangerous => Self::default_dangerous_tools()
                .iter()
                .any(|pattern| matches_tool_pattern(pattern, tool_name)),
        }
    }

    /// Format a human-readable approval prompt for the given tool invocation.
    ///
    /// The output is intended for display in a CLI or chat message to ask
    /// the user whether to proceed.
    pub fn format_approval_request(&self, tool_name: &str, args: &Value) -> String {
        let args_display = match serde_json::to_string_pretty(args) {
            Ok(pretty) => pretty,
            Err(_) => args.to_string(),
        };
        format!(
            "[Approval Required]\n\
             Tool: {tool_name}\n\
             Arguments:\n{args_display}\n\n\
             Approve execution? (yes/no)"
        )
    }

    /// Create an `ApprovalRequest` for the given tool invocation.
    ///
    /// Uses the gate's configured auto-approve timeout.
    pub fn create_request(&self, tool_name: &str, args: &Value) -> ApprovalRequest {
        ApprovalRequest::new(
            tool_name.to_string(),
            args.clone(),
            self.auto_approve_timeout_secs,
        )
    }

    /// Return the default list of dangerous tool names.
    ///
    /// These are tools that perform potentially destructive actions and
    /// should require user approval when the `RequireForDangerous` policy
    /// is active.
    pub fn default_dangerous_tools() -> Vec<String> {
        vec![
            "shell".to_string(),
            "write_file".to_string(),
            "edit_file".to_string(),
            "google".to_string(),
        ]
    }

    /// Return a reference to the resolved runtime policy.
    pub fn policy(&self) -> &ApprovalPolicy {
        &self.policy
    }

    /// Return whether the approval gate is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

fn matches_tool_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return tool_name.ends_with(suffix);
    }
    pattern == tool_name
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Policy: AlwaysAllow -------------------------------------------

    #[test]
    fn test_always_allow_returns_false() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::AlwaysAllow,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        assert!(!gate.requires_approval("shell"));
        assert!(!gate.requires_approval("write_file"));
        assert!(!gate.requires_approval("echo"));
        assert!(!gate.requires_approval("anything"));
    }

    // ---- Policy: AlwaysRequire -----------------------------------------

    #[test]
    fn test_always_require_returns_true() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::AlwaysRequire,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        assert!(gate.requires_approval("shell"));
        assert!(gate.requires_approval("write_file"));
        assert!(gate.requires_approval("echo"));
        assert!(gate.requires_approval("web_search"));
    }

    // ---- Policy: RequireForTools ---------------------------------------

    #[test]
    fn test_require_for_tools_matches_listed_tools() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForTools,
            require_for: vec!["shell".to_string(), "write_file".to_string()],
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        assert!(gate.requires_approval("shell"));
        assert!(gate.requires_approval("write_file"));
        assert!(!gate.requires_approval("echo"));
        assert!(!gate.requires_approval("read_file"));
    }

    #[test]
    fn test_require_for_tools_empty_list() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForTools,
            require_for: vec![],
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        // Empty list means no tools require approval under this policy.
        assert!(!gate.requires_approval("shell"));
        assert!(!gate.requires_approval("anything"));
    }

    #[test]
    fn test_require_for_tools_multiple_tools() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForTools,
            require_for: vec![
                "shell".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "web_fetch".to_string(),
            ],
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        assert!(gate.requires_approval("shell"));
        assert!(gate.requires_approval("write_file"));
        assert!(gate.requires_approval("edit_file"));
        assert!(gate.requires_approval("web_fetch"));
        assert!(!gate.requires_approval("echo"));
        assert!(!gate.requires_approval("read_file"));
    }

    // ---- Policy: RequireForDangerous -----------------------------------

    #[test]
    fn test_require_for_dangerous_matches_default_dangerous_tools() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForDangerous,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        assert!(gate.requires_approval("shell"));
        assert!(gate.requires_approval("write_file"));
        assert!(gate.requires_approval("edit_file"));
        assert!(!gate.requires_approval("echo"));
        assert!(!gate.requires_approval("read_file"));
        assert!(!gate.requires_approval("web_search"));
    }

    #[test]
    fn test_require_for_dangerous_custom_list() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForDangerous,
            dangerous_tools: vec!["web_fetch".to_string(), "message".to_string()],
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        // Custom dangerous list overrides the defaults.
        assert!(gate.requires_approval("web_fetch"));
        assert!(gate.requires_approval("message"));
        assert!(!gate.requires_approval("shell"));
        assert!(!gate.requires_approval("write_file"));
    }

    // ---- Disabled config -----------------------------------------------

    #[test]
    fn test_disabled_config_bypasses_all_checks() {
        let config = ApprovalConfig {
            enabled: false,
            policy: ApprovalPolicyConfig::AlwaysRequire,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        // Even with AlwaysRequire policy, disabled gate returns false.
        assert!(!gate.requires_approval("shell"));
        assert!(!gate.requires_approval("write_file"));
        assert!(!gate.requires_approval("echo"));
    }

    // ---- Default config ------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = ApprovalConfig::default();

        assert!(!config.enabled);
        assert_eq!(config.policy, ApprovalPolicyConfig::AlwaysAllow);
        assert!(config.require_for.is_empty());
        assert_eq!(
            config.dangerous_tools,
            vec!["shell", "write_file", "edit_file", "google"]
        );
        assert_eq!(config.auto_approve_timeout_secs, 0);
    }

    // ---- Tool name case sensitivity ------------------------------------

    #[test]
    fn test_tool_name_case_sensitivity() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForTools,
            require_for: vec!["shell".to_string()],
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        // Tool names are case-sensitive.
        assert!(gate.requires_approval("shell"));
        assert!(!gate.requires_approval("Shell"));
        assert!(!gate.requires_approval("SHELL"));
    }

    #[test]
    fn test_wildcard_patterns() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForTools,
            require_for: vec!["shell*".to_string(), "*_file".to_string()],
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);
        assert!(gate.requires_approval("shell"));
        assert!(gate.requires_approval("shell_exec"));
        assert!(gate.requires_approval("write_file"));
        assert!(!gate.requires_approval("web_search"));
    }

    // ---- format_approval_request ---------------------------------------

    #[test]
    fn test_format_approval_request_output() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config);

        let args = json!({"command": "rm -rf /tmp/test"});
        let output = gate.format_approval_request("shell", &args);

        assert!(output.contains("[Approval Required]"));
        assert!(output.contains("Tool: shell"));
        assert!(output.contains("rm -rf /tmp/test"));
        assert!(output.contains("Approve execution? (yes/no)"));
    }

    #[test]
    fn test_format_approval_request_complex_args() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config);

        let args = json!({
            "path": "/home/user/file.txt",
            "content": "Hello, world!",
            "overwrite": true
        });
        let output = gate.format_approval_request("write_file", &args);

        assert!(output.contains("Tool: write_file"));
        assert!(output.contains("/home/user/file.txt"));
        assert!(output.contains("Hello, world!"));
    }

    // ---- ApprovalRequest -----------------------------------------------

    #[test]
    fn test_approval_request_construction() {
        let args = json!({"command": "ls -la"});
        let request = ApprovalRequest::new("shell".to_string(), args.clone(), 0);

        assert_eq!(request.tool_name, "shell");
        assert_eq!(request.arguments, args);
        assert!(request.auto_approve_at.is_none());
    }

    #[test]
    fn test_approval_request_with_auto_approve_timeout() {
        let args = json!({"command": "ls"});
        let before = Utc::now();
        let request = ApprovalRequest::new("shell".to_string(), args, 30);
        let after = Utc::now();

        assert!(request.auto_approve_at.is_some());
        let deadline = request.auto_approve_at.unwrap();

        // The deadline should be roughly 30 seconds after creation.
        let earliest = before + Duration::seconds(30);
        let latest = after + Duration::seconds(30);
        assert!(deadline >= earliest);
        assert!(deadline <= latest);
    }

    #[test]
    fn test_approval_request_not_auto_approved_immediately() {
        let args = json!({"command": "ls"});
        let request = ApprovalRequest::new("shell".to_string(), args, 60);

        // With a 60-second timeout, the request should not be auto-approved
        // immediately.
        assert!(!request.is_auto_approved());
    }

    #[test]
    fn test_approval_request_no_auto_approve_when_disabled() {
        let args = json!({"command": "ls"});
        let request = ApprovalRequest::new("shell".to_string(), args, 0);

        // With timeout=0, auto-approve is disabled.
        assert!(!request.is_auto_approved());
    }

    // ---- ApprovalResponse ----------------------------------------------

    #[test]
    fn test_approval_response_variants() {
        let approved = ApprovalResponse::Approved;
        let denied = ApprovalResponse::Denied("too dangerous".to_string());
        let timed_out = ApprovalResponse::TimedOut;

        assert_eq!(approved, ApprovalResponse::Approved);
        assert_eq!(
            denied,
            ApprovalResponse::Denied("too dangerous".to_string())
        );
        assert_eq!(timed_out, ApprovalResponse::TimedOut);
    }

    // ---- Serialization roundtrip ---------------------------------------

    #[test]
    fn test_approval_config_serialization_roundtrip() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::RequireForTools,
            require_for: vec!["shell".to_string(), "write_file".to_string()],
            dangerous_tools: vec![
                "shell".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
            ],
            auto_approve_timeout_secs: 30,
        };

        let json_str = serde_json::to_string(&config).expect("serialize");
        let deserialized: ApprovalConfig = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(deserialized.enabled, config.enabled);
        assert_eq!(deserialized.policy, config.policy);
        assert_eq!(deserialized.require_for, config.require_for);
        assert_eq!(deserialized.dangerous_tools, config.dangerous_tools);
        assert_eq!(
            deserialized.auto_approve_timeout_secs,
            config.auto_approve_timeout_secs
        );
    }

    #[test]
    fn test_approval_config_deserialize_from_json() {
        let json_str = r#"{
            "enabled": true,
            "policy": "require_for_dangerous",
            "dangerous_tools": ["shell", "write_file"]
        }"#;

        let config: ApprovalConfig = serde_json::from_str(json_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.policy, ApprovalPolicyConfig::RequireForDangerous);
        assert_eq!(
            config.dangerous_tools,
            vec!["shell".to_string(), "write_file".to_string()]
        );
        // require_for should default to empty.
        assert!(config.require_for.is_empty());
        // auto_approve_timeout_secs should default to 0.
        assert_eq!(config.auto_approve_timeout_secs, 0);
    }

    // ---- Gate helper methods -------------------------------------------

    #[test]
    fn test_default_dangerous_tools_list() {
        let defaults = ApprovalGate::default_dangerous_tools();
        assert_eq!(defaults.len(), 4);
        assert!(defaults.contains(&"shell".to_string()));
        assert!(defaults.contains(&"write_file".to_string()));
        assert!(defaults.contains(&"edit_file".to_string()));
        assert!(defaults.contains(&"google".to_string()));
    }

    #[test]
    fn test_gate_create_request() {
        let config = ApprovalConfig {
            enabled: true,
            auto_approve_timeout_secs: 45,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);

        let args = json!({"command": "echo test"});
        let request = gate.create_request("shell", &args);

        assert_eq!(request.tool_name, "shell");
        assert_eq!(request.arguments, args);
        assert!(request.auto_approve_at.is_some());
    }

    #[test]
    fn test_gate_is_enabled() {
        let enabled_gate = ApprovalGate::new(ApprovalConfig {
            enabled: true,
            ..Default::default()
        });
        assert!(enabled_gate.is_enabled());

        let disabled_gate = ApprovalGate::new(ApprovalConfig::default());
        assert!(!disabled_gate.is_enabled());
    }

    #[test]
    fn test_gate_policy_accessor() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::AlwaysRequire,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);
        assert_eq!(*gate.policy(), ApprovalPolicy::AlwaysRequire);
    }

    #[test]
    fn test_gate_policy_always_allow_accessor() {
        let config = ApprovalConfig {
            enabled: true,
            policy: ApprovalPolicyConfig::AlwaysAllow,
            ..Default::default()
        };
        let gate = ApprovalGate::new(config);
        assert_eq!(*gate.policy(), ApprovalPolicy::AlwaysAllow);
    }
}
