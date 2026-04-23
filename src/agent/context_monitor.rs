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

use crate::config::CompactionConfig;
use crate::providers::ToolDefinition;
use crate::session::{ContentPart, Message, Role};

// --- Token estimation constants (ported from OpenClaw) ---

/// Average characters per token for general text.
const CHARS_PER_TOKEN: f64 = 4.0;

/// Characters per token for tool results / code / JSON (tokenizes denser).
const TOOL_RESULT_CHARS_PER_TOKEN: f64 = 2.0;

/// Flat token estimate per image (regardless of resolution).
const IMAGE_TOKEN_ESTIMATE: usize = 2000;

/// Per-message framing overhead (role markers, delimiters).
const MESSAGE_FRAMING_TOKENS: usize = 4;

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

/// Result of a pre-flight context size check.
#[derive(Debug, Clone, PartialEq)]
pub enum PreflightAction {
    /// Context fits within budget — proceed normally.
    Ok,
    /// Oversized tool results were trimmed in-memory — proceed with modified messages.
    Trimmed,
    /// Context still too large after trimming — caller should trigger full compaction.
    NeedsCompaction,
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
    /// Fraction of context_limit usable for messages.
    input_headroom_ratio: f64,
    /// Maximum fraction of context window for a single tool result.
    single_tool_result_share: f64,
    /// Safety margin multiplier for estimates.
    safety_margin: f64,
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
            input_headroom_ratio: 0.75,
            single_tool_result_share: 0.50,
            safety_margin: 1.2,
        }
    }

    /// Create from a `CompactionConfig`.
    pub fn from_config(config: &CompactionConfig) -> Self {
        Self {
            context_limit: config.context_limit,
            threshold: config.threshold,
            emergency_threshold: config.emergency_threshold,
            critical_threshold: config.critical_threshold,
            input_headroom_ratio: config.input_headroom_ratio,
            single_tool_result_share: config.single_tool_result_share,
            safety_margin: config.safety_margin,
        }
    }

    /// Estimate the total token count for a slice of messages.
    ///
    /// Accounts for message text, content parts (text + images), tool calls
    /// (name + arguments), and tool result weighting. Applies a safety margin
    /// to compensate for heuristic inaccuracy.
    pub fn estimate_tokens(messages: &[Message]) -> usize {
        Self::estimate_tokens_with_margin(messages, 1.2)
    }

    /// Estimate tokens with a custom safety margin (1.0 = no margin).
    pub fn estimate_tokens_with_margin(messages: &[Message], safety_margin: f64) -> usize {
        let raw: f64 = messages.iter().map(Self::estimate_message_tokens).sum();
        (raw * safety_margin) as usize
    }

    /// Estimate tokens for a single message.
    fn estimate_message_tokens(msg: &Message) -> f64 {
        let is_tool_result = msg.role == Role::Tool;
        let cpt = if is_tool_result {
            TOOL_RESULT_CHARS_PER_TOKEN
        } else {
            CHARS_PER_TOKEN
        };

        // Estimate from plain content string
        let content_tokens = msg.content.len() as f64 / cpt;

        // Estimate from structured content parts
        let parts_tokens: f64 = if msg.content_parts.is_empty() {
            0.0
        } else {
            msg.content_parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => text.len() as f64 / cpt,
                    ContentPart::Image { .. } => IMAGE_TOKEN_ESTIMATE as f64,
                })
                .sum()
        };

        // Use the larger of the two to avoid double-counting
        let body_tokens = content_tokens.max(parts_tokens);

        // Tool call arguments (assistant messages with tool_use)
        let tool_call_tokens: f64 = msg
            .tool_calls
            .as_ref()
            .map(|calls| {
                calls
                    .iter()
                    .map(|tc| {
                        tc.name.len() as f64 / CHARS_PER_TOKEN
                            + tc.arguments.len() as f64 / TOOL_RESULT_CHARS_PER_TOKEN
                    })
                    .sum()
            })
            .unwrap_or(0.0);

        body_tokens + tool_call_tokens + MESSAGE_FRAMING_TOKENS as f64
    }

    /// Estimate total tokens including tool definitions and system prompt overhead.
    ///
    /// Use this for pre-flight checks where the full request payload matters.
    pub fn estimate_tokens_full(
        messages: &[Message],
        tool_definitions: &[ToolDefinition],
        safety_margin: f64,
    ) -> usize {
        let msg_tokens: f64 = messages.iter().map(Self::estimate_message_tokens).sum();

        let tool_def_tokens: f64 = tool_definitions
            .iter()
            .map(|td| {
                let schema = td.parameters.to_string();
                (td.name.len() + td.description.len()) as f64 / CHARS_PER_TOKEN
                    + schema.len() as f64 / TOOL_RESULT_CHARS_PER_TOKEN
            })
            .sum();

        ((msg_tokens + tool_def_tokens) * safety_margin) as usize
    }

    /// Effective token budget for messages (context_limit * input_headroom_ratio).
    pub fn context_budget(&self) -> usize {
        (self.context_limit as f64 * self.input_headroom_ratio) as usize
    }

    /// Check whether the messages exceed the compaction threshold.
    pub fn needs_compaction(&self, messages: &[Message]) -> bool {
        let estimated = Self::estimate_tokens_with_margin(messages, self.safety_margin);
        estimated as f64 > self.threshold * self.context_limit as f64
    }

    /// Determine compaction urgency tier based on fullness ratio.
    pub fn urgency(&self, messages: &[Message]) -> Option<CompactionUrgency> {
        let estimated = Self::estimate_tokens_with_margin(messages, self.safety_margin);
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
    pub fn suggest_strategy(&self, messages: &[Message]) -> CompactionStrategy {
        let estimated = Self::estimate_tokens_with_margin(messages, self.safety_margin);
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

    /// Pre-flight context guard — runs before each provider call.
    ///
    /// Trims oversized tool results in-place and checks if the total context
    /// fits within the budget. Returns the action taken.
    pub fn preflight_check(
        &self,
        messages: &mut [Message],
        tool_definitions: &[ToolDefinition],
    ) -> PreflightAction {
        let budget = self.context_budget();
        let single_result_cap_tokens =
            (self.context_limit as f64 * self.single_tool_result_share) as usize;
        let single_result_cap_chars =
            (single_result_cap_tokens as f64 * TOOL_RESULT_CHARS_PER_TOKEN) as usize;
        let mut trimmed = false;

        // Pass 1: cap any single tool result that exceeds single_tool_result_share
        for msg in messages.iter_mut() {
            if msg.role == Role::Tool && msg.content.len() > single_result_cap_chars {
                let original_len = msg.content.len();
                let head_budget = single_result_cap_chars * 7 / 10; // 70% head
                let tail_budget = single_result_cap_chars * 3 / 10; // 30% tail

                let head = safe_truncate(&msg.content, head_budget);
                let tail = safe_truncate_tail(&msg.content, tail_budget);
                let truncated = original_len - head.len() - tail.len();

                msg.content = format!("{head}\n...[truncated {truncated} bytes]...\n{tail}");
                // Also update content_parts if populated
                if !msg.content_parts.is_empty() {
                    msg.content_parts = vec![ContentPart::Text {
                        text: msg.content.clone(),
                    }];
                }
                trimmed = true;
            }
        }

        // Pass 2: if message tokens alone exceed the message budget, replace
        // oldest tool results.  We use message-only estimates here because
        // `context_budget()` already reserves headroom for tool definitions,
        // system prompt, and output tokens.
        let total = Self::estimate_tokens_with_margin(messages, self.safety_margin);
        if total > budget {
            // Collect indices of tool result messages (oldest first)
            let tool_indices: Vec<usize> = messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m.role == Role::Tool && m.content != "[compacted]")
                .map(|(i, _)| i)
                .collect();

            for idx in tool_indices {
                messages[idx].content = "[compacted]".to_string();
                if !messages[idx].content_parts.is_empty() {
                    messages[idx].content_parts = vec![ContentPart::Text {
                        text: "[compacted]".to_string(),
                    }];
                }
                trimmed = true;

                let new_total = Self::estimate_tokens_with_margin(messages, self.safety_margin);
                if new_total <= budget {
                    break;
                }
            }
        }

        // Pass 3: check if still over the configured thresholds
        let final_total =
            Self::estimate_tokens_full(messages, tool_definitions, self.safety_margin);
        let hard_limit = (self.context_limit as f64 * self.emergency_threshold) as usize;
        let compaction_limit = budget.min(hard_limit);
        if final_total > compaction_limit {
            return PreflightAction::NeedsCompaction;
        }

        if trimmed {
            PreflightAction::Trimmed
        } else {
            PreflightAction::Ok
        }
    }
}

