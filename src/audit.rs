//! Structured audit logging for security-sensitive events.
//!
//! Emits structured `tracing` events with consistent field names so that
//! downstream log aggregators (Loki, Datadog, etc.) can filter on
//! `audit=true` and query by `category`, `event_type`, `severity`, etc.

use chrono::Utc;
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};
use std::sync::Mutex;
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
    /// Taint tracking: data-flow policy violation.
    TaintViolation,
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
            Self::TaintViolation => write!(f, "taint_violation"),
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

/// Action type in the execution hash chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    ToolInvoke,
    ShellExec,
    NetworkAccess,
    AgentSpawn,
    AgentMessage,
    MemoryAccess,
    FileAccess,
    AuthAttempt,
    ConfigChange,
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolInvoke => write!(f, "tool_invoke"),
            Self::ShellExec => write!(f, "shell_exec"),
            Self::NetworkAccess => write!(f, "network_access"),
            Self::AgentSpawn => write!(f, "agent_spawn"),
            Self::AgentMessage => write!(f, "agent_message"),
            Self::MemoryAccess => write!(f, "memory_access"),
            Self::FileAccess => write!(f, "file_access"),
            Self::AuthAttempt => write!(f, "auth_attempt"),
            Self::ConfigChange => write!(f, "config_change"),
        }
    }
}

/// One immutable chain entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub seq: u64,
    pub timestamp: String,
    pub agent_id: String,
    pub action: AuditAction,
    pub detail: String,
    pub outcome: String,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug)]
struct AuditHashChain {
    state: Mutex<AuditChainState>,
}

#[derive(Debug, Default)]
struct AuditChainState {
    entries: Vec<AuditEntry>,
    tip: String,
}

impl Default for AuditHashChain {
    fn default() -> Self {
        Self {
            state: Mutex::new(AuditChainState {
                entries: Vec::new(),
                tip: genesis_hash(),
            }),
        }
    }
}

impl AuditHashChain {
    fn record(
        &self,
        agent_id: &str,
        action: AuditAction,
        detail: &str,
        outcome: &str,
    ) -> AuditEntry {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let seq = state.entries.len() as u64 + 1;
        let timestamp = Utc::now().to_rfc3339();
        let agent_id = sanitize_text(agent_id, 120);
        let detail = sanitize_text(detail, 512);
        let outcome = sanitize_text(outcome, 120);
        let prev_hash = state.tip.clone();
        let hash = compute_entry_hash(
            seq, &timestamp, &agent_id, action, &detail, &outcome, &prev_hash,
        );

        let entry = AuditEntry {
            seq,
            timestamp,
            agent_id,
            action,
            detail,
            outcome,
            prev_hash,
            hash: hash.clone(),
        };

        state.tip = hash;
        state.entries.push(entry.clone());
        entry
    }

    fn verify_integrity(&self) -> std::result::Result<(), String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut expected_prev = genesis_hash();

        for entry in &state.entries {
            if entry.prev_hash != expected_prev {
                return Err(format!(
                    "chain broken at seq {}: prev hash mismatch",
                    entry.seq
                ));
            }

            let expected_hash = compute_entry_hash(
                entry.seq,
                &entry.timestamp,
                &entry.agent_id,
                entry.action,
                &entry.detail,
                &entry.outcome,
                &expected_prev,
            );

            if entry.hash != expected_hash {
                return Err(format!("chain broken at seq {}: hash mismatch", entry.seq));
            }

            expected_prev = entry.hash.clone();
        }

        if state.tip != expected_prev {
            return Err("chain broken: tip does not match final entry hash".to_string());
        }

        Ok(())
    }

    fn recent(&self, n: usize) -> Vec<AuditEntry> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if n == 0 || state.entries.is_empty() {
            return Vec::new();
        }
        let start = state.entries.len().saturating_sub(n);
        state.entries[start..].to_vec()
    }

    fn tip_hash(&self) -> String {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.tip.clone()
    }
}

fn sanitize_text(value: &str, max_len: usize) -> String {
    let normalized = value.trim().replace(['\n', '\r'], " ");
    normalized.chars().take(max_len).collect()
}

fn compute_entry_hash(
    seq: u64,
    timestamp: &str,
    agent_id: &str,
    action: AuditAction,
    detail: &str,
    outcome: &str,
    prev_hash: &str,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        seq, timestamp, agent_id, action, detail, outcome, prev_hash
    );
    let digest = Sha256::digest(payload.as_bytes());
    hex::encode(digest)
}

