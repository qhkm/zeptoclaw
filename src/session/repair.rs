//! Session message history validation and auto-repair utilities.

use std::collections::HashSet;

use crate::session::{Message, Role};

/// Summary of applied repairs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepairStats {
    pub orphan_tool_results_removed: usize,
    pub empty_messages_removed: usize,
    pub role_alternation_fixes: usize,
    pub duplicate_messages_removed: usize,
    pub truncation_repairs: usize,
}

impl RepairStats {
    pub fn total_repairs(&self) -> usize {
        self.orphan_tool_results_removed
            + self.empty_messages_removed
            + self.role_alternation_fixes
            + self.duplicate_messages_removed
            + self.truncation_repairs
    }
}

/// Validate and repair a session message list.
pub fn repair_messages(messages: Vec<Message>) -> (Vec<Message>, RepairStats) {
    let (messages, mut stats) = remove_orphan_tool_results(messages);
    let (messages, empty_removed) = remove_empty_messages(messages);
    stats.empty_messages_removed += empty_removed;
    let (messages, alt_fixed) = fix_role_alternation(messages);
    stats.role_alternation_fixes += alt_fixed;
    let (messages, dup_removed) = remove_consecutive_duplicates(messages);
    stats.duplicate_messages_removed += dup_removed;
    let (messages, trunc_repaired) = repair_truncation_artifacts(messages);
    stats.truncation_repairs += trunc_repaired;
    (messages, stats)
}

fn remove_orphan_tool_results(messages: Vec<Message>) -> (Vec<Message>, RepairStats) {
    let mut seen_tool_calls = HashSet::new();
    let mut repaired = Vec::with_capacity(messages.len());
    let mut stats = RepairStats::default();

    for msg in messages {
        if let Some(tool_calls) = msg.tool_calls.as_ref() {
            for tool_call in tool_calls {
                seen_tool_calls.insert(tool_call.id.clone());
            }
        }

        if msg.role == Role::Tool {
            match msg.tool_call_id.as_ref() {
                Some(id) if seen_tool_calls.contains(id) => repaired.push(msg),
                _ => {
                    stats.orphan_tool_results_removed += 1;
                }
            }
        } else {
            repaired.push(msg);
        }
    }

    (repaired, stats)
}

fn remove_empty_messages(messages: Vec<Message>) -> (Vec<Message>, usize) {
    let mut removed = 0;
    let repaired = messages
        .into_iter()
        .filter(|msg| {
            let has_tools = msg
                .tool_calls
                .as_ref()
                .map(|calls| !calls.is_empty())
                .unwrap_or(false);
            if !msg.content.trim().is_empty() || has_tools || msg.role == Role::Tool {
                true
            } else {
                removed += 1;
                false
            }
        })
        .collect();
    (repaired, removed)
}

fn fix_role_alternation(messages: Vec<Message>) -> (Vec<Message>, usize) {
    let mut fixed = 0;
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());

    for mut msg in messages {
        match msg.role {
            Role::User | Role::Assistant => {
                // Only merge truly consecutive same-role dialog messages.
                if let Some(prev) = out.last_mut() {
                    if prev.role == msg.role {
                        if !msg.content.is_empty() {
                            if !prev.content.is_empty() {
                                prev.content.push('\n');
                            }
                            prev.content.push_str(&msg.content);
                        }
                        // Preserve tool_calls when merging assistant messages
                        if msg.role == Role::Assistant {
                            if let Some(mut calls) = msg.tool_calls.take() {
                                prev.tool_calls
                                    .get_or_insert_with(Vec::new)
                                    .append(&mut calls);
                            }
                        }
                        fixed += 1;
                        continue;
                    }
                }
                out.push(msg);
            }
            Role::System | Role::Tool => out.push(msg),
        }
    }

    (out, fixed)
}

fn remove_consecutive_duplicates(messages: Vec<Message>) -> (Vec<Message>, usize) {
    let mut removed = 0;
    let mut out = Vec::with_capacity(messages.len());

    for msg in messages {
        let duplicate = out.last().map(|prev: &Message| {
            prev.role == msg.role
                && prev.content == msg.content
                && prev.tool_call_id == msg.tool_call_id
                && prev.tool_calls == msg.tool_calls
        });
        if duplicate.unwrap_or(false) {
            removed += 1;
            continue;
        }
        out.push(msg);
    }

    (out, removed)
}

fn repair_truncation_artifacts(messages: Vec<Message>) -> (Vec<Message>, usize) {
    let mut repaired_count = 0;
    let repaired = messages
        .into_iter()
        .map(|mut msg| {
            let mut changed = false;
            if msg.content.ends_with('\u{fffd}') {
                msg.content = msg.content.trim_end_matches('\u{fffd}').to_string();
                changed = true;
            }
            if msg.content.ends_with("[...truncated") {
                msg.content.push_str("...]");
                changed = true;
            }
            if changed {
                repaired_count += 1;
            }
            msg
        })
        .collect();
    (repaired, repaired_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ToolCall;

    #[test]
    fn removes_orphan_tool_results() {
        let messages = vec![
            Message::assistant("call tool"),
            Message::tool_result("missing", "orphan"),
        ];
        let (repaired, stats) = repair_messages(messages);
        assert_eq!(repaired.len(), 1);
        assert_eq!(stats.orphan_tool_results_removed, 1);
    }

    #[test]
    fn keeps_matched_tool_results() {
        let messages = vec![
            Message::assistant_with_tools("call", vec![ToolCall::new("c1", "echo", "{}")]),
            Message::tool_result("c1", "ok"),
        ];
        let (repaired, stats) = repair_messages(messages);
        assert_eq!(repaired.len(), 2);
        assert_eq!(stats.orphan_tool_results_removed, 0);
    }

    #[test]
    fn removes_empty_and_duplicate_messages() {
        let messages = vec![
            Message::user(""),
            Message::assistant("hello"),
            Message::assistant("hello"),
        ];
        let (repaired, stats) = repair_messages(messages);
        assert_eq!(repaired.len(), 1);
        assert_eq!(stats.empty_messages_removed, 1);
        assert!(stats.duplicate_messages_removed > 0 || stats.role_alternation_fixes > 0);
    }

    #[test]
    fn fixes_dialog_role_alternation() {
        let messages = vec![
            Message::user("a"),
            Message::user("b"),
            Message::assistant("c"),
            Message::assistant("d"),
        ];
        let (repaired, stats) = repair_messages(messages);
        assert_eq!(repaired.len(), 2);
        assert_eq!(stats.role_alternation_fixes, 2);
        // Verify content was merged, not dropped
        assert_eq!(repaired[0].content, "a\nb");
        assert_eq!(repaired[1].content, "c\nd");
    }
}
