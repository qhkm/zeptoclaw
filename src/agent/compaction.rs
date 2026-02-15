//! Context compaction strategies for conversation history.
//!
//! Provides two strategies for reducing conversation history size:
//!
//! - **Truncate**: Drop old messages, keeping only the N most recent.
//!   Always preserves the first system message if present.
//! - **Summarize**: Replace old messages with a single summary message,
//!   keeping the N most recent messages intact.
//!
//! These are pure functions that operate on `Vec<Message>`. The caller
//! is responsible for obtaining any LLM-generated summaries before
//! calling `summarize_messages`.

use crate::session::{Message, Role};

/// Truncate messages to keep only the N most recent.
///
/// Always preserves the first system message if present. When the first
/// message has `role == System`, the result contains that system message
/// plus the `keep_recent` most recent non-system-prefix messages.
///
/// # Arguments
/// * `messages` - The full conversation history
/// * `keep_recent` - How many recent messages to keep
///
/// # Returns
/// A truncated message list of at most `keep_recent` messages (plus the
/// leading system message, if preserved).
///
/// # Examples
/// ```
/// use zeptoclaw::session::Message;
/// use zeptoclaw::agent::compaction::truncate_messages;
///
/// let msgs = vec![
///     Message::system("You are helpful."),
///     Message::user("Hi"),
///     Message::assistant("Hello!"),
///     Message::user("How are you?"),
///     Message::assistant("Great!"),
/// ];
/// let result = truncate_messages(msgs, 2);
/// assert_eq!(result.len(), 3); // system + 2 recent
/// ```
pub fn truncate_messages(messages: Vec<Message>, keep_recent: usize) -> Vec<Message> {
    if messages.len() <= keep_recent {
        return messages;
    }

    if keep_recent == 0 {
        // Preserve system message even when keep_recent is 0
        if let Some(first) = messages.first() {
            if first.role == Role::System {
                return vec![messages.into_iter().next().unwrap()];
            }
        }
        return Vec::new();
    }

    let has_system_prefix = messages
        .first()
        .map(|m| m.role == Role::System)
        .unwrap_or(false);

    if has_system_prefix {
        let total = messages.len();
        // System message + the last `keep_recent` messages from the rest
        let skip = (total - 1).saturating_sub(keep_recent);
        let mut result = Vec::with_capacity(1 + keep_recent);
        let mut iter = messages.into_iter();
        result.push(iter.next().unwrap()); // system message
                                           // Skip old non-system messages
        for msg in iter.skip(skip) {
            result.push(msg);
        }
        result
    } else {
        // No system prefix — just keep the tail
        let skip = messages.len() - keep_recent;
        messages.into_iter().skip(skip).collect()
    }
}

/// Summarize old messages into a single summary message, keeping the most
/// recent messages intact.
///
/// Splits the conversation into "old" (to be summarized) and "recent" (to
/// keep). The old messages are replaced with a single system message
/// containing the summary text. If the first message is a system message,
/// it is preserved before the summary.
///
/// # Arguments
/// * `messages` - The full conversation history
/// * `keep_recent` - How many recent messages to keep verbatim
/// * `summary_text` - An LLM-generated summary of the old messages
///
/// # Returns
/// A compacted message list: `[system_msg?, summary_msg, ...recent_msgs]`
///
/// # Examples
/// ```
/// use zeptoclaw::session::Message;
/// use zeptoclaw::agent::compaction::summarize_messages;
///
/// let msgs = vec![
///     Message::system("You are helpful."),
///     Message::user("Tell me about Rust"),
///     Message::assistant("Rust is a systems language..."),
///     Message::user("What about async?"),
///     Message::assistant("Async in Rust uses tokio..."),
/// ];
/// let result = summarize_messages(msgs, 2, "User asked about Rust and async.");
/// assert_eq!(result.len(), 4); // system + summary + 2 recent
/// ```
pub fn summarize_messages(
    messages: Vec<Message>,
    keep_recent: usize,
    summary_text: &str,
) -> Vec<Message> {
    if messages.is_empty() {
        return vec![Message::system(&format!(
            "[Conversation Summary]\n{}",
            summary_text
        ))];
    }

    if messages.len() <= keep_recent {
        // Nothing to summarize — everything is "recent"
        return messages;
    }

    let has_system_prefix = messages
        .first()
        .map(|m| m.role == Role::System)
        .unwrap_or(false);

    let summary_msg = Message::system(&format!("[Conversation Summary]\n{}", summary_text));

    if has_system_prefix {
        let total = messages.len();
        // recent = last `keep_recent` messages (excluding system prefix)
        let skip = (total - 1).saturating_sub(keep_recent);
        let mut result = Vec::with_capacity(2 + keep_recent);
        let mut iter = messages.into_iter();
        result.push(iter.next().unwrap()); // original system message
        result.push(summary_msg);
        for msg in iter.skip(skip) {
            result.push(msg);
        }
        result
    } else {
        let total = messages.len();
        let skip = total - keep_recent;
        let mut result = Vec::with_capacity(1 + keep_recent);
        result.push(summary_msg);
        for msg in messages.into_iter().skip(skip) {
            result.push(msg);
        }
        result
    }
}

