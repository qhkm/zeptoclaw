//! Conversation history command handler.

use anyhow::{Context, Result};

use zeptoclaw::session::{ConversationHistory, Role, SessionManager};

use super::HistoryAction;

/// Manage CLI conversation history.
pub(crate) async fn cmd_history(action: HistoryAction) -> Result<()> {
    let history =
        ConversationHistory::new().with_context(|| "Failed to initialize history store")?;

    match action {
        HistoryAction::List { limit } => {
            let entries = history.list_conversations()?;
            if entries.is_empty() {
                println!("No CLI conversation history found.");
                return Ok(());
            }

            let shown = entries.len().min(limit);
            println!("Showing {} of {} conversation(s):", shown, entries.len());
            for entry in entries.iter().take(limit) {
                println!(
                    "- {} | {} msgs | {} | {}",
                    entry.session_key, entry.message_count, entry.last_updated, entry.title
                );
            }
        }
        HistoryAction::Show { query } => {
            let Some(entry) = history.find_conversation(&query)? else {
                anyhow::bail!("No conversation found for query '{}'", query);
            };

            let manager = SessionManager::new().with_context(|| "Failed to open session store")?;
            let Some(session) = manager.get(&entry.session_key).await? else {
                anyhow::bail!(
                    "Conversation '{}' exists in index but could not be loaded",
                    entry.session_key
                );
            };

            println!("Session: {}", session.key);
            println!("Updated: {}", session.updated_at.to_rfc3339());
            println!("Messages: {}", session.messages.len());
            if let Some(summary) = &session.summary {
                println!("Summary: {}", summary);
            }
            println!();

            for message in session.messages {
                println!("[{}]", role_label(&message.role));
                println!("{}", message.content);
                println!();
            }
        }
        HistoryAction::Cleanup { keep } => {
            let deleted = history.cleanup_old(keep)?;
            println!(
                "Cleanup complete: deleted {} old conversation(s), kept {} most recent.",
                deleted, keep
            );
        }
    }

    Ok(())
}

fn role_label(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}
