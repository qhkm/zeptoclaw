//! Context monitor for token estimation and threshold detection.
//!
//! Provides heuristic-based token estimation from conversation messages
//! and suggests compaction strategies when the context window is getting full.
//!
//! # Token Estimation
//!
//! Uses the heuristic: `words * 1.3 + 4` per message, where the 4 accounts
//! for message framing overhead (role markers, delimiters).
//!
//! # Compaction Strategies
//!
//! When estimated tokens exceed the configured threshold:
//! - **Summarize**: Ask the LLM to compress older messages into a summary
//! - **Truncate**: Drop oldest messages entirely (emergency, near-limit)
//!
//! # Example
//!
//! ```rust
//! use zeptoclaw::agent::context_monitor::{ContextMonitor, CompactionStrategy};
//! use zeptoclaw::session::Message;
//!
//! let monitor = ContextMonitor::new(100_000, 0.80);
//! let messages = vec![Message::user("Hello, world!")];
//!
//! assert!(!monitor.needs_compaction(&messages));
//! assert_eq!(monitor.suggest_strategy(&messages), CompactionStrategy::None);
//! ```

use crate::session::Message;

/// Strategy suggested when context is getting too large.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionStrategy {
    /// No compaction needed.
    None,
    /// Summarize oldest messages, keeping `keep_recent` most recent.
    Summarize { keep_recent: usize },
    /// Drop oldest messages, keeping `keep_recent` most recent.
    Truncate { keep_recent: usize },
}

/// Compaction urgency tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionUrgency {
    Normal,
    Emergency,
    Critical,
}

/// Monitors conversation context size and suggests compaction strategies.
///
/// Uses heuristic token estimation to detect when the conversation is
/// approaching the context window limit, and recommends an appropriate
/// compaction strategy based on how full the context is.
pub struct ContextMonitor {
    /// Maximum token capacity of the context window.
    context_limit: usize,
    /// Fraction (0.0-1.0) of context_limit at which compaction is suggested.
    threshold: f64,
    /// Fraction for emergency truncation behavior.
    emergency_threshold: f64,
    /// Fraction for critical hard-trim behavior.
    critical_threshold: f64,
}

impl ContextMonitor {
    /// Create a new context monitor.
    ///
    /// # Arguments
    /// * `context_limit` - Maximum token capacity (e.g. 100_000)
    /// * `threshold` - Fraction of limit that triggers compaction (e.g. 0.80)
    pub fn new(context_limit: usize, threshold: f64) -> Self {
        Self::new_with_thresholds(context_limit, threshold, 0.90, 0.95)
    }

    /// Create a new context monitor with explicit normal/emergency/critical thresholds.
    pub fn new_with_thresholds(
        context_limit: usize,
        threshold: f64,
        emergency_threshold: f64,
        critical_threshold: f64,
    ) -> Self {
        Self {
            context_limit,
            threshold,
            emergency_threshold,
            critical_threshold,
        }
    }