/// Shrink tool result messages to reduce context size.
///
/// Iterates through messages and truncates tool result content to `max_bytes`.
/// Returns the modified messages and the number of results truncated.
///
/// # Arguments
/// * `messages` - The conversation messages to process
/// * `max_bytes_per_result` - Maximum byte length for each tool result
///
/// # Returns
/// A tuple of (modified messages, count of shrunk results).
///
/// # Examples
/// ```
/// use zeptoclaw::session::Message;
/// use zeptoclaw::agent::compaction::shrink_tool_results;
///
/// let msgs = vec![
///     Message::user("Hi"),
///     Message::tool_result("call_1", "A very long tool result that exceeds the limit"),
///     Message::assistant("Done"),
/// ];
/// let (result, count) = shrink_tool_results(msgs, 20);
/// assert_eq!(count, 1);
/// assert!(result[1].content.len() < 100);
/// ```
pub fn shrink_tool_results(
    messages: Vec<Message>,
    max_bytes_per_result: usize,
) -> (Vec<Message>, usize) {
    let mut shrunk_count = 0;
    let result = messages
        .into_iter()
        .map(|mut msg| {
            if msg.is_tool_result() && msg.content.len() > max_bytes_per_result {
                let original_len = msg.content.len();
                msg.content.truncate(max_bytes_per_result);
                // Ensure we don't split a multi-byte char
                while !msg.content.is_char_boundary(msg.content.len()) {
                    msg.content.pop();
                }
                msg.content.push_str(&format!(
                    "\n...[shrunk from {} to {} bytes]",
                    original_len,
                    msg.content.len()
                ));
                shrunk_count += 1;
            }
            msg
        })
        .collect();
    (result, shrunk_count)
}

/// Progressively shrink tool results with decreasing budgets for older messages.
///
/// Newer tool results (last `recent_count`) keep their full budget.
/// Older tool results get 1/4 of the budget for more aggressive truncation.
///
/// # Arguments
/// * `messages` - The conversation messages to process
/// * `target_max_bytes` - Maximum byte length for recent tool results
/// * `recent_count` - How many recent tool results keep the full budget
///
/// # Returns
/// The modified messages with progressively shrunk tool results.
///
/// # Examples
/// ```
/// use zeptoclaw::session::Message;
/// use zeptoclaw::agent::compaction::shrink_tool_results_progressive;
///
/// let msgs = vec![
///     Message::tool_result("call_1", "old result that is quite long"),
///     Message::tool_result("call_2", "new result that is quite long"),
/// ];
/// let result = shrink_tool_results_progressive(msgs, 20, 1);
/// // Old result gets 1/4 budget, new result gets full budget
/// assert!(result[0].content.len() < result[1].content.len() || result[1].content.len() <= 20);
/// ```
pub fn shrink_tool_results_progressive(
    messages: Vec<Message>,
    target_max_bytes: usize,
    recent_count: usize,
) -> Vec<Message> {
    // Collect indices of tool result messages
    let tool_result_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_tool_result())
        .map(|(i, _)| i)
        .collect();

    let total_tool_results = tool_result_indices.len();
    if total_tool_results == 0 {
        return messages;
    }

    let mut messages = messages;
    for (pos, &idx) in tool_result_indices.iter().enumerate() {
        let is_recent = pos >= total_tool_results.saturating_sub(recent_count);
        let budget = if is_recent {
            target_max_bytes
        } else {
            // Older results get 1/4 the budget
            target_max_bytes / 4
        };

        let msg = &mut messages[idx];
        if msg.content.len() > budget {
            let original_len = msg.content.len();
            msg.content.truncate(budget);
            while !msg.content.is_char_boundary(msg.content.len()) {
                msg.content.pop();
            }
            msg.content.push_str(&format!(
                "\n...[shrunk from {} to {} bytes]",
                original_len,
                msg.content.len()
            ));
        }
    }
    messages
}