/// Truncate a string to at most `max_bytes`, respecting char boundaries.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Take the last `max_bytes` of a string, respecting char boundaries.
fn safe_truncate_tail(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

impl Default for ContextMonitor {
    fn default() -> Self {
        Self {
            context_limit: 180_000,
            threshold: 0.70,
            emergency_threshold: 0.90,
            critical_threshold: 0.95,
            input_headroom_ratio: 0.75,
            single_tool_result_share: 0.50,
            safety_margin: 1.2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ToolCall;

    fn make_message(content: &str) -> Message {
        Message::user(content)
    }

    /// Helper: estimate a single message with no safety margin.
    fn raw_estimate(msg: &Message) -> usize {
        ContextMonitor::estimate_tokens_with_margin(std::slice::from_ref(msg), 1.0)
    }

    // --- Token estimation tests ---

    #[test]
    fn test_estimate_tokens_empty_messages() {
        let messages: Vec<Message> = vec![];
        assert_eq!(
            ContextMonitor::estimate_tokens_with_margin(&messages, 1.0),
            0
        );
    }

    #[test]
    fn test_estimate_tokens_single_message() {
        // "Hello world" = 11 chars => 11/4 + 4 = 6.75 => 6 (no margin)
        let messages = vec![make_message("Hello world")];
        assert_eq!(
            ContextMonitor::estimate_tokens_with_margin(&messages, 1.0),
            6
        );
    }

    #[test]
    fn test_estimate_tokens_empty_content() {
        // Empty string = 0 chars => 0/4 + 4 = 4
        let messages = vec![make_message("")];
        assert_eq!(
            ContextMonitor::estimate_tokens_with_margin(&messages, 1.0),
            4
        );
    }

    #[test]
    fn test_estimate_tokens_with_safety_margin() {
        // "Hello world" = 11 chars => (11/4 + 4) * 1.2 = 6.75 * 1.2 = 8.1 => 8
        let messages = vec![make_message("Hello world")];
        assert_eq!(ContextMonitor::estimate_tokens(&messages), 8);
    }

    #[test]
    fn test_tool_result_weighted_heavier() {
        let text = "a]".repeat(100); // 200 chars
        let user_msg = Message::user(&text);
        let tool_msg = Message::tool_result("call_1", &text);

        let user_tokens = raw_estimate(&user_msg);
        let tool_tokens = raw_estimate(&tool_msg);
        // Tool results use 2 chars/token vs 4 chars/token for user
        // user: 200/4 + 4 = 54, tool: 200/2 + 4 = 104
        assert!(
            tool_tokens > user_tokens,
            "Tool results should be weighted heavier: tool={tool_tokens} user={user_tokens}"
        );
    }

    #[test]
    fn test_tool_call_arguments_counted() {
        let mut msg = Message::assistant("ok");
        msg.tool_calls = Some(vec![ToolCall::new(
            "call_1",
            "shell",
            r#"{"command": "ls -la /very/long/path/to/something"}"#,
        )]);
        let with_calls = raw_estimate(&msg);

        let plain = raw_estimate(&Message::assistant("ok"));
        assert!(
            with_calls > plain,
            "Tool call arguments should add tokens: with={with_calls} plain={plain}"
        );
    }

    #[test]
    fn test_estimate_tokens_full_includes_tool_defs() {
        let messages = vec![make_message("Hello")];
        let tool_defs = vec![ToolDefinition::new(
            "shell",
            "Execute a shell command on the system",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run"}
                },
                "required": ["command"]
            }),
        )];
        let without = ContextMonitor::estimate_tokens_full(&messages, &[], 1.0);
        let with = ContextMonitor::estimate_tokens_full(&messages, &tool_defs, 1.0);
        assert!(
            with > without,
            "Tool definitions should add tokens: with={with} without={without}"
        );
    }

    // --- Urgency tests (use larger context limits to accommodate new estimation) ---

    #[test]
    fn test_urgency_tiers() {
        // Use a context_limit where we can control the ratio precisely.
        // Each "x]" repeated 100 times = 200 chars => raw ~54 tokens (200/4+4) => with 1.2 margin ~64
        let monitor = ContextMonitor::new_with_thresholds(1000, 0.70, 0.90, 0.95);

        // Below threshold: need < 700 estimated tokens
        let small: Vec<Message> = (0..5).map(|_| make_message("hello")).collect();
        assert_eq!(monitor.urgency(&small), None);

        // Build messages to exceed critical (>950)
        // 200-char message with margin: ~64 tokens each. Need 950/64 ≈ 15 messages.
        let text = "x".repeat(200);
        let critical: Vec<Message> = (0..20).map(|_| make_message(&text)).collect();
        let est = ContextMonitor::estimate_tokens(&critical);
        assert!(est > 950, "Expected >950, got {est}");
        assert_eq!(
            monitor.urgency(&critical),
            Some(CompactionUrgency::Critical)
        );
    }

    // --- needs_compaction tests ---

    #[test]
    fn test_needs_compaction_below_threshold() {
        let monitor = ContextMonitor::new(10_000, 0.80);
        let messages = vec![make_message("Hello")];
        assert!(!monitor.needs_compaction(&messages));
    }

    #[test]
    fn test_needs_compaction_above_threshold() {
        // context_limit=100, threshold=0.80 => trigger at >80 tokens
        // Need many chars to get there. 300 chars => 300/4+4 = 79 raw * 1.2 = 94.8 => 94 > 80
        let monitor = ContextMonitor::new(100, 0.80);
        let messages = vec![make_message(&"x".repeat(300))];
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
        // context_limit=1000, threshold=0.80 => trigger at >800
        // Need ratio between 0.80 and 0.85 => 800-850 estimated tokens
        // 2500 chars => (2500/4+4)*1.2 = 629*1.2 = 754.8. Too low.
        // 2700 chars => (2700/4+4)*1.2 = 679*1.2 = 814.8 => 814. Ratio=0.814, above 0.80 below 0.85
        let monitor = ContextMonitor::new(1000, 0.80);
        // 2700 chars in one message: (2700/4+4)*1.2 = 679*1.2 ≈ 814
        let messages = vec![make_message(&"x".repeat(2700))];
        let est = ContextMonitor::estimate_tokens(&messages);
        assert!(est > 800 && est < 850, "Expected 800-850, got {est}");
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::Summarize { keep_recent: 8 }
        );
    }

    #[test]
    fn test_strategy_above_85() {
        // context_limit=1000, need ratio > 0.85 but < 0.90
        // 2900 chars: (2900/4+4)*1.2 = 729*1.2 ≈ 874. Ratio=0.874
        let monitor = ContextMonitor::new(1000, 0.80);
        let messages = vec![make_message(&"x".repeat(2900))];
        let est = ContextMonitor::estimate_tokens(&messages);
        assert!(est > 850 && est < 900, "Expected 850-900, got {est}");
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::Summarize { keep_recent: 5 }
        );
    }

    #[test]
    fn test_strategy_above_95() {
        // context_limit=1000, need ratio > 0.95
        // 3200 chars: (3200/4+4)*1.2 = 804*1.2 ≈ 964. Ratio=0.964 > 0.95
        let monitor = ContextMonitor::new(1000, 0.80);
        let messages = vec![make_message(&"x".repeat(3200))];
        let est = ContextMonitor::estimate_tokens(&messages);
        assert!(est > 950, "Expected >950, got {est}");
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
        // "Hello world" = 11 chars => (11/4+4)*1.2 ≈ 8 tokens, below 10
        let messages = vec![make_message("Hello world")];
        assert!(!monitor.needs_compaction(&messages));

        // Two messages => ~16 tokens, above 10
        let messages = vec![make_message("Hello world"), make_message("Hello world")];
        assert!(monitor.needs_compaction(&messages));
    }

    #[test]
    fn test_default_values() {
        let monitor = ContextMonitor::default();
        let messages = vec![make_message("Hello"), make_message("World")];
        assert!(!monitor.needs_compaction(&messages));
        assert_eq!(
            monitor.suggest_strategy(&messages),
            CompactionStrategy::None
        );
    }

    // --- Pre-flight guard tests ---

    #[test]
    fn test_preflight_ok_when_small() {
        let monitor = ContextMonitor::default();
        let mut messages = vec![make_message("Hello")];
        assert_eq!(
            monitor.preflight_check(&mut messages, &[]),
            PreflightAction::Ok
        );
    }

    #[test]
    fn test_preflight_trims_oversized_tool_result() {
        // context_limit=1000, single_tool_result_share=0.5 => cap at 500 tokens
        // 500 tokens * 2 chars/token = 1000 chars cap
        let monitor = ContextMonitor::new(1000, 0.70);
        let big_result = "x".repeat(5000); // way over 1000 chars
        let mut messages = vec![
            Message::user("hi"),
            Message::tool_result("call_1", &big_result),
        ];
        let action = monitor.preflight_check(&mut messages, &[]);
        assert!(
            matches!(action, PreflightAction::Trimmed | PreflightAction::Ok),
            "Expected trimmed or ok, got {action:?}"
        );
        assert!(
            messages[1].content.len() < 5000,
            "Tool result should have been trimmed"
        );
    }

    #[test]
    fn test_from_config() {
        let config = CompactionConfig {
            enabled: true,
            context_limit: 50_000,
            threshold: 0.60,
            emergency_threshold: 0.85,
            critical_threshold: 0.90,
            input_headroom_ratio: 0.70,
            single_tool_result_share: 0.40,
            safety_margin: 1.3,
            overflow_retries: 5,
        };
        let monitor = ContextMonitor::from_config(&config);
        assert_eq!(monitor.context_budget(), 35_000); // 50_000 * 0.70
    }

    // --- Preflight Pass 2: batch compaction of old tool results ---

    #[test]
    fn test_preflight_pass2_compacts_oldest_tool_results() {
        // context_limit=200, input_headroom_ratio=0.75 → budget=150 tokens
        // single_tool_result_share=0.90 → each result cap is huge (won't trigger pass 1)
        // But total will exceed budget, so pass 2 should compact oldest tool results.
        let config = CompactionConfig {
            enabled: true,
            context_limit: 200,
            threshold: 0.70,
            emergency_threshold: 0.90,
            critical_threshold: 0.95,
            input_headroom_ratio: 0.75,
            single_tool_result_share: 0.90,
            safety_margin: 1.0, // no margin for precise control
            overflow_retries: 3,
        };
        let monitor = ContextMonitor::from_config(&config);
        // budget = 200 * 0.75 = 150 tokens
        assert_eq!(monitor.context_budget(), 150);

        // Each 400-char tool result = 400/2 + 4 = 204 tokens (tool result uses 2 chars/token)
        // 3 of them = 612 tokens > 150. Pass 2 should compact oldest first.
        let mut messages = vec![
            Message::tool_result("c1", &"a".repeat(400)),
            Message::tool_result("c2", &"b".repeat(400)),
            Message::tool_result("c3", &"c".repeat(400)),
        ];

        let action = monitor.preflight_check(&mut messages, &[]);
        // Oldest results should have been replaced with "[compacted]"
        let compacted_count = messages
            .iter()
            .filter(|m| m.content == "[compacted]")
            .count();
        assert!(
            compacted_count > 0,
            "Pass 2 should have compacted some tool results, got 0"
        );
        // The newest result should ideally survive (or at least fewer compacted than total)
        assert!(
            matches!(
                action,
                PreflightAction::Trimmed | PreflightAction::NeedsCompaction
            ),
            "Expected Trimmed or NeedsCompaction, got {action:?}"
        );
    }

    #[test]
    fn test_preflight_pass2_syncs_content_parts() {
        let config = CompactionConfig {
            enabled: true,
            context_limit: 100,
            input_headroom_ratio: 0.50,
            single_tool_result_share: 0.90,
            safety_margin: 1.0,
            ..Default::default()
        };
        let monitor = ContextMonitor::from_config(&config);

        let mut messages = vec![Message::tool_result("c1", &"x".repeat(400))];
        // Verify content_parts is populated
        assert!(!messages[0].content_parts.is_empty());

        monitor.preflight_check(&mut messages, &[]);
        // If compacted, content_parts should be in sync
        if messages[0].content == "[compacted]" {
            assert_eq!(messages[0].content_parts.len(), 1);
            if let ContentPart::Text { text } = &messages[0].content_parts[0] {
                assert_eq!(text, "[compacted]");
            }
        }
    }

    // --- Preflight Pass 3: NeedsCompaction ---

    #[test]
    fn test_preflight_pass3_needs_compaction() {
        // Even after compacting all tool results, if non-tool messages push us
        // over 90% of context_limit, we should get NeedsCompaction.
        let config = CompactionConfig {
            enabled: true,
            context_limit: 50,
            input_headroom_ratio: 0.50,
            single_tool_result_share: 0.90,
            safety_margin: 1.0,
            ..Default::default()
        };
        let monitor = ContextMonitor::from_config(&config);
        // budget = 25 tokens, 90% threshold = 45 tokens
        // A single user message with 200 chars = 200/4+4 = 54 tokens > 45
        // No tool results to compact, so pass 2 does nothing, pass 3 fires.
        let mut messages = vec![Message::user(&"x".repeat(200))];

        let action = monitor.preflight_check(&mut messages, &[]);
        assert_eq!(action, PreflightAction::NeedsCompaction);
    }

    // --- Image token estimation ---

    #[test]
    fn test_estimate_tokens_with_image() {
        use crate::session::ImageSource;

        let mut msg = Message::user("caption");
        msg.content_parts = vec![
            ContentPart::Text {
                text: "caption".to_string(),
            },
            ContentPart::Image {
                source: ImageSource::Base64 {
                    data: "abc".to_string(),
                },
                media_type: "image/png".to_string(),
            },
        ];
        let tokens = raw_estimate(&msg);
        // "caption" = 7 chars → 7/4 = 1.75 tokens
        // Image = 2000 tokens
        // Total parts = 2001.75 > content_tokens (7/4=1.75)
        // So parts_tokens dominates. + 4 framing = ~2005
        assert!(
            tokens > 2000,
            "Image should contribute ~2000 tokens, got {tokens}"
        );
    }

    // --- content_parts vs content max logic ---

    #[test]
    fn test_estimate_uses_max_of_content_and_parts() {
        // content is short but content_parts has more text
        let mut msg = Message::user("short");
        msg.content_parts = vec![ContentPart::Text {
            text: "x".repeat(1000),
        }];
        let tokens = raw_estimate(&msg);
        // content: 5/4 = 1.25 tokens
        // parts: 1000/4 = 250 tokens
        // max(1.25, 250) + 4 = 254
        assert!(
            tokens > 200,
            "Should use content_parts estimate when larger, got {tokens}"
        );
    }

    // --- Urgency: Normal and Emergency tiers ---

    #[test]
    fn test_urgency_normal_tier() {
        let monitor = ContextMonitor::new_with_thresholds(1000, 0.70, 0.90, 0.95);
        // Need ratio between 0.70 and 0.90 → 700-900 estimated tokens
        // 2400 chars: (2400/4+4)*1.2 = 604*1.2 ≈ 724. Ratio = 0.724 → Normal
        let messages = vec![make_message(&"x".repeat(2400))];
        assert_eq!(monitor.urgency(&messages), Some(CompactionUrgency::Normal));
    }

    #[test]
    fn test_urgency_emergency_tier() {
        let monitor = ContextMonitor::new_with_thresholds(1000, 0.70, 0.90, 0.95);
        // Need ratio between 0.90 and 0.95 → 900-950 estimated tokens
        // 3050 chars: (3050/4+4)*1.2 = 766.5*1.2 ≈ 919. Ratio = 0.919 → Emergency
        let messages = vec![make_message(&"x".repeat(3050))];
        let est = ContextMonitor::estimate_tokens(&messages);
        assert!(est >= 900 && est < 950, "Expected 900-950, got {est}");
        assert_eq!(
            monitor.urgency(&messages),
            Some(CompactionUrgency::Emergency)
        );
    }
}