    /// Estimate the total token count for a slice of messages.
    ///
    /// Uses the heuristic: for each message, count words in content,
    /// multiply by 1.3, then add 4 for message framing overhead.
    ///
    /// # Arguments
    /// * `messages` - The conversation messages to estimate
    ///
    /// # Returns
    /// Estimated token count (rounded down to nearest integer).
    pub fn estimate_tokens(messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|msg| {
                let word_count = msg.content.split_whitespace().count();
                (word_count as f64 * 1.3 + 4.0) as usize
            })
            .sum()
    }

    /// Check whether the messages exceed the compaction threshold.
    ///
    /// # Arguments
    /// * `messages` - The conversation messages to check
    ///
    /// # Returns
    /// `true` if estimated tokens exceed `threshold * context_limit`.
    pub fn needs_compaction(&self, messages: &[Message]) -> bool {
        let estimated = Self::estimate_tokens(messages);
        estimated as f64 > self.threshold * self.context_limit as f64
    }

    /// Determine compaction urgency tier based on fullness ratio.
    pub fn urgency(&self, messages: &[Message]) -> Option<CompactionUrgency> {
        let estimated = Self::estimate_tokens(messages);
        let ratio = estimated as f64 / self.context_limit as f64;
        if ratio <= self.threshold {
            None
        } else if ratio >= self.critical_threshold {
            Some(CompactionUrgency::Critical)
        } else if ratio >= self.emergency_threshold {
            Some(CompactionUrgency::Emergency)
        } else {
            Some(CompactionUrgency::Normal)
        }
    }

    /// Suggest a compaction strategy based on current context fullness.
    ///
    /// Returns:
    /// - `None` if below the threshold
    /// - `Truncate { keep_recent: 3 }` if above 95% of the limit
    /// - `Summarize { keep_recent: 5 }` if above 85% of the limit
    /// - `Summarize { keep_recent: 8 }` if above the threshold but below 85%
    ///
    /// # Arguments
    /// * `messages` - The conversation messages to evaluate
    pub fn suggest_strategy(&self, messages: &[Message]) -> CompactionStrategy {
        let estimated = Self::estimate_tokens(messages);
        let ratio = estimated as f64 / self.context_limit as f64;

        match self.urgency(messages) {
            None => CompactionStrategy::None,
            Some(CompactionUrgency::Critical) => CompactionStrategy::Truncate { keep_recent: 3 },
            Some(CompactionUrgency::Emergency) => CompactionStrategy::Truncate { keep_recent: 5 },
            Some(CompactionUrgency::Normal) => {
                if ratio > 0.85 {
                    CompactionStrategy::Summarize { keep_recent: 5 }
                } else {
                    CompactionStrategy::Summarize { keep_recent: 8 }
                }
            }
        }
    }
}

impl Default for ContextMonitor {
    fn default() -> Self {
        Self {
            context_limit: 100_000,
            threshold: 0.70,
            emergency_threshold: 0.90,
            critical_threshold: 0.95,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(content: &str) -> Message {
        Message::user(content)
    }

    // --- Token estimation tests ---

    #[test]
    fn test_estimate_tokens_empty_messages() {
        let messages: Vec<Message> = vec![];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 0);
    }

    #[test]
    fn test_estimate_tokens_single_message() {
        // "Hello world" = 2 words => 2 * 1.3 + 4 = 6.6 => 6
        let messages = vec![make_message("Hello world")];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 6);
    }

