//! Agent mode enforcement — controls what categories of tools the agent can use.
//!
//! Three modes are supported:
//! - **Observer**: Read-only. Can use filesystem read, network read, and memory tools.
//! - **Assistant**: Read/write. Can use most tools, but shell, hardware, and destructive
//!   operations require explicit approval.
//! - **Autonomous**: Full access. All tool categories are allowed without approval.
//!
//! The mode is checked in the agent loop **before** the approval gate.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::tools::ToolCategory;

/// Agent execution mode controlling tool permissions.
///
/// Determines which tool categories are allowed, require approval, or are
/// blocked during an agent session. The default mode is `Autonomous` for
/// backward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Read-only mode. Only FilesystemRead, NetworkRead, and Memory are allowed.
    Observer,
    /// Read/write mode. Most tools allowed; Shell, Hardware, Destructive need approval.
    Assistant,
    /// Full access. All tool categories are allowed without restriction.
    #[default]
    Autonomous,
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Observer => write!(f, "observer"),
            Self::Assistant => write!(f, "assistant"),
            Self::Autonomous => write!(f, "autonomous"),
        }
    }
}

impl std::str::FromStr for AgentMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "observer" => Ok(Self::Observer),
            "assistant" => Ok(Self::Assistant),
            "autonomous" => Ok(Self::Autonomous),
            _ => Err(format!(
                "unknown agent mode: '{}' (expected observer/assistant/autonomous)",
                s
            )),
        }
    }
}

/// The result of checking a tool category against the current agent mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryPermission {
    /// Tool is allowed to execute without additional checks.
    Allowed,
    /// Tool requires explicit user approval before execution.
    RequiresApproval,
    /// Tool is completely blocked and cannot be executed.
    Blocked,
}

/// Enforces agent mode restrictions on tool categories.
///
/// Created with a specific [`AgentMode`] and used to check whether a given
/// [`ToolCategory`] is allowed, requires approval, or is blocked.
pub struct ModePolicy {
    mode: AgentMode,
}

impl ModePolicy {
    /// Create a new mode policy for the given agent mode.
    pub fn new(mode: AgentMode) -> Self {
        Self { mode }
    }

    /// Get the agent mode for this policy.
    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    /// Check what permission a tool category has under the current mode.
    pub fn check(&self, category: ToolCategory) -> CategoryPermission {
        match self.mode {
            AgentMode::Autonomous => CategoryPermission::Allowed,
            AgentMode::Observer => match category {
                ToolCategory::FilesystemRead | ToolCategory::NetworkRead | ToolCategory::Memory => {
                    CategoryPermission::Allowed
                }
                _ => CategoryPermission::Blocked,
            },
            AgentMode::Assistant => match category {
                ToolCategory::FilesystemRead
                | ToolCategory::FilesystemWrite
                | ToolCategory::NetworkRead
                | ToolCategory::NetworkWrite
                | ToolCategory::Memory
                | ToolCategory::Messaging => CategoryPermission::Allowed,
                ToolCategory::Shell | ToolCategory::Hardware | ToolCategory::Destructive => {
                    CategoryPermission::RequiresApproval
                }
            },
        }
    }

    /// Get all blocked categories for this mode.
    pub fn blocked_categories(&self) -> HashSet<ToolCategory> {
        ToolCategory::all()
            .into_iter()
            .filter(|c| self.check(*c) == CategoryPermission::Blocked)
            .collect()
    }

    /// Get all categories that require approval for this mode.
    pub fn approval_categories(&self) -> HashSet<ToolCategory> {
        ToolCategory::all()
            .into_iter()
            .filter(|c| self.check(*c) == CategoryPermission::RequiresApproval)
            .collect()
    }
}

/// Configuration for agent mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentModeConfig {
    /// The agent mode: "observer", "assistant", or "autonomous".
    pub mode: String,
}

impl Default for AgentModeConfig {
    fn default() -> Self {
        Self {
            mode: "autonomous".into(),
        }
    }
}

