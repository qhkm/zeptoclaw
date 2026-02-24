//! Structured audit logging for security-sensitive events.
//!
//! Emits structured `tracing` events with consistent field names so that
//! downstream log aggregators (Loki, Datadog, etc.) can filter on
//! `audit=true` and query by `category`, `event_type`, `severity`, etc.

use tracing::{error, info, warn};

/// Broad category of audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditCategory {
    /// Credential / secret leak detection.
    LeakDetection,
    /// Security policy violation.
    PolicyViolation,
    /// Prompt injection attempt.
    InjectionAttempt,
    /// Shell command blocked.
    ShellSecurity,
    /// Path traversal or symlink escape.
    PathSecurity,
    /// Mount validation failure.
    MountSecurity,
    /// Plugin integrity check failure.
    PluginIntegrity,
    /// Dangerous tool call sequence detected.
    ToolChainAlert,
}

impl std::fmt::Display for AuditCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LeakDetection => write!(f, "leak_detection"),
            Self::PolicyViolation => write!(f, "policy_violation"),
            Self::InjectionAttempt => write!(f, "injection_attempt"),
            Self::ShellSecurity => write!(f, "shell_security"),
            Self::PathSecurity => write!(f, "path_security"),
            Self::MountSecurity => write!(f, "mount_security"),
            Self::PluginIntegrity => write!(f, "plugin_integrity"),
            Self::ToolChainAlert => write!(f, "tool_chain_alert"),
        }
    }
}

/// Severity level for audit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSeverity {
    /// Informational — action was noted but not harmful.
    Info,
    /// Warning — action was sanitized or redacted.
    Warning,
    /// Critical — action was blocked entirely.
    Critical,
}

impl std::fmt::Display for AuditSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Warning => write!(f, "warning"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Emit a structured audit event via `tracing`.
///
/// All audit events carry `audit = true` so log pipelines can filter on them.
pub fn log_audit_event(
    category: AuditCategory,
    severity: AuditSeverity,
    event_type: &str,
    detail: &str,
    blocked: bool,
) {
    match severity {
        AuditSeverity::Info => {
            info!(
                audit = true,
                category = %category,
                severity = %severity,
                event_type = event_type,
                detail = detail,
                blocked = blocked,
                "audit event"
            );
        }
        AuditSeverity::Warning => {
            warn!(
                audit = true,
                category = %category,
                severity = %severity,
                event_type = event_type,
                detail = detail,
                blocked = blocked,
                "audit event"
            );
        }
        AuditSeverity::Critical => {
            error!(
                audit = true,
                category = %category,
                severity = %severity,
                event_type = event_type,
                detail = detail,
                blocked = blocked,
                "audit event"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_category_display() {
        assert_eq!(AuditCategory::LeakDetection.to_string(), "leak_detection");
        assert_eq!(
            AuditCategory::PolicyViolation.to_string(),
            "policy_violation"
        );
        assert_eq!(
            AuditCategory::InjectionAttempt.to_string(),
            "injection_attempt"
        );
        assert_eq!(AuditCategory::ShellSecurity.to_string(), "shell_security");
        assert_eq!(AuditCategory::PathSecurity.to_string(), "path_security");
        assert_eq!(AuditCategory::MountSecurity.to_string(), "mount_security");
        assert_eq!(
            AuditCategory::PluginIntegrity.to_string(),
            "plugin_integrity"
        );
        assert_eq!(
            AuditCategory::ToolChainAlert.to_string(),
            "tool_chain_alert"
        );
    }

    #[test]
    fn test_audit_severity_display() {
        assert_eq!(AuditSeverity::Info.to_string(), "info");
        assert_eq!(AuditSeverity::Warning.to_string(), "warning");
        assert_eq!(AuditSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn test_log_audit_event_info() {
        // Should not panic — emits a tracing event at info level.
        log_audit_event(
            AuditCategory::LeakDetection,
            AuditSeverity::Info,
            "secret_warn",
            "Potential secret detected",
            false,
        );
    }

    #[test]
    fn test_log_audit_event_warning() {
        log_audit_event(
            AuditCategory::InjectionAttempt,
            AuditSeverity::Warning,
            "injection_sanitized",
            "Prompt injection pattern removed",
            false,
        );
    }

    #[test]
    fn test_log_audit_event_critical() {
        log_audit_event(
            AuditCategory::PolicyViolation,
            AuditSeverity::Critical,
            "policy_block",
            "System file access blocked",
            true,
        );
    }

    #[test]
    fn test_audit_enums_debug_partial_eq() {
        // Verify Debug and PartialEq derives work.
        assert_eq!(AuditCategory::ShellSecurity, AuditCategory::ShellSecurity);
        assert_ne!(AuditCategory::ShellSecurity, AuditCategory::PathSecurity);
        assert_eq!(AuditSeverity::Critical, AuditSeverity::Critical);
        assert_ne!(AuditSeverity::Info, AuditSeverity::Warning);

        // Debug formatting.
        let dbg = format!("{:?}", AuditCategory::MountSecurity);
        assert!(dbg.contains("MountSecurity"));
        let dbg = format!("{:?}", AuditSeverity::Warning);
        assert!(dbg.contains("Warning"));
    }
}
