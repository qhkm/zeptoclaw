//! Multi-layered tool loop guard for repeated tool-call detection.
//!
//! Detects repeated tool-call patterns via five complementary strategies:
//!
//! 1. **Call-hash repetition**: SHA-256 hashes of `(tool_name, params)` with
//!    per-hash counters and graduated Warn -> Block -> CircuitBreak response.
//! 2. **Ping-pong detection**: Identifies period-2 (A-B-A-B) and period-3
//!    (A-B-C-A-B-C) oscillation patterns in the call sequence.
//! 3. **Outcome-aware blocking**: Hashes `(tool_name, params, result_prefix)`
//!    to block calls that repeatedly produce identical outcomes.
//! 4. **Poll relaxation**: Commands matching status/poll patterns get relaxed
//!    thresholds (configurable multiplier).
//! 5. **Backoff schedule**: Suggests increasing delays for repeated poll calls.

use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::config::LoopGuardConfig;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Action returned by the loop guard after checking a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopGuardAction {
    /// The call is allowed without restriction.
    Allow,
    /// The call is allowed but a warning is emitted.
    Warn {
        reason: String,
        suggested_delay_ms: Option<u64>,
    },
    /// The call should be blocked (but the session continues).
    Block { reason: String },
    /// The global circuit breaker has tripped; the session should stop.
    CircuitBreak { total_repetitions: u32 },
}

/// Aggregated statistics for observability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopGuardStats {
    pub total_checks: u64,
    pub warnings: u64,
    pub blocks: u64,
    pub circuit_breaks: u64,
    pub ping_pong_detections: u64,
    pub outcome_blocks: u64,
}

/// Signature of a single tool call for hashing.
#[derive(Debug, Clone, Copy)]
pub struct ToolCallSig<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

// ---------------------------------------------------------------------------
// LoopGuard
// ---------------------------------------------------------------------------

/// Multi-layered loop guard.
#[derive(Debug, Clone)]
pub struct LoopGuard {
    config: LoopGuardConfig,

    /// Per call-hash repetition counters (call hash -> count).
    call_counts: HashMap<String, u32>,

    /// Per outcome-hash repetition counters (outcome hash -> count).
    outcome_counts: HashMap<String, u32>,

    /// Ordered sequence of call hashes for ping-pong detection.
    call_sequence: Vec<String>,

    /// Ordered sequence of outcome hashes for window pruning.
    outcome_sequence: Vec<String>,

    /// Running total of all repetitions across all hashes (for global breaker).
    total_repetitions: u32,

    /// Stats.
    stats: LoopGuardStats,
}

/// Substrings that identify a poll/status command eligible for relaxed thresholds.
const POLL_PATTERNS: &[&str] = &[
    "status",
    "poll",
    "wait",
    "docker ps",
    "kubectl get",
    "git status",
];

impl LoopGuard {
    /// Create a new loop guard from configuration.
    pub fn new(config: LoopGuardConfig) -> Self {
        Self {
            config,
            call_counts: HashMap::new(),
            outcome_counts: HashMap::new(),
            call_sequence: Vec::new(),
            outcome_sequence: Vec::new(),
            total_repetitions: 0,
            stats: LoopGuardStats::default(),
        }
    }