impl AgentModeConfig {
    /// Parse the configured mode string into an `AgentMode`.
    ///
    /// Returns `Autonomous` if the string is invalid (with a tracing warning).
    pub fn resolve(&self) -> AgentMode {
        self.mode.parse::<AgentMode>().unwrap_or_else(|_| {
            tracing::warn!(
                mode = %self.mode,
                "Unknown agent mode '{}', falling back to Autonomous. \
                 Valid values: observer, assistant, autonomous.",
                self.mode
            );
            AgentMode::Autonomous
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observer_allows_read() {
        let p = ModePolicy::new(AgentMode::Observer);
        assert_eq!(
            p.check(ToolCategory::FilesystemRead),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::NetworkRead),
            CategoryPermission::Allowed
        );
        assert_eq!(p.check(ToolCategory::Memory), CategoryPermission::Allowed);
    }

    #[test]
    fn test_observer_blocks_write() {
        let p = ModePolicy::new(AgentMode::Observer);
        assert_eq!(
            p.check(ToolCategory::FilesystemWrite),
            CategoryPermission::Blocked
        );
        assert_eq!(p.check(ToolCategory::Shell), CategoryPermission::Blocked);
        assert_eq!(p.check(ToolCategory::Hardware), CategoryPermission::Blocked);
        assert_eq!(
            p.check(ToolCategory::Messaging),
            CategoryPermission::Blocked
        );
    }

    #[test]
    fn test_observer_blocks_network_write() {
        let p = ModePolicy::new(AgentMode::Observer);
        assert_eq!(
            p.check(ToolCategory::NetworkWrite),
            CategoryPermission::Blocked
        );
        assert_eq!(
            p.check(ToolCategory::Destructive),
            CategoryPermission::Blocked
        );
    }

    #[test]
    fn test_assistant_allows_readwrite() {
        let p = ModePolicy::new(AgentMode::Assistant);
        assert_eq!(
            p.check(ToolCategory::FilesystemRead),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::FilesystemWrite),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::NetworkWrite),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::NetworkRead),
            CategoryPermission::Allowed
        );
        assert_eq!(p.check(ToolCategory::Memory), CategoryPermission::Allowed);
        assert_eq!(
            p.check(ToolCategory::Messaging),
            CategoryPermission::Allowed
        );
    }

    #[test]
    fn test_assistant_requires_approval_for_dangerous() {
        let p = ModePolicy::new(AgentMode::Assistant);
        assert_eq!(
            p.check(ToolCategory::Shell),
            CategoryPermission::RequiresApproval
        );
        assert_eq!(
            p.check(ToolCategory::Hardware),
            CategoryPermission::RequiresApproval
        );
        assert_eq!(
            p.check(ToolCategory::Destructive),
            CategoryPermission::RequiresApproval
        );
    }

    #[test]
    fn test_autonomous_allows_all() {
        let p = ModePolicy::new(AgentMode::Autonomous);
        assert_eq!(p.check(ToolCategory::Shell), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::Hardware), CategoryPermission::Allowed);
        assert_eq!(
            p.check(ToolCategory::Destructive),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::FilesystemRead),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::FilesystemWrite),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::NetworkRead),
            CategoryPermission::Allowed
        );
        assert_eq!(
            p.check(ToolCategory::NetworkWrite),
            CategoryPermission::Allowed
        );
        assert_eq!(p.check(ToolCategory::Memory), CategoryPermission::Allowed);
        assert_eq!(
            p.check(ToolCategory::Messaging),
            CategoryPermission::Allowed
        );
    }

    #[test]
    fn test_parse_mode_from_string() {
        assert_eq!(
            "observer".parse::<AgentMode>().unwrap(),
            AgentMode::Observer
        );
        assert_eq!(
            "assistant".parse::<AgentMode>().unwrap(),
            AgentMode::Assistant
        );
        assert_eq!(
            "autonomous".parse::<AgentMode>().unwrap(),
            AgentMode::Autonomous
        );
        assert_eq!(
            "OBSERVER".parse::<AgentMode>().unwrap(),
            AgentMode::Observer
        );
        assert_eq!(
            "Assistant".parse::<AgentMode>().unwrap(),
            AgentMode::Assistant
        );
        assert!("invalid".parse::<AgentMode>().is_err());
        assert!("".parse::<AgentMode>().is_err());
    }

    #[test]
    fn test_observer_blocked_categories() {
        let p = ModePolicy::new(AgentMode::Observer);
        let blocked = p.blocked_categories();
        assert!(blocked.contains(&ToolCategory::Shell));
        assert!(blocked.contains(&ToolCategory::FilesystemWrite));
        assert!(blocked.contains(&ToolCategory::NetworkWrite));
        assert!(blocked.contains(&ToolCategory::Hardware));
        assert!(blocked.contains(&ToolCategory::Messaging));
        assert!(blocked.contains(&ToolCategory::Destructive));
        assert!(!blocked.contains(&ToolCategory::FilesystemRead));
        assert!(!blocked.contains(&ToolCategory::NetworkRead));
        assert!(!blocked.contains(&ToolCategory::Memory));
    }

    #[test]
    fn test_assistant_approval_categories() {
        let p = ModePolicy::new(AgentMode::Assistant);
        let approval = p.approval_categories();
        assert!(approval.contains(&ToolCategory::Shell));
        assert!(approval.contains(&ToolCategory::Hardware));
        assert!(approval.contains(&ToolCategory::Destructive));
        assert_eq!(approval.len(), 3);
    }

    #[test]
    fn test_autonomous_no_blocked() {
        let p = ModePolicy::new(AgentMode::Autonomous);
        assert!(p.blocked_categories().is_empty());
        assert!(p.approval_categories().is_empty());
    }

    #[test]
    fn test_default_mode_is_autonomous() {
        assert_eq!(AgentMode::default(), AgentMode::Autonomous);
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(AgentMode::Observer.to_string(), "observer");
        assert_eq!(AgentMode::Assistant.to_string(), "assistant");
        assert_eq!(AgentMode::Autonomous.to_string(), "autonomous");
    }

    #[test]
    fn test_mode_config_defaults() {
        let cfg = AgentModeConfig::default();
        assert_eq!(cfg.mode, "autonomous");
    }

    #[test]
    fn test_mode_config_resolve() {
        let mut cfg = AgentModeConfig::default();
        assert_eq!(cfg.resolve(), AgentMode::Autonomous);

        cfg.mode = "observer".to_string();
        assert_eq!(cfg.resolve(), AgentMode::Observer);

        cfg.mode = "assistant".to_string();
        assert_eq!(cfg.resolve(), AgentMode::Assistant);

        // Invalid falls back to Autonomous
        cfg.mode = "garbage".to_string();
        assert_eq!(cfg.resolve(), AgentMode::Autonomous);
    }

    #[test]
    fn test_mode_serde_roundtrip() {
        let mode = AgentMode::Observer;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"observer\"");
        let parsed: AgentMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
    }

    #[test]
    fn test_mode_policy_getter() {
        let p = ModePolicy::new(AgentMode::Assistant);
        assert_eq!(p.mode(), AgentMode::Assistant);
    }
}
