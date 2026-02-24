//! Tool chain alerting — detects suspicious tool call sequences.
//!
//! Tracks tool names across a session's tool loop and logs warnings
//! when known-dangerous patterns are observed. Pure observability —
//! never blocks tool execution.

use tracing::warn;

/// Known dangerous tool call patterns (ordered subsequences).
/// Each pattern is a sequence of tool names that, when observed
/// in order within a session, trigger a warning.
const DANGEROUS_CHAINS: &[(&[&str], &str)] = &[
    (
        &["filesystem_write", "shell_execute"],
        "write-then-execute: possible code injection",
    ),
    (
        &["shell_execute", "web_fetch"],
        "execute-then-fetch: possible data exfiltration",
    ),
    (
        &["longterm_memory", "shell_execute"],
        "memory-then-execute: possible memory poisoning exploitation",
    ),
];

/// Tracks tool calls within a single agent message processing cycle
/// and checks for dangerous sequential patterns.
#[derive(Debug)]
pub struct ChainTracker {
    /// Ordered list of tool names called in this session.
    history: Vec<String>,
    /// Patterns already alerted (by index into `DANGEROUS_CHAINS`) to avoid spam.
    alerted: Vec<bool>,
}

impl Default for ChainTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ChainTracker {
    /// Create a new tracker.
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            alerted: vec![false; DANGEROUS_CHAINS.len()],
        }
    }

    /// Record one or more tool calls from a single iteration (they may execute
    /// in parallel). After recording, checks all patterns and logs warnings for
    /// any new matches.
    pub fn record(&mut self, tool_names: &[String]) {
        self.history.extend(tool_names.iter().cloned());
        self.check_patterns();
    }

    /// Check if the history contains any dangerous subsequence.
    fn check_patterns(&mut self) {
        for (idx, (pattern, description)) in DANGEROUS_CHAINS.iter().enumerate() {
            if self.alerted[idx] {
                continue;
            }
            if Self::contains_subsequence(&self.history, pattern) {
                self.alerted[idx] = true;
                warn!(
                    pattern = ?pattern,
                    tools = ?self.history,
                    "Tool chain alert: {}",
                    description,
                );
                crate::audit::log_audit_event(
                    crate::audit::AuditCategory::ToolChainAlert,
                    crate::audit::AuditSeverity::Warning,
                    "tool_chain_alert",
                    &format!("{description}: {:?}", self.history),
                    false,
                );
            }
        }
    }

    /// Check if `haystack` contains `needle` as an ordered subsequence
    /// (not necessarily contiguous).
    fn contains_subsequence(haystack: &[String], needle: &[&str]) -> bool {
        let mut needle_idx = 0;
        for item in haystack {
            if needle_idx < needle.len() && item == needle[needle_idx] {
                needle_idx += 1;
            }
        }
        needle_idx == needle.len()
    }

    /// Returns the current tool history (for testing).
    #[cfg(test)]
    pub fn history(&self) -> &[String] {
        &self.history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_alert_on_clean_sequence() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["web_fetch".into(), "filesystem_read".into()]);
        tracker.record(&["filesystem_read".into()]);
        // No dangerous patterns — alerted flags all false.
        assert!(!tracker.alerted.iter().any(|&a| a));
        assert_eq!(tracker.history().len(), 3);
    }

    #[test]
    fn test_detects_write_then_execute() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["filesystem_write".into()]);
        tracker.record(&["shell_execute".into()]);
        assert!(tracker.alerted[0]);
        // Other patterns not triggered.
        assert!(!tracker.alerted[1]);
        assert!(!tracker.alerted[2]);
    }

    #[test]
    fn test_detects_execute_then_fetch() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["shell_execute".into()]);
        tracker.record(&["web_fetch".into()]);
        assert!(tracker.alerted[1]);
    }

    #[test]
    fn test_detects_memory_then_execute() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["longterm_memory".into()]);
        tracker.record(&["filesystem_read".into()]); // interleaved, harmless
        tracker.record(&["shell_execute".into()]);
        assert!(tracker.alerted[2]);
    }

    #[test]
    fn test_no_false_positive_on_reverse_order() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["shell_execute".into()]);
        tracker.record(&["filesystem_write".into()]);
        // Reversed order should NOT trigger write-then-execute.
        assert!(!tracker.alerted[0]);
    }

    #[test]
    fn test_subsequence_detection_non_contiguous() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["filesystem_write".into()]);
        tracker.record(&["web_fetch".into()]); // not part of pattern
        tracker.record(&["shell_execute".into()]);
        assert!(tracker.alerted[0]); // write ... execute still detected
    }

    #[test]
    fn test_no_duplicate_alerts() {
        let mut tracker = ChainTracker::new();
        tracker.record(&["filesystem_write".into()]);
        tracker.record(&["shell_execute".into()]);
        assert!(tracker.alerted[0]);
        // Record more of the same pattern — alerted flag already true, no double-warn.
        tracker.record(&["filesystem_write".into()]);
        tracker.record(&["shell_execute".into()]);
        assert!(tracker.alerted[0]);
    }

    #[test]
    fn test_contains_subsequence_basic() {
        assert!(ChainTracker::contains_subsequence(
            &["a".into(), "b".into(), "c".into()],
            &["a", "c"],
        ));
        assert!(!ChainTracker::contains_subsequence(
            &["a".into(), "b".into(), "c".into()],
            &["c", "a"],
        ));
    }

    #[test]
    fn test_empty_history_no_match() {
        assert!(!ChainTracker::contains_subsequence(&[], &["a", "b"]));
    }

    #[test]
    fn test_empty_pattern_always_matches() {
        assert!(ChainTracker::contains_subsequence(&["a".into()], &[]));
    }

    #[test]
    fn test_default_trait() {
        let tracker = ChainTracker::default();
        assert!(tracker.history.is_empty());
        assert_eq!(tracker.alerted.len(), DANGEROUS_CHAINS.len());
    }

    #[test]
    fn test_parallel_tool_batch_recording() {
        // Simulate a single LLM turn requesting both write and execute in parallel.
        let mut tracker = ChainTracker::new();
        tracker.record(&["filesystem_write".into(), "shell_execute".into()]);
        assert!(tracker.alerted[0]);
    }

    #[test]
    fn test_multiple_patterns_detected() {
        let mut tracker = ChainTracker::new();
        // write → execute triggers pattern 0
        tracker.record(&["filesystem_write".into()]);
        tracker.record(&["shell_execute".into()]);
        assert!(tracker.alerted[0]);
        // execute → fetch triggers pattern 1
        tracker.record(&["web_fetch".into()]);
        assert!(tracker.alerted[1]);
    }
}