fn genesis_hash() -> String {
    "0".repeat(64)
}

static AUDIT_CHAIN: Lazy<AuditHashChain> = Lazy::new(AuditHashChain::default);

/// Append a new event to the in-memory audit hash chain.
pub fn record_audit_chain_event(
    agent_id: &str,
    action: AuditAction,
    detail: &str,
    outcome: &str,
) -> AuditEntry {
    AUDIT_CHAIN.record(agent_id, action, detail, outcome)
}

/// Verify integrity of all in-memory audit chain entries.
pub fn verify_audit_chain_integrity() -> std::result::Result<(), String> {
    AUDIT_CHAIN.verify_integrity()
}

/// Return the `n` most recent chain entries.
pub fn recent_audit_entries(n: usize) -> Vec<AuditEntry> {
    AUDIT_CHAIN.recent(n)
}

/// Return current chain tip hash.
pub fn audit_tip_hash() -> String {
    AUDIT_CHAIN.tip_hash()
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
        assert_eq!(AuditCategory::TaintViolation.to_string(), "taint_violation");
    }

    #[test]
    fn test_audit_severity_display() {
        assert_eq!(AuditSeverity::Info.to_string(), "info");
        assert_eq!(AuditSeverity::Warning.to_string(), "warning");
        assert_eq!(AuditSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn test_audit_action_display() {
        assert_eq!(AuditAction::ToolInvoke.to_string(), "tool_invoke");
        assert_eq!(AuditAction::ShellExec.to_string(), "shell_exec");
        assert_eq!(AuditAction::NetworkAccess.to_string(), "network_access");
        assert_eq!(AuditAction::AgentSpawn.to_string(), "agent_spawn");
        assert_eq!(AuditAction::AgentMessage.to_string(), "agent_message");
        assert_eq!(AuditAction::MemoryAccess.to_string(), "memory_access");
        assert_eq!(AuditAction::FileAccess.to_string(), "file_access");
        assert_eq!(AuditAction::AuthAttempt.to_string(), "auth_attempt");
        assert_eq!(AuditAction::ConfigChange.to_string(), "config_change");
    }

    #[test]
    fn test_hash_chain_record_and_verify() {
        let chain = AuditHashChain::default();

        let first = chain.record("agent-a", AuditAction::ToolInvoke, "tool=echo", "success");
        let second = chain.record("agent-a", AuditAction::ShellExec, "tool=shell", "success");

        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev_hash, first.hash);
        assert_eq!(chain.verify_integrity(), Ok(()));
    }

    #[test]
    fn test_hash_chain_recent_and_tip() {
        let chain = AuditHashChain::default();
        assert_eq!(chain.tip_hash(), genesis_hash());

        let first = chain.record("agent-x", AuditAction::ToolInvoke, "tool=echo", "ok");
        let second = chain.record(
            "agent-x",
            AuditAction::NetworkAccess,
            "tool=web_fetch",
            "ok",
        );
        assert_ne!(first.hash, chain.tip_hash());
        assert_eq!(second.hash, chain.tip_hash());

        let recent = chain.recent(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].seq, 2);
    }

    #[test]
    fn test_hash_chain_detects_tampering() {
        let chain = AuditHashChain::default();
        chain.record("agent-t", AuditAction::ToolInvoke, "tool=echo", "success");
        chain.record("agent-t", AuditAction::ToolInvoke, "tool=shell", "success");

        {
            let mut state = chain.state.lock().unwrap_or_else(|e| e.into_inner());
            state.entries[1].detail = "tampered".to_string();
        }

        let err = chain.verify_integrity().unwrap_err();
        assert!(err.contains("hash mismatch"));
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
        assert_eq!(AuditAction::AuthAttempt, AuditAction::AuthAttempt);
        assert_ne!(AuditAction::AuthAttempt, AuditAction::ConfigChange);

        // Debug formatting.
        let dbg = format!("{:?}", AuditCategory::MountSecurity);
        assert!(dbg.contains("MountSecurity"));
        let dbg = format!("{:?}", AuditSeverity::Warning);
        assert!(dbg.contains("Warning"));
        let dbg = format!("{:?}", AuditAction::ShellExec);
        assert!(dbg.contains("ShellExec"));
    }
}