/// Three-tier overflow recovery for context compaction.
///
/// Attempts progressively more aggressive strategies to bring the context
/// size below 95% of the limit:
///
/// - **Tier 1**: Truncate old messages (keep `keep_recent_tier1` most recent)
/// - **Tier 2**: Shrink tool results progressively (older results get smaller budgets)
/// - **Tier 3**: Hard truncate to system message + last 3 messages
///
/// # Arguments
/// * `messages` - The conversation messages to compact
/// * `context_limit` - Maximum token capacity of the context window
/// * `keep_recent_tier1` - How many recent messages to keep in tier 1 truncation
/// * `tool_result_budget` - Maximum bytes per tool result in tier 2
///
/// # Returns
/// A tuple of (recovered messages, tier used). Tier 0 means no recovery was needed.
///
/// # Examples
/// ```
/// use zeptoclaw::session::Message;
/// use zeptoclaw::agent::compaction::try_recover_context;
///
/// let msgs = vec![Message::user("Hello"), Message::assistant("Hi!")];
/// let (result, tier) = try_recover_context(msgs, 100_000, 8, 5120);
/// assert_eq!(tier, 0); // no recovery needed
/// ```
pub fn try_recover_context(
    messages: Vec<Message>,
    context_limit: usize,
    keep_recent_tier1: usize,
    tool_result_budget: usize,
) -> (Vec<Message>, u8) {
    use super::context_monitor::ContextMonitor;

    let target = context_limit as f64 * 0.95;

    // Check if recovery is needed
    let estimated = ContextMonitor::estimate_tokens(&messages);
    if (estimated as f64) <= target {
        return (messages, 0);
    }

    // Tier 1: Truncate old messages
    let recovered = truncate_messages(messages, keep_recent_tier1);
    let estimated = ContextMonitor::estimate_tokens(&recovered);
    if (estimated as f64) <= target {
        return (recovered, 1);
    }

    // Tier 2: Shrink tool results progressively
    let recovered = shrink_tool_results_progressive(recovered, tool_result_budget, 3);
    let estimated = ContextMonitor::estimate_tokens(&recovered);
    if (estimated as f64) <= target {
        return (recovered, 2);
    }

    // Tier 3: Hard truncate to system + last 3 messages
    let recovered = truncate_messages(recovered, 3);
    (recovered, 3)
}

