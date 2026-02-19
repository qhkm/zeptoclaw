//! CLI conversation history management for ZeptoClaw.
//!
//! This module provides discovery, listing, and cleanup of CLI conversation
//! sessions stored on disk. It operates as a read-only layer on top of the
//! existing `SessionManager` persistence format, filtering only sessions
//! whose key starts with `"cli:"`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Result, ZeptoError};
use crate::session::{Message, Role, Session};

/// Metadata for a saved CLI conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    /// Unique session key (e.g., "cli:1739523000")
    pub session_key: String,
    /// Human-readable title (first user message, truncated to 80 chars)
    pub title: String,
    /// Number of messages in the conversation
    pub message_count: usize,
    /// When the conversation was last updated (ISO 8601)
    pub last_updated: String,
    /// File size in bytes
    pub file_size: u64,
}

/// Manages CLI conversation history on disk.
///
/// Scans the session storage directory for files that correspond to CLI
/// sessions (keys starting with `"cli:"`), reads their metadata, and
/// provides listing, search, and cleanup operations.
pub struct ConversationHistory {
    storage_path: PathBuf,
}

impl ConversationHistory {
    /// Create a new `ConversationHistory` using the default sessions directory.
    ///
    /// The default path is `~/.zeptoclaw/sessions/`.
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be created.
    pub fn new() -> Result<Self> {
        let storage_path = Config::dir().join("sessions");
        std::fs::create_dir_all(&storage_path)?;
        Ok(Self { storage_path })
    }