    #[test]
    fn test_estimate_tokens_multiple_messages() {
        let messages = vec![
            make_message("Hello world"),       // 2 words => 2*1.3+4 = 6.6 => 6
            make_message("How are you today"), // 4 words => 4*1.3+4 = 9.2 => 9
        ];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 6 + 9);
    }

    #[test]
    fn test_estimate_tokens_empty_content() {
        // Empty string = 0 words => 0*1.3+4 = 4
        let messages = vec![make_message("")];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 4);
    }

    #[test]
    fn test_estimate_tokens_long_content() {
        // 10 words => 10*1.3+4 = 17
        let messages = vec![make_message(
            "one two three four five six seven eight nine ten",
        )];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 17);
    }

    #[test]
    fn test_word_count_with_extra_whitespace() {
        // split_whitespace handles multiple spaces/tabs/newlines
        // "hello   world" = 2 words => 2*1.3+4 = 6.6 => 6
        let messages = vec![make_message("hello   world")];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 6);
    }

    #[test]
    fn test_urgency_normal_emergency_critical() {
        let monitor = ContextMonitor::new_with_thresholds(200, 0.70, 0.90, 0.95);

        let normal: Vec<Message> = (0..9)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        // 9 * 17 = 153 => normal range (>=140 && <180)
        assert_eq!(monitor.urgency(&normal), Some(CompactionUrgency::Normal));

        let emergency: Vec<Message> = (0..11)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        // 11 * 17 = 187 => emergency range (>=180 && <190)
        assert_eq!(
            monitor.urgency(&emergency),
            Some(CompactionUrgency::Emergency)
        );

        let critical: Vec<Message> = (0..12)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        assert_eq!(
            monitor.urgency(&critical),
            Some(CompactionUrgency::Critical)
        );
    }

    // --- needs_compaction tests ---

    #[test]
    fn test_needs_compaction_below_threshold() {
        let monitor = ContextMonitor::new(1000, 0.80);
        // Small message, well below 800 token threshold
        let messages = vec![make_message("Hello")];
        assert!(!monitor.needs_compaction(&messages));
    }

    #[test]
    fn test_needs_compaction_above_threshold() {
        // context_limit=100, threshold=0.80 => trigger at >80 tokens
        // We need many messages to exceed 80 tokens
        // Each 10-word message = 17 tokens, so 5 messages = 85 tokens > 80
        let monitor = ContextMonitor::new(100, 0.80);
        let messages: Vec<Message> = (0..5)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        // 5 * 17 = 85 > 80
        assert!(monitor.needs_compaction(&messages));
    }

    // --- suggest_strategy tests ---

    #[test]
    fn test_strategy_below_threshold() {
        let monitor = ContextMonitor::new(100_000, 0.80);
        let messages = vec![make_message("Hello world")];
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::None
        );
    }

    #[test]
    fn test_strategy_above_threshold_below_85() {
        // Need ratio > 0.80 but <= 0.85
        // context_limit=100, threshold=0.80
        // 82 tokens => ratio=0.82 => Summarize { keep_recent: 8 }
        // Each 10-word msg = 17 tokens. We need ~82 tokens.
        // 4 msgs = 68, 5 msgs = 85. Let's use a mix.
        // 4 * 17 = 68, plus one 6-word msg: 6*1.3+4=11.8=>11, total=79 (below)
        // 4 * 17 = 68, plus one 8-word msg: 8*1.3+4=14.4=>14, total=82 (above 80, below 85)
        let monitor = ContextMonitor::new(100, 0.80);
        let mut messages: Vec<Message> = (0..4)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        messages.push(make_message("one two three four five six seven eight"));
        // Total: 4*17 + 14 = 82
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 82);
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::Summarize { keep_recent: 8 }
        );
    }

    #[test]
    fn test_strategy_above_85() {
        // Need ratio > 0.85 but < emergency threshold (0.90).
        // context_limit=100
        // 5 msgs of 10 words = 85. Add one empty message (4 tokens) => 89.
        let monitor = ContextMonitor::new(100, 0.80);
        let mut messages: Vec<Message> = (0..5)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        messages.push(make_message(""));
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 89);
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::Summarize { keep_recent: 5 }
        );
    }

    #[test]
    fn test_strategy_above_95() {
        // Need ratio > 0.95
        // context_limit=100, need > 95 tokens
        // 6 * 17 = 102 => ratio=1.02 => Truncate { keep_recent: 3 }
        let monitor = ContextMonitor::new(100, 0.80);
        let messages: Vec<Message> = (0..6)
            .map(|_| make_message("one two three four five six seven eight nine ten"))
            .collect();
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 102);
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::Truncate { keep_recent: 3 }
        );
    }

    // --- Edge case tests ---

    #[test]
    fn test_empty_message_list_strategy() {
        let monitor = ContextMonitor::new(100_000, 0.80);
        assert_eq!(monitor.suggest_strategy(&[]), CompactionStrategy::None);
        assert!(!monitor.needs_compaction(&[]));
    }

    #[test]
    fn test_single_message_no_compaction() {
        let monitor = ContextMonitor::new(100_000, 0.80);
        let messages = vec![make_message("Just one message here")];
        assert!(!monitor.needs_compaction(&messages));
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::None
        );
    }

    #[test]
    fn test_custom_threshold() {
        // Very low threshold: 0.10 on limit=100 => trigger at >10 tokens
        let monitor = ContextMonitor::new(100, 0.10);
        // "Hello world" => 6 tokens, below 10
        let messages = vec![make_message("Hello world")];
        assert!(!monitor.needs_compaction(&messages));

        // Two messages => 6+6=12 tokens, above 10
        let messages = vec![make_message("Hello world"), make_message("Hello world")];
        assert!(monitor.needs_compaction(&messages));
    }

    #[test]
    fn test_default_values() {
        let monitor = ContextMonitor::default();
        // Default: context_limit=100_000, threshold=0.80
        // A few messages should be well below threshold
        let messages = vec![make_message("Hello"), make_message("World")];
        assert!(!monitor.needs_compaction(&messages));
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::None
        );
    }
}