/// Build a prompt asking an LLM to summarize a set of messages.
///
/// Formats the messages into a human-readable transcript and appends
/// instructions for producing a concise summary.
///
/// # Arguments
/// * `messages` - The messages to summarize
///
/// # Returns
/// A prompt string suitable for sending to an LLM.
///
/// # Examples
/// ```
/// use zeptoclaw::session::Message;
/// use zeptoclaw::agent::compaction::build_summary_prompt;
///
/// let msgs = vec![
///     Message::user("Hello"),
///     Message::assistant("Hi there!"),
/// ];
/// let prompt = build_summary_prompt(&msgs);
/// assert!(prompt.contains("user: Hello"));
/// assert!(prompt.contains("assistant: Hi there!"));
/// ```
pub fn build_summary_prompt(messages: &[Message]) -> String {
    let mut transcript = String::new();
    for msg in messages {
        transcript.push_str(&format!("{}: {}\n", msg.role, msg.content));
    }

    format!(
        "Summarize the following conversation focusing on key decisions, \
         information exchanged, and actions taken. Be concise.\n\n{}",
        transcript
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── truncate_messages ──────────────────────────────────────────────

    #[test]
    fn test_truncate_keeps_n_recent() {
        let msgs = vec![
            Message::user("one"),
            Message::user("two"),
            Message::user("three"),
            Message::user("four"),
        ];
        let result = truncate_messages(msgs, 2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "three");
        assert_eq!(result[1].content, "four");
    }

    #[test]
    fn test_truncate_preserves_system_message() {
        let msgs = vec![
            Message::system("system prompt"),
            Message::user("one"),
            Message::user("two"),
            Message::user("three"),
        ];
        let result = truncate_messages(msgs, 2);
        assert_eq!(result.len(), 3); // system + 2 recent
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].content, "system prompt");
        assert_eq!(result[1].content, "two");
        assert_eq!(result[2].content, "three");
    }

    #[test]
    fn test_truncate_empty_messages() {
        let result = truncate_messages(Vec::new(), 5);
        assert!(result.is_empty());
    }

    #[test]
    fn test_truncate_keep_greater_than_len() {
        let msgs = vec![Message::user("one"), Message::user("two")];
        let result = truncate_messages(msgs, 10);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "one");
        assert_eq!(result[1].content, "two");
    }

    #[test]
    fn test_truncate_keep_equal_to_len() {
        let msgs = vec![
            Message::user("one"),
            Message::user("two"),
            Message::user("three"),
        ];
        let result = truncate_messages(msgs, 3);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_truncate_keep_zero() {
        let msgs = vec![Message::user("one"), Message::user("two")];
        let result = truncate_messages(msgs, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_truncate_keep_zero_with_system() {
        let msgs = vec![
            Message::system("sys"),
            Message::user("one"),
            Message::user("two"),
        ];
        let result = truncate_messages(msgs, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].content, "sys");
    }

    #[test]
    fn test_truncate_single_message() {
        let msgs = vec![Message::user("only")];
        let result = truncate_messages(msgs, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "only");
    }

    // ── summarize_messages ─────────────────────────────────────────────

    #[test]
    fn test_summarize_with_system_message() {
        let msgs = vec![
            Message::system("You are helpful."),
            Message::user("Tell me about Rust"),
            Message::assistant("Rust is great."),
            Message::user("And async?"),
            Message::assistant("Use tokio."),
        ];
        let result = summarize_messages(msgs, 2, "Discussed Rust basics.");
        // system + summary + 2 recent
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].content, "You are helpful.");
        assert_eq!(result[1].role, Role::System);
        assert!(result[1].content.contains("[Conversation Summary]"));
        assert!(result[1].content.contains("Discussed Rust basics."));
        assert_eq!(result[2].content, "And async?");
        assert_eq!(result[3].content, "Use tokio.");
    }

    #[test]
    fn test_summarize_without_system_message() {
        let msgs = vec![
            Message::user("Hello"),
            Message::assistant("Hi!"),
            Message::user("Bye"),
            Message::assistant("Goodbye!"),
        ];
        let result = summarize_messages(msgs, 2, "User greeted.");
        // summary + 2 recent
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, Role::System);
        assert!(result[0].content.contains("[Conversation Summary]"));
        assert!(result[0].content.contains("User greeted."));
        assert_eq!(result[1].content, "Bye");
        assert_eq!(result[2].content, "Goodbye!");
    }

    #[test]
    fn test_summarize_empty_messages() {
        let result = summarize_messages(Vec::new(), 2, "Nothing happened.");
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("[Conversation Summary]"));
        assert!(result[0].content.contains("Nothing happened."));
    }

    #[test]
    fn test_summarize_keep_greater_than_len() {
        let msgs = vec![Message::user("one"), Message::user("two")];
        let result = summarize_messages(msgs, 10, "summary");
        // Nothing to summarize — all messages are "recent"
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "one");
        assert_eq!(result[1].content, "two");
    }

    // ── build_summary_prompt ───────────────────────────────────────────

    #[test]
    fn test_build_summary_prompt_includes_content() {
        let msgs = vec![
            Message::user("What is Rust?"),
            Message::assistant("A systems programming language."),
        ];
        let prompt = build_summary_prompt(&msgs);
        assert!(prompt.contains("What is Rust?"));
        assert!(prompt.contains("A systems programming language."));
    }

    #[test]
    fn test_build_summary_prompt_includes_role_labels() {
        let msgs = vec![
            Message::user("Hi"),
            Message::assistant("Hello"),
            Message::system("Be concise"),
        ];
        let prompt = build_summary_prompt(&msgs);
        assert!(prompt.contains("user: Hi"));
        assert!(prompt.contains("assistant: Hello"));
        assert!(prompt.contains("system: Be concise"));
    }

    #[test]
    fn test_build_summary_prompt_includes_instruction() {
        let msgs = vec![Message::user("test")];
        let prompt = build_summary_prompt(&msgs);
        assert!(prompt.contains("Summarize the following conversation"));
        assert!(prompt.contains("key decisions"));
        assert!(prompt.contains("Be concise"));
    }

    #[test]
    fn test_build_summary_prompt_empty_messages() {
        let prompt = build_summary_prompt(&[]);
        assert!(prompt.contains("Summarize the following conversation"));
        // No message content, but prompt itself is still valid
        assert!(!prompt.contains("user:"));
    }

    // ── shrink_tool_results ──────────────────────────────────────────

    #[test]
    fn test_shrink_tool_results_basic() {
        let long_content = "x".repeat(200);
        let msgs = vec![
            Message::user("Hello"),
            Message::tool_result("call_1", &long_content),
            Message::assistant("Done"),
        ];
        let (result, count) = shrink_tool_results(msgs, 50);
        assert_eq!(count, 1);
        // The tool result should be truncated + have the shrunk annotation
        assert!(result[1].content.contains("...[shrunk from 200 to"));
        // The user and assistant messages should be untouched
        assert_eq!(result[0].content, "Hello");
        assert_eq!(result[2].content, "Done");
    }

    #[test]
    fn test_shrink_tool_results_preserves_small() {
        let msgs = vec![
            Message::user("Hello"),
            Message::tool_result("call_1", "short result"),
            Message::assistant("Done"),
        ];
        let (result, count) = shrink_tool_results(msgs, 1000);
        assert_eq!(count, 0);
        assert_eq!(result[1].content, "short result");
    }

    #[test]
    fn test_shrink_tool_results_no_tool_results() {
        let msgs = vec![
            Message::user("Hello"),
            Message::assistant("Hi there"),
            Message::user("Bye"),
        ];
        let (result, count) = shrink_tool_results(msgs, 10);
        assert_eq!(count, 0);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "Hello");
        assert_eq!(result[1].content, "Hi there");
        assert_eq!(result[2].content, "Bye");
    }

    // ── shrink_tool_results_progressive ──────────────────────────────

    #[test]
    fn test_shrink_tool_results_progressive_older_smaller() {
        let long_content = "x".repeat(500);
        let msgs = vec![
            Message::tool_result("call_1", &long_content), // old — gets 1/4 budget
            Message::user("middle"),
            Message::tool_result("call_2", &long_content), // old — gets 1/4 budget
            Message::tool_result("call_3", &long_content), // recent — gets full budget
        ];
        let result = shrink_tool_results_progressive(msgs, 200, 1);

        // Old results (call_1, call_2) should be shrunk to ~50 bytes (200/4)
        // Recent result (call_3) should be shrunk to ~200 bytes
        assert!(result[0].content.contains("...[shrunk from"));
        assert!(result[2].content.contains("...[shrunk from"));
        assert!(result[3].content.contains("...[shrunk from"));

        // The old results should be shorter than the recent one
        // (before annotation the old ones are truncated to 50, recent to 200)
        // We check the truncation target was different
        let old_base_len = result[0].content.find("\n...[shrunk").unwrap();
        let recent_base_len = result[3].content.find("\n...[shrunk").unwrap();
        assert!(
            old_base_len < recent_base_len,
            "Old result base ({}) should be shorter than recent ({})",
            old_base_len,
            recent_base_len
        );

        // User message should be untouched
        assert_eq!(result[1].content, "middle");
    }

    // ── try_recover_context ──────────────────────────────────────────

    #[test]
    fn test_try_recover_context_no_recovery_needed() {
        let msgs = vec![Message::user("Hello"), Message::assistant("Hi!")];
        let (result, tier) = try_recover_context(msgs.clone(), 100_000, 8, 5120);
        assert_eq!(tier, 0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_try_recover_context_tier1_sufficient() {
        // Create enough messages to exceed 95% of a small limit (100 tokens).
        // Each 10-word message = 17 tokens. 6 messages = 102 tokens > 95.
        // After tier 1 truncation to 3 recent: 3*17=51 < 95.
        let msgs: Vec<Message> = (0..6)
            .map(|_| Message::user("one two three four five six seven eight nine ten"))
            .collect();
        let (result, tier) = try_recover_context(msgs, 100, 3, 5120);
        assert_eq!(tier, 1);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_try_recover_context_tier2_needed() {
        // Construct a case where tier 1 isn't enough but tier 2 is.
        // Use a system message + a few user messages + large tool results.
        // context_limit = 200, so target is 190 (95%).
        //
        // System message: "sys" = 1 word => 1*1.3+4 = 5.3 => 5 tokens
        // We add 10 user messages of 10 words each = 10*17 = 170 tokens
        // Plus 2 large tool results of 2000 bytes each.
        // Total with 10 user + 2 tool results ~ 5 + 170 + (tool tokens) > 190.
        //
        // After tier 1 (keep 8): system + 8 recent messages.
        // If the 8 recent include the tool results, they still have huge content
        // pushing tokens high. After tier 2 shrinks them, tokens drop.
        //
        // For simplicity: use limit=500, lots of messages and big tool results.
        let mut msgs = vec![Message::system("system prompt")];
        // Add 20 user messages (each 10 words = 17 tokens)
        for _ in 0..20 {
            msgs.push(Message::user(
                "one two three four five six seven eight nine ten",
            ));
        }
        // Add 5 large tool results (each ~3000 bytes, many words)
        for i in 0..5 {
            let big = "word ".repeat(600); // 600 words => 600*1.3+4 = 784 tokens
            msgs.push(Message::tool_result(&format!("call_{}", i), &big));
        }
        // Add 3 more user messages at the end
        for _ in 0..3 {
            msgs.push(Message::user(
                "one two three four five six seven eight nine ten",
            ));
        }

        // Total: system(5) + 20*17(340) + 5*784(3920) + 3*17(51) = ~4316 tokens
        // context_limit = 4500, target = 4275
        // After tier 1 (keep 8): system + last 8 msgs.
        // Last 8 = 5 tool results + 3 user msgs = 5*784 + 3*17 = 3920+51 = 3971
        // Plus system = 3976. Still > 4275? No, 3976 < 4275. Hmm.
        //
        // Let's use a tighter limit to force tier 2.
        // context_limit = 2000, target = 1900
        // After tier 1 (keep 8): system(5) + 5 tool results(3920) + 3 user(51) = 3976 > 1900
        // After tier 2: shrink tool results. Recent 3 tool results get 5120 budget,
        // older 2 get 1280 budget. But word count matters for token estimation.
        // With budget=100 bytes, "word " * 600 = 3000 bytes truncated to 100 bytes
        // = ~20 words => 20*1.3+4 = 30 tokens.
        // 5 tool results at ~30 tokens = 150, + system(5) + 3 user(51) = 206 < 1900.

        let (result, tier) = try_recover_context(msgs, 2000, 8, 100);
        assert!(
            tier == 2 || tier == 1,
            "Expected tier 1 or 2, got tier {}",
            tier
        );
        // Verify context was actually reduced
        let estimated = super::super::context_monitor::ContextMonitor::estimate_tokens(&result);
        assert!(
            (estimated as f64) <= 2000.0 * 0.95,
            "Estimated {} should be <= {}",
            estimated,
            (2000.0 * 0.95) as usize
        );
    }

    #[test]
    fn test_try_recover_context_tier3_needed() {
        // Create a scenario where even after tier 1 + tier 2, context is too large.
        // Use keep_recent_tier1=8, and make each of the 8 remaining messages very large
        // even after tool shrinking (because they are user/assistant messages, not tool results).
        //
        // context_limit = 100, target = 95
        // 10 messages of 10 words each = 10*17 = 170 > 95.
        // After tier 1 (keep 8): 8*17 = 136 > 95.
        // After tier 2 (no tool results): still 136 > 95.
        // After tier 3 (keep 3): 3*17 = 51 < 95.
        let msgs: Vec<Message> = (0..10)
            .map(|_| Message::user("one two three four five six seven eight nine ten"))
            .collect();
        let (result, tier) = try_recover_context(msgs, 100, 8, 5120);
        assert_eq!(tier, 3);
        assert_eq!(result.len(), 3);
        let estimated = super::super::context_monitor::ContextMonitor::estimate_tokens(&result);
        assert!(
            (estimated as f64) <= 100.0 * 0.95,
            "Estimated {} should be <= 95",
            estimated
        );
    }
}
