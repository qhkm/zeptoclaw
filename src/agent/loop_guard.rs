//! Tool loop guard for repeated tool-call sequence detection.
//!
//! Detects repeated tool-call patterns by hashing normalized tool call batches
//! and scanning a sliding window for repeated hashes.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopGuardDecision {
    Continue,
    Warn {
        hash: String,
        occurrences: usize,
        loops_detected: usize,
    },
    Break {
        hash: String,
        occurrences: usize,
        loops_detected: usize,
    },
}

#[derive(Debug, Clone)]
pub struct LoopGuard {
    window: usize,
    repetition_threshold: usize,
    max_loops_detected: usize,
    recent_hashes: VecDeque<String>,
    loops_detected: usize,
}

impl LoopGuard {
    pub fn new(window: usize, repetition_threshold: usize, max_loops_detected: usize) -> Self {
        Self {
            window: window.max(1),
            repetition_threshold: repetition_threshold.max(2),
            max_loops_detected: max_loops_detected.max(1),
            recent_hashes: VecDeque::new(),
            loops_detected: 0,
        }
    }

    pub fn record_batch(&mut self, batch: &[ToolCallSig<'_>]) -> LoopGuardDecision {
        if batch.is_empty() {
            return LoopGuardDecision::Continue;
        }

        let hash = hash_batch(batch);
        self.recent_hashes.push_back(hash.clone());
        while self.recent_hashes.len() > self.window {
            let _ = self.recent_hashes.pop_front();
        }

        let mut counts: HashMap<&str, usize> = HashMap::new();
        for h in &self.recent_hashes {
            *counts.entry(h.as_str()).or_insert(0) += 1;
        }

        let occurrences = counts.get(hash.as_str()).copied().unwrap_or(0);
        if occurrences < self.repetition_threshold {
            return LoopGuardDecision::Continue;
        }

        self.loops_detected += 1;
        if self.loops_detected >= self.max_loops_detected {
            LoopGuardDecision::Break {
                hash,
                occurrences,
                loops_detected: self.loops_detected,
            }
        } else {
            LoopGuardDecision::Warn {
                hash,
                occurrences,
                loops_detected: self.loops_detected,
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolCallSig<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

fn hash_batch(batch: &[ToolCallSig<'_>]) -> String {
    let mut hasher = Sha256::new();
    for call in batch {
        hasher.update(call.name.as_bytes());
        hasher.update(b"\n");
        hasher.update(normalize_args(call.arguments).as_bytes());
        hasher.update(b"\n--\n");
    }
    hex::encode(hasher.finalize())
}

fn normalize_args(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => v.to_string(),
        Err(_) => raw.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call<'a>(name: &'a str, arguments: &'a str) -> ToolCallSig<'a> {
        ToolCallSig { name, arguments }
    }

    #[test]
    fn test_detects_single_tool_repetition() {
        let mut guard = LoopGuard::new(10, 3, 3);
        assert_eq!(
            guard.record_batch(&[call("web_search", r#"{"q":"rust"}"#)]),
            LoopGuardDecision::Continue
        );
        assert_eq!(
            guard.record_batch(&[call("web_search", r#"{"q":"rust"}"#)]),
            LoopGuardDecision::Continue
        );
        match guard.record_batch(&[call("web_search", r#"{"q":"rust"}"#)]) {
            LoopGuardDecision::Warn { occurrences, .. } => assert_eq!(occurrences, 3),
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[test]
    fn test_detects_ping_pong_batch_repetition() {
        let mut guard = LoopGuard::new(10, 3, 3);
        let batch = [call("tool_a", r#"{"x":1}"#), call("tool_b", r#"{"y":2}"#)];
        assert_eq!(guard.record_batch(&batch), LoopGuardDecision::Continue);
        assert_eq!(guard.record_batch(&batch), LoopGuardDecision::Continue);
        match guard.record_batch(&batch) {
            LoopGuardDecision::Warn { occurrences, .. } => assert_eq!(occurrences, 3),
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[test]
    fn test_circuit_breaker_after_n_detections() {
        let mut guard = LoopGuard::new(10, 2, 2);
        let batch = [call("echo", r#"{"message":"x"}"#)];
        assert_eq!(guard.record_batch(&batch), LoopGuardDecision::Continue);
        assert!(matches!(
            guard.record_batch(&batch),
            LoopGuardDecision::Warn { .. }
        ));
        assert!(matches!(
            guard.record_batch(&batch),
            LoopGuardDecision::Break { .. }
        ));
    }
}
