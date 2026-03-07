//! Dedicated tool call counter for per-template max_tool_calls enforcement.
//!
//! This is intentionally NOT wired into LoopGuard, because LoopGuard's
//! global_circuit_breaker only counts repetitions that exceed warn_threshold,
//! not total calls.

use std::sync::atomic::{AtomicU32, Ordering};

/// Tracks total tool calls against an optional hard cap.
///
/// Thread-safe via AtomicU32. When `limit` is None, the tracker never exceeds.
pub struct ToolCallLimitTracker {
    limit: Option<u32>,
    count: AtomicU32,
}

impl ToolCallLimitTracker {
    /// Create a new tracker with an optional limit.
    ///
    /// # Examples
    ///
    /// ```
    /// use zeptoclaw::agent::ToolCallLimitTracker;
    ///
    /// let tracker = ToolCallLimitTracker::new(Some(10));
    /// assert_eq!(tracker.count(), 0);
    /// assert_eq!(tracker.limit(), Some(10));
    /// assert!(!tracker.is_exceeded());
    /// ```
    pub fn new(limit: Option<u32>) -> Self {
        Self {
            limit,
            count: AtomicU32::new(0),
        }
    }

    /// Add `n` tool calls to the counter.
    pub fn increment(&self, n: u32) {
        self.count.fetch_add(n, Ordering::Relaxed);
    }

    /// Returns true if the limit has been reached or exceeded.
    pub fn is_exceeded(&self) -> bool {
        match self.limit {
            None => false,
            Some(max) => self.count.load(Ordering::Relaxed) >= max,
        }
    }

    /// Returns the current count of tool calls.
    pub fn count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    /// Returns the configured limit, if any.
    pub fn limit(&self) -> Option<u32> {
        self.limit
    }

    /// Resets the counter to zero. The limit remains unchanged.
    ///
    /// Called at the start of each `process_message` invocation so that
    /// per-agent-run limits apply to each run independently, not across
    /// the lifetime of the `AgentLoop` struct.
    pub fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
    }

    /// Returns the number of tool calls still allowed before the limit is hit.
    ///
    /// Returns `None` if the tracker has no limit (unlimited).
    /// Returns `Some(0)` if the limit is already reached or exceeded.
    pub fn remaining(&self) -> Option<u32> {
        self.limit
            .map(|max| max.saturating_sub(self.count.load(Ordering::Relaxed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_limit_tracker() {
        let tracker = ToolCallLimitTracker::new(Some(5));
        assert!(!tracker.is_exceeded());
        for _ in 0..5 {
            tracker.increment(1);
        }
        assert!(tracker.is_exceeded());
    }

    #[test]
    fn test_tool_call_limit_tracker_unlimited() {
        let tracker = ToolCallLimitTracker::new(None);
        for _ in 0..1000 {
            tracker.increment(1);
        }
        assert!(!tracker.is_exceeded());
    }

    #[test]
    fn test_tool_call_limit_tracker_batch_increment() {
        let tracker = ToolCallLimitTracker::new(Some(10));
        tracker.increment(3);
        tracker.increment(4);
        assert!(!tracker.is_exceeded());
        tracker.increment(3);
        assert!(tracker.is_exceeded());
    }

    #[test]
    fn test_tool_call_limit_zero_means_immediate() {
        let tracker = ToolCallLimitTracker::new(Some(0));
        assert!(tracker.is_exceeded());
    }

    #[test]
    fn test_tool_call_limit_count_and_limit() {
        let tracker = ToolCallLimitTracker::new(Some(10));
        assert_eq!(tracker.count(), 0);
        assert_eq!(tracker.limit(), Some(10));
        tracker.increment(5);
        assert_eq!(tracker.count(), 5);
    }

    #[test]
    fn test_reset() {
        let tracker = ToolCallLimitTracker::new(Some(5));
        tracker.increment(5);
        assert!(tracker.is_exceeded());
        tracker.reset();
        assert_eq!(tracker.count(), 0);
        assert!(!tracker.is_exceeded());
        assert_eq!(tracker.remaining(), Some(5));
        // Limit unchanged after reset.
        assert_eq!(tracker.limit(), Some(5));
    }

    #[test]
    fn test_remaining_unlimited() {
        let tracker = ToolCallLimitTracker::new(None);
        assert_eq!(tracker.remaining(), None);
        tracker.increment(100);
        assert_eq!(tracker.remaining(), None);
    }

    #[test]
    fn test_remaining_with_limit() {
        let tracker = ToolCallLimitTracker::new(Some(10));
        assert_eq!(tracker.remaining(), Some(10));
        tracker.increment(3);
        assert_eq!(tracker.remaining(), Some(7));
        tracker.increment(7);
        assert_eq!(tracker.remaining(), Some(0));
        tracker.increment(5);
        assert_eq!(tracker.remaining(), Some(0));
    }

    #[test]
    fn test_remaining_zero_limit() {
        let tracker = ToolCallLimitTracker::new(Some(0));
        assert_eq!(tracker.remaining(), Some(0));
    }
}