    /// Create a new `ConversationHistory` with a custom storage path.
    ///
    /// Useful for testing with temporary directories.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created.
    pub fn with_path(path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&path)?;
        Ok(Self { storage_path: path })
    }

    /// List all CLI conversations, sorted by `last_updated` descending (newest first).
    ///
    /// Scans the session directory for `cli%3A*.json` files, reads each one to
    /// extract metadata, and returns a sorted list of conversation entries.
    ///
    /// Non-CLI sessions (e.g., telegram, slack) are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error if reading the directory or any session file fails.
    pub fn list_conversations(&self) -> Result<Vec<ConversationEntry>> {
        let mut entries = Vec::new();

        let dir_entries = std::fs::read_dir(&self.storage_path)?;
        for entry in dir_entries {
            let entry = entry?;
            let path = entry.path();

            // Only look at .json files whose sanitized name starts with cli%3A
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            if !file_name.ends_with(".json") || !file_name.starts_with("cli%3A") {
                continue;
            }

            // Read and parse the session
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let session: Session = match serde_json::from_str(&content) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Only include sessions with CLI keys
            if !session.key.starts_with("cli:") {
                continue;
            }

            let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            entries.push(ConversationEntry {
                session_key: session.key.clone(),
                title: Self::extract_title(&session.messages),
                message_count: session.messages.len(),
                last_updated: session.updated_at.to_rfc3339(),
                file_size,
            });
        }

        // Sort by last_updated descending (newest first)
        entries.sort_by(|a, b| b.last_updated.cmp(&a.last_updated));

        Ok(entries)
    }

    /// Return the most recently updated CLI conversation, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if listing conversations fails.
    pub fn latest_conversation(&self) -> Result<Option<ConversationEntry>> {
        let conversations = self.list_conversations()?;
        Ok(conversations.into_iter().next())
    }

    /// Find a CLI conversation by title substring (case-insensitive) or exact session key.
    ///
    /// First attempts an exact match on session key, then falls back to a
    /// case-insensitive substring match on the title. Returns the first match.
    ///
    /// # Errors
    ///
    /// Returns an error if listing conversations fails.
    pub fn find_conversation(&self, query: &str) -> Result<Option<ConversationEntry>> {
        let conversations = self.list_conversations()?;

        // Try exact session key match first
        if let Some(entry) = conversations.iter().find(|e| e.session_key == query) {
            return Ok(Some(entry.clone()));
        }

        // Fall back to case-insensitive title substring match
        let query_lower = query.to_lowercase();
        Ok(conversations
            .into_iter()
            .find(|e| e.title.to_lowercase().contains(&query_lower)))
    }

    /// Generate a unique CLI session key using the current unix timestamp.
    ///
    /// Format: `cli:<unix_epoch_seconds>`
    pub fn generate_session_key() -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("cli:{}", timestamp)
    }

    /// Extract a title from a list of messages.
    ///
    /// Finds the first user message and truncates its content to 80 characters.
    /// Returns `"(empty conversation)"` if there are no messages, or
    /// `"(no user messages)"` if no user messages are found.
    pub fn extract_title(messages: &[Message]) -> String {
        if messages.is_empty() {
            return "(empty conversation)".to_string();
        }

        match messages.iter().find(|m| m.role == Role::User) {
            Some(msg) => {
                let content = msg.content.trim();
                if content.len() <= 80 {
                    content.to_string()
                } else {
                    let mut truncated: String = content.chars().take(80).collect();
                    truncated.push_str("...");
                    truncated
                }
            }
            None => "(no user messages)".to_string(),
        }
    }

    /// Delete the oldest CLI conversations, keeping only the most recent `keep_count`.
    ///
    /// Conversations are sorted by `last_updated` descending, so the newest
    /// `keep_count` are preserved and the rest are deleted from disk.
    ///
    /// # Returns
    ///
    /// The number of conversations deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if listing or deleting session files fails.
    pub fn cleanup_old(&self, keep_count: usize) -> Result<usize> {
        let conversations = self.list_conversations()?;

        if conversations.len() <= keep_count {
            return Ok(0);
        }

        let to_delete = &conversations[keep_count..];
        let mut deleted = 0;

        for entry in to_delete {
            let sanitized = Self::sanitize_key(&entry.session_key);
            let file_path = self.storage_path.join(format!("{}.json", sanitized));
            if file_path.exists() {
                std::fs::remove_file(&file_path).map_err(|e| {
                    ZeptoError::Session(format!(
                        "Failed to delete session file {}: {}",
                        file_path.display(),
                        e
                    ))
                })?;
                deleted += 1;
            }
        }

        Ok(deleted)
    }

    /// Sanitize a session key for use as a filename (matches `SessionManager::sanitize_key`).
    fn sanitize_key(key: &str) -> String {
        let mut result = String::with_capacity(key.len() * 3);
        for c in key.chars() {
            match c {
                '/' => result.push_str("%2F"),
                '\\' => result.push_str("%5C"),
                ':' => result.push_str("%3A"),
                '*' => result.push_str("%2A"),
                '?' => result.push_str("%3F"),
                '"' => result.push_str("%22"),
                '<' => result.push_str("%3C"),
                '>' => result.push_str("%3E"),
                '|' => result.push_str("%7C"),
                '%' => result.push_str("%25"),
                c => result.push(c),
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    /// Helper to write a test session JSON file directly to disk.
    fn write_test_session(dir: &Path, key: &str, user_msg: &str, updated_at: &str) {
        // Build the session JSON manually to control the updated_at timestamp.
        // The real Session uses chrono DateTime, so we produce compatible JSON.
        let session_json = serde_json::json!({
            "key": key,
            "messages": [{
                "role": "user",
                "content": user_msg
            }],
            "summary": null,
            "created_at": updated_at,
            "updated_at": updated_at
        });
        let sanitized = key.replace(':', "%3A");
        let path = dir.join(format!("{}.json", sanitized));
        std::fs::write(path, serde_json::to_string(&session_json).unwrap()).unwrap();
    }

    #[test]
    fn test_conversation_entry_creation() {
        let entry = ConversationEntry {
            session_key: "cli:1700000000".to_string(),
            title: "Hello world".to_string(),
            message_count: 3,
            last_updated: "2025-11-14T22:13:20+00:00".to_string(),
            file_size: 256,
        };

        assert_eq!(entry.session_key, "cli:1700000000");
        assert_eq!(entry.title, "Hello world");
        assert_eq!(entry.message_count, 3);
        assert_eq!(entry.last_updated, "2025-11-14T22:13:20+00:00");
        assert_eq!(entry.file_size, 256);
    }

    #[test]
    fn test_generate_session_key() {
        let key = ConversationHistory::generate_session_key();
        assert!(
            key.starts_with("cli:"),
            "Key should start with 'cli:', got: {}",
            key
        );

        // The remainder should be parseable as a u64 timestamp
        let timestamp_part = &key[4..];
        let parsed: u64 = timestamp_part
            .parse()
            .expect("Timestamp part should be a valid u64");
        assert!(parsed > 0, "Timestamp should be positive");
    }

    #[test]
    fn test_extract_title_from_messages() {
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Tell me about Rust programming language and its memory safety features"),
        ];

        let title = ConversationHistory::extract_title(&messages);
        assert_eq!(
            title,
            "Tell me about Rust programming language and its memory safety features"
        );

        // Test truncation at 80 chars
        let long_msg = "a".repeat(120);
        let messages = vec![Message::user(&long_msg)];
        let title = ConversationHistory::extract_title(&messages);
        assert!(
            title.len() <= 83,
            "Title should be at most 80 chars + '...'"
        );
        assert!(title.ends_with("..."), "Long title should end with '...'");
        // The first 80 chars should be preserved
        let title_prefix: String = title.chars().take(80).collect();
        let long_msg_prefix: String = long_msg.chars().take(80).collect();
        assert_eq!(title_prefix, long_msg_prefix);
    }

    #[test]
    fn test_extract_title_empty_messages() {
        let messages: Vec<Message> = vec![];
        let title = ConversationHistory::extract_title(&messages);
        assert_eq!(title, "(empty conversation)");
    }

    #[test]
    fn test_extract_title_no_user_messages() {
        let messages = vec![
            Message::system("System prompt"),
            Message::assistant("Hello there"),
        ];
        let title = ConversationHistory::extract_title(&messages);
        assert_eq!(title, "(no user messages)");
    }

    #[test]
    fn test_list_conversations_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let history = ConversationHistory::with_path(temp_dir.path().to_path_buf()).unwrap();

        let conversations = history.list_conversations().unwrap();
        assert!(conversations.is_empty());
    }

    #[test]
    fn test_list_conversations_with_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        write_test_session(
            dir,
            "cli:1000",
            "First conversation",
            "2025-01-01T00:00:00Z",
        );
        write_test_session(
            dir,
            "cli:2000",
            "Second conversation",
            "2025-06-15T12:00:00Z",
        );
        write_test_session(
            dir,
            "cli:3000",
            "Third conversation",
            "2025-03-10T06:30:00Z",
        );

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();
        let conversations = history.list_conversations().unwrap();

        assert_eq!(conversations.len(), 3);

        // Should be sorted by last_updated descending (newest first)
        assert_eq!(conversations[0].session_key, "cli:2000");
        assert_eq!(conversations[1].session_key, "cli:3000");
        assert_eq!(conversations[2].session_key, "cli:1000");

        // Verify titles
        assert_eq!(conversations[0].title, "Second conversation");
        assert_eq!(conversations[1].title, "Third conversation");
        assert_eq!(conversations[2].title, "First conversation");

        // Verify message counts
        for conv in &conversations {
            assert_eq!(conv.message_count, 1);
        }
    }

    #[test]
    fn test_list_conversations_ignores_non_cli() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        // Write a CLI session
        write_test_session(dir, "cli:1000", "CLI session", "2025-01-01T00:00:00Z");

        // Write a telegram session (should be ignored)
        write_test_session(
            dir,
            "telegram:chat123",
            "Telegram session",
            "2025-01-02T00:00:00Z",
        );

        // Write a slack session (should be ignored)
        write_test_session(
            dir,
            "slack:channel456",
            "Slack session",
            "2025-01-03T00:00:00Z",
        );

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();
        let conversations = history.list_conversations().unwrap();

        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].session_key, "cli:1000");
        assert_eq!(conversations[0].title, "CLI session");
    }

    #[test]
    fn test_latest_conversation() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        write_test_session(dir, "cli:1000", "Old conversation", "2025-01-01T00:00:00Z");
        write_test_session(
            dir,
            "cli:2000",
            "Newest conversation",
            "2025-12-31T23:59:59Z",
        );
        write_test_session(
            dir,
            "cli:3000",
            "Middle conversation",
            "2025-06-15T12:00:00Z",
        );

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();
        let latest = history.latest_conversation().unwrap();

        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.session_key, "cli:2000");
        assert_eq!(latest.title, "Newest conversation");
    }

    #[test]
    fn test_latest_conversation_empty() {
        let temp_dir = TempDir::new().unwrap();
        let history = ConversationHistory::with_path(temp_dir.path().to_path_buf()).unwrap();

        let latest = history.latest_conversation().unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn test_find_conversation_by_title() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        write_test_session(
            dir,
            "cli:1000",
            "Discuss Rust memory safety",
            "2025-01-01T00:00:00Z",
        );
        write_test_session(
            dir,
            "cli:2000",
            "Python web framework comparison",
            "2025-06-15T12:00:00Z",
        );

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();

        // Case-insensitive substring match
        let found = history.find_conversation("rust memory").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().session_key, "cli:1000");

        // Case-insensitive match with different casing
        let found = history.find_conversation("PYTHON WEB").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().session_key, "cli:2000");

        // No match
        let found = history.find_conversation("nonexistent topic").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_find_conversation_by_key() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        write_test_session(dir, "cli:1000", "First session", "2025-01-01T00:00:00Z");
        write_test_session(dir, "cli:2000", "Second session", "2025-06-15T12:00:00Z");

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();

        // Exact session key match
        let found = history.find_conversation("cli:1000").unwrap();
        assert!(found.is_some());
        let entry = found.unwrap();
        assert_eq!(entry.session_key, "cli:1000");
        assert_eq!(entry.title, "First session");
    }

    #[test]
    fn test_cleanup_old() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        // Create 5 CLI sessions with distinct timestamps
        write_test_session(dir, "cli:1000", "Session one", "2025-01-01T00:00:00Z");
        write_test_session(dir, "cli:2000", "Session two", "2025-02-01T00:00:00Z");
        write_test_session(dir, "cli:3000", "Session three", "2025-03-01T00:00:00Z");
        write_test_session(dir, "cli:4000", "Session four", "2025-04-01T00:00:00Z");
        write_test_session(dir, "cli:5000", "Session five", "2025-05-01T00:00:00Z");

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();

        // Verify we have 5 conversations
        assert_eq!(history.list_conversations().unwrap().len(), 5);

        // Cleanup, keeping only the 2 newest
        let deleted = history.cleanup_old(2).unwrap();
        assert_eq!(deleted, 3);

        // Verify only 2 remain (the newest ones)
        let remaining = history.list_conversations().unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].session_key, "cli:5000");
        assert_eq!(remaining[1].session_key, "cli:4000");

        // The older 3 files should be gone
        assert!(!dir.join("cli%3A1000.json").exists());
        assert!(!dir.join("cli%3A2000.json").exists());
        assert!(!dir.join("cli%3A3000.json").exists());

        // The newer 2 files should still exist
        assert!(dir.join("cli%3A4000.json").exists());
        assert!(dir.join("cli%3A5000.json").exists());
    }

    #[test]
    fn test_cleanup_old_nothing_to_delete() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        write_test_session(dir, "cli:1000", "Session one", "2025-01-01T00:00:00Z");
        write_test_session(dir, "cli:2000", "Session two", "2025-02-01T00:00:00Z");

        let history = ConversationHistory::with_path(dir.to_path_buf()).unwrap();

        // keep_count >= total conversations, nothing deleted
        let deleted = history.cleanup_old(5).unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(history.list_conversations().unwrap().len(), 2);
    }
}