    /// Check a batch of tool calls and return the appropriate action.
    ///
    /// This is the main entry point called from the agent loop after each LLM
    /// response that contains tool calls.
    pub fn check(&mut self, calls: &[ToolCallSig<'_>]) -> LoopGuardAction {
        if !self.config.enabled || calls.is_empty() {
            return LoopGuardAction::Allow;
        }

        self.stats.total_checks += 1;

        let call_hash = hash_call_batch(calls);
        let is_poll = is_poll_command(calls);

        // Record in sequence for ping-pong detection.
        self.call_sequence.push(call_hash.clone());

        // Prune call window if it exceeds the configured size.
        let window = self.config.window_size as usize;
        if window > 0 && self.call_sequence.len() > window {
            self.prune_call_window(window);
        }

        // Increment per-hash counter.
        let count = self.call_counts.entry(call_hash.clone()).or_insert(0);
        *count += 1;
        let count = *count;

        // Compute effective thresholds (poll commands get relaxed).
        let multiplier = if is_poll {
            self.config.poll_multiplier
        } else {
            1
        };
        let warn_at = self.config.warn_threshold * multiplier;
        let block_at = self.config.block_threshold * multiplier;

        // --- Ping-pong detection (checked before graduated response) ---
        if let Some(action) = self.check_ping_pong() {
            return action;
        }

        // --- Graduated response ---
        if count >= warn_at {
            self.total_repetitions += 1;

            // Global circuit breaker.
            if self.total_repetitions >= self.config.global_circuit_breaker {
                self.stats.circuit_breaks += 1;
                return LoopGuardAction::CircuitBreak {
                    total_repetitions: self.total_repetitions,
                };
            }

            if count >= block_at {
                self.stats.blocks += 1;
                return LoopGuardAction::Block {
                    reason: format!(
                        "tool call repeated {} times (threshold {})",
                        count, block_at
                    ),
                };
            }

            // Warn with optional backoff suggestion for polls.
            let suggested_delay_ms = if is_poll {
                Some(backoff_delay(count, warn_at))
            } else {
                None
            };

            self.stats.warnings += 1;
            return LoopGuardAction::Warn {
                reason: format!(
                    "tool call repeated {} times (warn threshold {})",
                    count, warn_at
                ),
                suggested_delay_ms,
            };
        }

        LoopGuardAction::Allow
    }

    /// Record the outcome of a tool call for outcome-aware blocking.
    ///
    /// Call this after the tool has executed with the first 1000 bytes of
    /// the result. Returns `Some(action)` if the outcome triggers a block.
    pub fn record_outcome(
        &mut self,
        name: &str,
        params: &str,
        result_prefix: &str,
    ) -> Option<LoopGuardAction> {
        if !self.config.enabled {
            return None;
        }

        let outcome_hash = hash_outcome(name, params, result_prefix);

        // Track outcome sequence for window pruning.
        self.outcome_sequence.push(outcome_hash.clone());
        let window = self.config.window_size as usize;
        if window > 0 && self.outcome_sequence.len() > window {
            self.prune_outcome_window(window);
        }

        let count = self.outcome_counts.entry(outcome_hash).or_insert(0);
        *count += 1;
        let count = *count;

        if count >= self.config.outcome_block_threshold {
            self.stats.outcome_blocks += 1;
            self.total_repetitions += 1;

            // Check global circuit breaker.
            if self.total_repetitions >= self.config.global_circuit_breaker {
                self.stats.circuit_breaks += 1;
                return Some(LoopGuardAction::CircuitBreak {
                    total_repetitions: self.total_repetitions,
                });
            }

            return Some(LoopGuardAction::Block {
                reason: format!(
                    "identical outcome repeated {} times (threshold {})",
                    count, self.config.outcome_block_threshold
                ),
            });
        }

        if count >= self.config.outcome_warn_threshold {
            self.stats.warnings += 1;
            return Some(LoopGuardAction::Warn {
                reason: format!(
                    "identical outcome repeated {} times (warn threshold {})",
                    count, self.config.outcome_warn_threshold
                ),
                suggested_delay_ms: None,
            });
        }

        None
    }

    /// Return a snapshot of the guard's statistics.
    pub fn stats(&self) -> &LoopGuardStats {
        &self.stats
    }

    // --- Private helpers ---

    /// Prune the call sequence to the most recent `keep` entries and rebuild
    /// `call_counts` from the remaining window.  Session-level metrics
    /// (`total_repetitions`, `stats`) are intentionally preserved.
    fn prune_call_window(&mut self, keep: usize) {
        let half = keep / 2;
        let drain_count = self.call_sequence.len() - half;
        self.call_sequence.drain(..drain_count);

        // Rebuild call_counts from the surviving window.
        self.call_counts.clear();
        for hash in &self.call_sequence {
            *self.call_counts.entry(hash.clone()).or_insert(0) += 1;
        }
    }

    /// Prune the outcome sequence to the most recent `keep/2` entries and
    /// rebuild `outcome_counts` from the remaining window.
    fn prune_outcome_window(&mut self, keep: usize) {
        let half = keep / 2;
        let drain_count = self.outcome_sequence.len() - half;
        self.outcome_sequence.drain(..drain_count);

        // Rebuild outcome_counts from the surviving window.
        self.outcome_counts.clear();
        for hash in &self.outcome_sequence {
            *self.outcome_counts.entry(hash.clone()).or_insert(0) += 1;
        }
    }

    /// Detect period-2 and period-3 oscillation patterns.
    fn check_ping_pong(&mut self) -> Option<LoopGuardAction> {
        // Treat ping_pong_min_repeats == 0 as "disabled".
        if self.config.ping_pong_min_repeats == 0 {
            return None;
        }

        let seq = &self.call_sequence;
        let min_repeats = self.config.ping_pong_min_repeats as usize;

        // Check period-2: need at least 2 * min_repeats entries.
        if seq.len() >= 2 * min_repeats && Self::has_periodic_pattern(seq, 2, min_repeats) {
            self.stats.ping_pong_detections += 1;
            self.stats.warnings += 1;
            self.total_repetitions += 1;

            if self.total_repetitions >= self.config.global_circuit_breaker {
                self.stats.circuit_breaks += 1;
                return Some(LoopGuardAction::CircuitBreak {
                    total_repetitions: self.total_repetitions,
                });
            }

            return Some(LoopGuardAction::Warn {
                reason: format!(
                    "ping-pong pattern (period 2) detected over {} cycles",
                    min_repeats
                ),
                suggested_delay_ms: None,
            });
        }

        // Check period-3: need at least 3 * min_repeats entries.
        if seq.len() >= 3 * min_repeats && Self::has_periodic_pattern(seq, 3, min_repeats) {
            self.stats.ping_pong_detections += 1;
            self.stats.warnings += 1;
            self.total_repetitions += 1;

            if self.total_repetitions >= self.config.global_circuit_breaker {
                self.stats.circuit_breaks += 1;
                return Some(LoopGuardAction::CircuitBreak {
                    total_repetitions: self.total_repetitions,
                });
            }

            return Some(LoopGuardAction::Warn {
                reason: format!(
                    "ping-pong pattern (period 3) detected over {} cycles",
                    min_repeats
                ),
                suggested_delay_ms: None,
            });
        }

        None
    }

    /// Check if the tail of `seq` repeats a pattern of length `period` at least
    /// `min_repeats` times.
    ///
    /// Requires the pattern to contain at least 2 distinct hashes — a uniform
    /// sequence (A-A-A-A) is not a ping-pong; it is already caught by the
    /// per-hash graduated response.
    fn has_periodic_pattern(seq: &[String], period: usize, min_repeats: usize) -> bool {
        // Defensive: zero period or min_repeats makes detection meaningless
        // and would cause a panic on the `tail[..period]` slice below.
        if period == 0 || min_repeats == 0 {
            return false;
        }

        let needed = period * min_repeats;
        if seq.len() < needed {
            return false;
        }

        let tail = &seq[seq.len() - needed..];
        let pattern = &tail[..period];

        // Require at least 2 distinct elements in the pattern.
        if pattern.iter().all(|h| h == &pattern[0]) {
            return false;
        }

        for cycle in 1..min_repeats {
            let offset = cycle * period;
            for i in 0..period {
                if tail[offset + i] != pattern[i] {
                    return false;
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// Hashing helpers
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max_bytes` bytes without splitting a
/// multi-byte UTF-8 character.  Always returns a valid `&str`.
pub(crate) fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Hash a batch of tool call signatures (name + normalized params).
fn hash_call_batch(batch: &[ToolCallSig<'_>]) -> String {
    let mut hasher = Sha256::new();
    for call in batch {
        hasher.update(call.name.as_bytes());
        hasher.update(b"\n");
        hasher.update(normalize_args(call.arguments).as_bytes());
        hasher.update(b"\n--\n");
    }
    hex::encode(hasher.finalize())
}

/// Hash a tool outcome: (name, params_truncated, result_prefix).
fn hash_outcome(name: &str, params: &str, result_prefix: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"\n");
    // Truncate params to 2KB for the hash to avoid massive allocations.
    let params_truncated = truncate_utf8(params, 2048);
    hasher.update(normalize_args(params_truncated).as_bytes());
    hasher.update(b"\n--\n");
    // Use first 1000 bytes of result.
    let prefix = truncate_utf8(result_prefix, 1000);
    hasher.update(prefix.as_bytes());
    hex::encode(hasher.finalize())
}

/// Normalize JSON arguments for stable hashing.
fn normalize_args(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => v.to_string(),
        Err(_) => raw.trim().to_string(),
    }
}

/// Check if any tool call in the batch matches a poll/status pattern.
fn is_poll_command(calls: &[ToolCallSig<'_>]) -> bool {
    for call in calls {
        let lower_name = call.name.to_lowercase();
        let lower_args = call.arguments.to_lowercase();
        for pattern in POLL_PATTERNS {
            if lower_name.contains(pattern) || lower_args.contains(pattern) {
                return true;
            }
        }
    }
    false
}

/// Compute an exponential backoff delay for repeated poll commands.
///
/// Uses `2^(count - warn_at)` seconds, capped at 30 seconds.
fn backoff_delay(count: u32, warn_at: u32) -> u64 {
    let exponent = count.saturating_sub(warn_at);
    let delay_secs = 1u64.wrapping_shl(exponent).min(30);
    delay_secs * 1000
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sig<'a>(name: &'a str, args: &'a str) -> ToolCallSig<'a> {
        ToolCallSig {
            name,
            arguments: args,
        }
    }

    fn default_config() -> LoopGuardConfig {
        LoopGuardConfig::default()
    }

    #[test]
    fn test_allow_first_call() {
        let mut guard = LoopGuard::new(default_config());
        let action = guard.check(&[sig("web_search", r#"{"q":"rust"}"#)]);
        assert_eq!(action, LoopGuardAction::Allow);
        assert_eq!(guard.stats().total_checks, 1);
    }

    #[test]
    fn test_warn_on_repeated_calls() {
        let mut guard = LoopGuard::new(default_config());
        let call = [sig("web_search", r#"{"q":"rust"}"#)];

        // First two calls are allowed.
        assert_eq!(guard.check(&call), LoopGuardAction::Allow);
        assert_eq!(guard.check(&call), LoopGuardAction::Allow);

        // Third call triggers warn (warn_threshold = 3).
        match guard.check(&call) {
            LoopGuardAction::Warn { reason, .. } => {
                assert!(reason.contains("repeated 3 times"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
        assert_eq!(guard.stats().warnings, 1);
    }

    #[test]
    fn test_block_after_threshold() {
        let mut guard = LoopGuard::new(default_config());
        let call = [sig("shell", r#"{"command":"ls"}"#)];

        // Calls 1-4: Allow (1-2), Warn (3-4).
        for _ in 0..4 {
            guard.check(&call);
        }

        // Call 5: block_threshold = 5.
        match guard.check(&call) {
            LoopGuardAction::Block { reason } => {
                assert!(reason.contains("repeated 5 times"));
            }
            other => panic!("expected Block, got {other:?}"),
        }
        assert_eq!(guard.stats().blocks, 1);
    }

    #[test]
    fn test_circuit_breaker() {
        let config = LoopGuardConfig {
            global_circuit_breaker: 3,
            warn_threshold: 2,
            block_threshold: 100, // high so we hit global breaker first
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        // Two different call hashes, each repeated twice -> 2 warns, total_reps = 2.
        for i in 0..2 {
            let name = format!("tool_{i}");
            let call = [sig(&name, r#"{}"#)];
            guard.check(&call); // count 1 -> Allow
            guard.check(&call); // count 2 -> Warn, total_repetitions += 1
        }
        assert_eq!(guard.stats().warnings, 2);
        assert_eq!(guard.stats().circuit_breaks, 0);

        // Third different hash repeated -> total_reps = 3 = global_circuit_breaker.
        let call = [sig("tool_2", r#"{}"#)];
        guard.check(&call); // count 1 -> Allow
        let action = guard.check(&call); // count 2 -> total_reps = 3 -> CircuitBreak
        assert!(
            matches!(action, LoopGuardAction::CircuitBreak { .. }),
            "expected CircuitBreak, got {action:?}"
        );
        assert_eq!(guard.stats().circuit_breaks, 1);
    }

    #[test]
    fn test_ping_pong_detection_period2() {
        let config = LoopGuardConfig {
            ping_pong_min_repeats: 2,
            warn_threshold: 100, // high so graduated response doesn't fire first
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        let call_a = [sig("read_file", r#"{"path":"a.txt"}"#)];
        let call_b = [sig("write_file", r#"{"path":"b.txt"}"#)];

        // A, B, A, B — period 2, 2 repeats.
        guard.check(&call_a);
        guard.check(&call_b);
        guard.check(&call_a);

        match guard.check(&call_b) {
            LoopGuardAction::Warn { reason, .. } => {
                assert!(reason.contains("ping-pong"));
                assert!(reason.contains("period 2"));
            }
            other => panic!("expected Warn with ping-pong, got {other:?}"),
        }
        assert_eq!(guard.stats().ping_pong_detections, 1);
    }

    #[test]
    fn test_ping_pong_detection_period3() {
        let config = LoopGuardConfig {
            ping_pong_min_repeats: 2,
            warn_threshold: 100,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        let call_a = [sig("tool_a", r#"{"x":1}"#)];
        let call_b = [sig("tool_b", r#"{"x":2}"#)];
        let call_c = [sig("tool_c", r#"{"x":3}"#)];

        // A, B, C, A, B, C — period 3, 2 repeats.
        guard.check(&call_a);
        guard.check(&call_b);
        guard.check(&call_c);
        guard.check(&call_a);
        guard.check(&call_b);

        match guard.check(&call_c) {
            LoopGuardAction::Warn { reason, .. } => {
                assert!(reason.contains("ping-pong"));
                assert!(reason.contains("period 3"));
            }
            other => panic!("expected Warn with ping-pong period 3, got {other:?}"),
        }
        assert_eq!(guard.stats().ping_pong_detections, 1);
    }

    #[test]
    fn test_outcome_aware_blocking() {
        let mut guard = LoopGuard::new(default_config());

        // outcome_warn_threshold = 2, outcome_block_threshold = 3
        let r = guard.record_outcome("shell", r#"{"cmd":"ls"}"#, "file1.txt\nfile2.txt");
        assert!(r.is_none());

        let r = guard.record_outcome("shell", r#"{"cmd":"ls"}"#, "file1.txt\nfile2.txt");
        match r {
            Some(LoopGuardAction::Warn { reason, .. }) => {
                assert!(reason.contains("identical outcome"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }

        let r = guard.record_outcome("shell", r#"{"cmd":"ls"}"#, "file1.txt\nfile2.txt");
        match r {
            Some(LoopGuardAction::Block { reason }) => {
                assert!(reason.contains("identical outcome"));
            }
            other => panic!("expected Block, got {other:?}"),
        }
        assert_eq!(guard.stats().outcome_blocks, 1);
    }

    #[test]
    fn test_poll_relaxation() {
        let config = LoopGuardConfig {
            warn_threshold: 2,
            block_threshold: 4,
            poll_multiplier: 3,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        // Poll command: effective warn = 2*3 = 6, block = 4*3 = 12.
        let call = [sig("shell", r#"{"command":"git status"}"#)];

        // Calls 1-5 should all be Allow for a poll.
        for _ in 0..5 {
            assert_eq!(guard.check(&call), LoopGuardAction::Allow);
        }

        // Call 6 should warn (effective warn_threshold = 6).
        match guard.check(&call) {
            LoopGuardAction::Warn { reason, .. } => {
                assert!(reason.contains("repeated 6 times"));
            }
            other => panic!("expected Warn at call 6, got {other:?}"),
        }
    }

    #[test]
    fn test_graduated_response() {
        let config = LoopGuardConfig {
            warn_threshold: 2,
            block_threshold: 4,
            global_circuit_breaker: 100,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);
        let call = [sig("tool", r#"{}"#)];

        assert_eq!(guard.check(&call), LoopGuardAction::Allow); // 1
        assert!(matches!(guard.check(&call), LoopGuardAction::Warn { .. })); // 2
        assert!(matches!(guard.check(&call), LoopGuardAction::Warn { .. })); // 3
        assert!(matches!(guard.check(&call), LoopGuardAction::Block { .. })); // 4
    }

    #[test]
    fn test_backoff_schedule() {
        let config = LoopGuardConfig {
            warn_threshold: 2,
            block_threshold: 100,
            poll_multiplier: 1, // no multiplier so poll thresholds = normal
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        let call = [sig("shell", r#"{"command":"docker ps"}"#)];

        guard.check(&call); // 1 -> Allow

        // 2 -> Warn with delay
        match guard.check(&call) {
            LoopGuardAction::Warn {
                suggested_delay_ms, ..
            } => {
                // 2^(2-2) * 1000 = 1000ms
                assert_eq!(suggested_delay_ms, Some(1000));
            }
            other => panic!("expected Warn, got {other:?}"),
        }

        // 3 -> Warn with bigger delay
        match guard.check(&call) {
            LoopGuardAction::Warn {
                suggested_delay_ms, ..
            } => {
                // 2^(3-2) * 1000 = 2000ms
                assert_eq!(suggested_delay_ms, Some(2000));
            }
            other => panic!("expected Warn, got {other:?}"),
        }

        // 4 -> Warn with even bigger delay
        match guard.check(&call) {
            LoopGuardAction::Warn {
                suggested_delay_ms, ..
            } => {
                // 2^(4-2) * 1000 = 4000ms
                assert_eq!(suggested_delay_ms, Some(4000));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn test_stats_tracking() {
        let config = LoopGuardConfig {
            warn_threshold: 1,
            block_threshold: 3,
            global_circuit_breaker: 100,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);
        let call = [sig("tool", r#"{}"#)];

        guard.check(&call); // warn (count 1 >= 1)
        guard.check(&call); // warn
        guard.check(&call); // block (count 3 >= 3)
        guard.check(&call); // block

        let s = guard.stats();
        assert_eq!(s.total_checks, 4);
        assert_eq!(s.warnings, 2);
        assert_eq!(s.blocks, 2);
        assert_eq!(s.circuit_breaks, 0);

        // Record some outcomes.
        guard.record_outcome("t", "{}", "same");
        guard.record_outcome("t", "{}", "same");
        guard.record_outcome("t", "{}", "same");

        let s = guard.stats();
        assert_eq!(s.outcome_blocks, 1);
        assert!(s.warnings >= 3); // 2 from calls + 1 from outcome warn
    }

    #[test]
    fn test_disabled_guard_allows_all() {
        let config = LoopGuardConfig {
            enabled: false,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);
        let call = [sig("tool", r#"{}"#)];

        for _ in 0..100 {
            assert_eq!(guard.check(&call), LoopGuardAction::Allow);
        }

        // Outcome recording also disabled.
        assert!(guard.record_outcome("t", "{}", "x").is_none());
        assert_eq!(guard.stats().total_checks, 0);
    }

    #[test]
    fn test_no_false_ping_pong_on_varied_calls() {
        let config = LoopGuardConfig {
            ping_pong_min_repeats: 2,
            warn_threshold: 100,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        // A, B, C, D — no repeating pattern.
        guard.check(&[sig("tool_a", r#"{"x":1}"#)]);
        guard.check(&[sig("tool_b", r#"{"x":2}"#)]);
        guard.check(&[sig("tool_c", r#"{"x":3}"#)]);

        let action = guard.check(&[sig("tool_d", r#"{"x":4}"#)]);
        assert_eq!(action, LoopGuardAction::Allow);
        assert_eq!(guard.stats().ping_pong_detections, 0);
    }

    #[test]
    fn test_backoff_delay_caps_at_30s() {
        // Verify backoff caps at 30 seconds (30_000ms).
        assert_eq!(backoff_delay(100, 2), 30_000);
        assert_eq!(backoff_delay(50, 2), 30_000);
        // Small exponents should not be capped.
        assert_eq!(backoff_delay(3, 2), 2_000); // 2^(3-2) * 1000 = 2000
        assert_eq!(backoff_delay(2, 2), 1_000); // 2^(2-2) * 1000 = 1000
    }

    #[test]
    fn test_config_defaults() {
        let config = LoopGuardConfig::default();
        assert!(config.enabled);
        assert_eq!(config.warn_threshold, 3);
        assert_eq!(config.block_threshold, 5);
        assert_eq!(config.global_circuit_breaker, 30);
        assert_eq!(config.ping_pong_min_repeats, 3);
        assert_eq!(config.poll_multiplier, 3);
        assert_eq!(config.outcome_warn_threshold, 2);
        assert_eq!(config.outcome_block_threshold, 3);
        assert_eq!(config.window_size, 200);
    }

    #[test]
    fn test_truncate_utf8_ascii() {
        assert_eq!(truncate_utf8("hello", 3), "hel");
        assert_eq!(truncate_utf8("hello", 10), "hello");
        assert_eq!(truncate_utf8("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_utf8_multibyte() {
        // Each CJK character is 3 bytes in UTF-8.
        let s = "\u{4f60}\u{597d}\u{4e16}\u{754c}"; // 12 bytes total
        assert_eq!(truncate_utf8(s, 6), "\u{4f60}\u{597d}"); // exactly 2 chars
                                                             // Cutting at byte 7 must back up to byte 6 (char boundary).
        assert_eq!(truncate_utf8(s, 7), "\u{4f60}\u{597d}");
        assert_eq!(truncate_utf8(s, 8), "\u{4f60}\u{597d}");
        assert_eq!(truncate_utf8(s, 9), "\u{4f60}\u{597d}\u{4e16}");
    }

    #[test]
    fn test_truncate_utf8_empty() {
        assert_eq!(truncate_utf8("", 0), "");
        assert_eq!(truncate_utf8("", 10), "");
    }

    #[test]
    fn test_truncate_utf8_emoji() {
        // Emoji like \u{1f600} is 4 bytes in UTF-8.
        let s = "\u{1f600}abc"; // 4 + 3 = 7 bytes
        assert_eq!(truncate_utf8(s, 3), ""); // 3 bytes into emoji -> backs up to 0
        assert_eq!(truncate_utf8(s, 4), "\u{1f600}");
        assert_eq!(truncate_utf8(s, 5), "\u{1f600}a");
    }

    #[test]
    fn test_ping_pong_min_repeats_zero_no_panic() {
        // When ping_pong_min_repeats is 0, ping-pong detection should be
        // disabled entirely -- no panics, no false detections.
        let config = LoopGuardConfig {
            ping_pong_min_repeats: 0,
            warn_threshold: 100,
            block_threshold: 200,
            global_circuit_breaker: 1000,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        let call_a = [sig("read_file", r#"{"path":"a.txt"}"#)];
        let call_b = [sig("write_file", r#"{"path":"b.txt"}"#)];

        // Feed an obvious A-B-A-B pattern that would normally trigger
        // ping-pong detection. With min_repeats == 0 it must be skipped.
        for _ in 0..10 {
            assert_eq!(guard.check(&call_a), LoopGuardAction::Allow);
            assert_eq!(guard.check(&call_b), LoopGuardAction::Allow);
        }

        assert_eq!(guard.stats().ping_pong_detections, 0);
    }

    #[test]
    fn test_has_periodic_pattern_zero_period() {
        // Directly verify has_periodic_pattern returns false for zero period.
        let seq: Vec<String> = vec!["a".into(), "b".into(), "a".into(), "b".into()];
        assert!(!LoopGuard::has_periodic_pattern(&seq, 0, 2));
    }

    #[test]
    fn test_has_periodic_pattern_zero_min_repeats() {
        // Directly verify has_periodic_pattern returns false for zero min_repeats.
        let seq: Vec<String> = vec!["a".into(), "b".into(), "a".into(), "b".into()];
        assert!(!LoopGuard::has_periodic_pattern(&seq, 2, 0));
    }

    #[test]
    fn test_hash_outcome_multibyte_no_panic() {
        // Ensure hash_outcome does not panic on multibyte strings exceeding limits.
        let long_params = "\u{4f60}\u{597d}".repeat(1500); // 9000 bytes > 2048
        let long_result = "\u{1f600}".repeat(500); // 2000 bytes > 1000
                                                   // Should not panic.
        let h = hash_outcome("tool", &long_params, &long_result);
        assert!(!h.is_empty());
    }

    #[test]
    fn test_window_pruning_prevents_unbounded_growth() {
        // After 300 distinct calls with window_size=100, internal sequences
        // must stay bounded.
        let config = LoopGuardConfig {
            window_size: 100,
            warn_threshold: 200, // high to avoid interference
            block_threshold: 300,
            global_circuit_breaker: 1000,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        for i in 0..300 {
            let name = format!("tool_{i}");
            guard.check(&[sig(&name, r#"{}"#)]);
            guard.record_outcome(&name, r#"{}"#, &format!("result_{i}"));
        }

        // call_sequence should have been pruned: at most window_size entries.
        assert!(
            guard.call_sequence.len() <= 100,
            "call_sequence len {} exceeds window_size 100",
            guard.call_sequence.len()
        );
        assert!(
            guard.outcome_sequence.len() <= 100,
            "outcome_sequence len {} exceeds window_size 100",
            guard.outcome_sequence.len()
        );
        // call_counts should only have entries for the surviving window.
        assert!(
            guard.call_counts.len() <= 100,
            "call_counts len {} exceeds window_size 100",
            guard.call_counts.len()
        );
        assert!(
            guard.outcome_counts.len() <= 100,
            "outcome_counts len {} exceeds window_size 100",
            guard.outcome_counts.len()
        );
    }

    #[test]
    fn test_window_pruning_resets_false_positives() {
        // A command repeated once every 50 calls across 500 total calls should
        // NOT trigger warnings after pruning. Without the window, the cumulative
        // count would exceed warn_threshold.
        let config = LoopGuardConfig {
            window_size: 100,
            warn_threshold: 3,
            block_threshold: 5,
            global_circuit_breaker: 1000,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        let target_call = [sig("target_tool", r#"{"q":"test"}"#)];

        for batch in 0..10 {
            // One target call per batch.
            let action = guard.check(&target_call);
            // After window pruning, count resets. The first few batches might
            // accumulate before the first prune, but once pruning kicks in
            // (after 100 calls), subsequent single occurrences per 50-call
            // gap should never hit warn_threshold (3).
            if batch >= 3 {
                // After enough pruning cycles, the target call count within
                // the window should be low (1 or 2 occurrences in 100 entries).
                assert!(
                    !matches!(action, LoopGuardAction::Block { .. }),
                    "target_tool should not be blocked at batch {batch}"
                );
            }

            // Interleave 49 distinct filler calls.
            for j in 0..49 {
                let filler = format!("filler_{}_{}", batch, j);
                guard.check(&[sig(&filler, r#"{}"#)]);
            }
        }
    }

    #[test]
    fn test_window_size_zero_disables_pruning() {
        // When window_size is 0, pruning should be disabled and all entries
        // kept (original behavior).
        let config = LoopGuardConfig {
            window_size: 0,
            warn_threshold: 200,
            block_threshold: 300,
            global_circuit_breaker: 1000,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        for i in 0..150 {
            let name = format!("tool_{i}");
            guard.check(&[sig(&name, r#"{}"#)]);
        }

        // No pruning: all 150 entries should remain.
        assert_eq!(guard.call_sequence.len(), 150);
    }

    #[test]
    fn test_outcome_window_pruning() {
        // Verify that outcome_counts are rebuilt correctly after pruning.
        let config = LoopGuardConfig {
            window_size: 20,
            outcome_warn_threshold: 5,
            outcome_block_threshold: 10,
            global_circuit_breaker: 1000,
            ..default_config()
        };
        let mut guard = LoopGuard::new(config);

        // Record 25 distinct outcomes to trigger pruning.
        for i in 0..25 {
            guard.record_outcome("tool", &format!(r#"{{"i":{i}}}"#), &format!("res_{i}"));
        }

        // After pruning, outcome_sequence should have ~10 entries (window/2).
        assert!(
            guard.outcome_sequence.len() <= 20,
            "outcome_sequence len {} exceeds window 20",
            guard.outcome_sequence.len()
        );
        // Each distinct outcome appears at most once, so no warn/block.
        assert_eq!(guard.stats().outcome_blocks, 0);
    }
}
