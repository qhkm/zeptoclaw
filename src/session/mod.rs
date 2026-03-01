//! Session module - Session and conversation state management
//!
//! This module provides session management for ZeptoClaw, including:
//! - In-memory session storage with async access
//! - File-based persistence for sessions
//! - Session creation, retrieval, and deletion
//!
//! # Example
//!
//! ```
//! use zeptoclaw::session::{SessionManager, Message};
//!
//! #[tokio::main]
//! async fn main() {
//!     let manager = SessionManager::new_memory();
//!
//!     // Get or create a session
//!     let mut session = manager.get_or_create("telegram:chat123").await.unwrap();
//!
//!     // Add messages
//!     session.add_message(Message::user("Hello!"));
//!     session.add_message(Message::assistant("Hi there!"));
//!
//!     // Save the session
//!     manager.save(&session).await.unwrap();
//! }
//! ```

pub mod history;
pub mod media;
pub mod repair;
pub mod types;

pub use history::ConversationHistory;
pub use repair::{repair_messages, RepairStats};
pub use types::{Message, Role, Session, ToolCall};

use crate::config::Config;
use crate::error::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

/// Session manager for storing and retrieving conversation sessions.
///
/// The `SessionManager` provides both in-memory caching and optional
/// file-based persistence for sessions. Sessions are identified by
/// unique keys (e.g., "telegram:chat123").
///
/// # Thread Safety
///
/// The manager uses `Arc<RwLock>` internally, making it safe to clone
/// and share across async tasks.
///
/// # Persistence
///
/// When created with `new()`, sessions are persisted to disk in the
/// `~/.zeptoclaw/sessions/` directory. Use `new_memory()` for testing
/// or when persistence is not needed.
pub struct SessionManager {
    /// In-memory cache of sessions
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    /// Optional path for file-based persistence
    storage_path: Option<PathBuf>,
}

impl SessionManager {
    /// Create a new session manager with file-based persistence.
    ///
    /// Sessions are stored in `~/.zeptoclaw/sessions/` as JSON files.
    /// The directory is created if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be created.
    ///
    /// # Example
    /// ```no_run
    /// use zeptoclaw::session::SessionManager;
    ///
    /// let manager = SessionManager::new().unwrap();
    /// ```
    pub fn new() -> Result<Self> {
        let storage_path = Config::dir().join("sessions");
        std::fs::create_dir_all(&storage_path)?;
        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage_path: Some(storage_path),
        })
    }

    /// Create an in-memory session manager without persistence.
    ///
    /// This is useful for testing or temporary sessions that don't
    /// need to survive application restarts.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::SessionManager;
    ///
    /// let manager = SessionManager::new_memory();
    /// ```
    pub fn new_memory() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage_path: None,
        }
    }

    /// Create a session manager with a custom storage path.
    ///
    /// # Arguments
    /// * `path` - Directory path for session storage
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created.
    ///
    /// # Example
    /// ```no_run
    /// use zeptoclaw::session::SessionManager;
    /// use std::path::PathBuf;
    ///
    /// let manager = SessionManager::with_path(PathBuf::from("/tmp/sessions")).unwrap();
    /// ```
    pub fn with_path(path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&path)?;
        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage_path: Some(path),
        })
    }

    /// Get an existing session or create a new one.
    ///
    /// If the session exists in memory, it is returned immediately.
    /// If persistence is enabled and the session exists on disk, it
    /// is loaded into memory. Otherwise, a new empty session is created.
    ///
    /// # Arguments
    /// * `key` - Unique session identifier
    ///
    /// # Errors
    ///
    /// Returns an error if loading from disk fails.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::SessionManager;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let manager = SessionManager::new_memory();
    ///     let session = manager.get_or_create("test-session").await.unwrap();
    ///     assert_eq!(session.key, "test-session");
    /// }
    /// ```
    pub async fn get_or_create(&self, key: &str) -> Result<Session> {
        // Check in-memory cache first
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(key) {
                return Ok(session.clone());
            }
        }

        // Try loading from disk if persistence is enabled
        if let Some(ref storage_path) = self.storage_path {
            let file_path = storage_path.join(format!("{}.json", Self::sanitize_key(key)));
            if file_path.exists() {
                let content = tokio::fs::read_to_string(&file_path).await?;
                let mut session: Session = serde_json::from_str(&content)?;
                self.maybe_repair_loaded_session(&mut session, "get_or_create");

                // Cache it in memory
                let mut sessions = self.sessions.write().await;
                sessions.insert(key.to_string(), session.clone());
                return Ok(session);
            }
        }

        // Create new session
        let session = Session::new(key);
        let mut sessions = self.sessions.write().await;
        sessions.insert(key.to_string(), session.clone());
        Ok(session)
    }

    /// Get a session by key without creating it.
    ///
    /// # Arguments
    /// * `key` - Unique session identifier
    ///
    /// # Returns
    ///
    /// `Some(Session)` if found, `None` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if loading from disk fails.
    pub async fn get(&self, key: &str) -> Result<Option<Session>> {
        // Check in-memory cache first
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(key) {
                return Ok(Some(session.clone()));
            }
        }

        // Try loading from disk if persistence is enabled
        if let Some(ref storage_path) = self.storage_path {
            let file_path = storage_path.join(format!("{}.json", Self::sanitize_key(key)));
            if file_path.exists() {
                let content = tokio::fs::read_to_string(&file_path).await?;
                let mut session: Session = serde_json::from_str(&content)?;
                self.maybe_repair_loaded_session(&mut session, "get");

                // Cache it in memory
                let mut sessions = self.sessions.write().await;
                sessions.insert(key.to_string(), session.clone());
                return Ok(Some(session));
            }
        }

        Ok(None)
    }

    /// Save a session to both memory and disk (if persistence is enabled).
    ///
    /// # Arguments
    /// * `session` - The session to save
    ///
    /// # Errors
    ///
    /// Returns an error if writing to disk fails.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{SessionManager, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let manager = SessionManager::new_memory();
    ///     let mut session = manager.get_or_create("test").await.unwrap();
    ///     session.add_message(Message::user("Hello"));
    ///     manager.save(&session).await.unwrap();
    /// }
    /// ```
    pub async fn save(&self, session: &Session) -> Result<()> {
        // Update in-memory cache
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.key.clone(), session.clone());
        }

        // Write to disk if persistence is enabled
        if let Some(ref storage_path) = self.storage_path {
            let file_path = storage_path.join(format!("{}.json", Self::sanitize_key(&session.key)));
            let content = serde_json::to_string_pretty(session)?;
            tokio::fs::write(&file_path, content).await?;
        }

        Ok(())
    }

    /// Delete a session from both memory and disk.
    ///
    /// # Arguments
    /// * `key` - Unique session identifier
    ///
    /// # Errors
    ///
    /// Returns an error if deleting from disk fails.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::SessionManager;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let manager = SessionManager::new_memory();
    ///     manager.get_or_create("test").await.unwrap();
    ///     manager.delete("test").await.unwrap();
    /// }
    /// ```
    pub async fn delete(&self, key: &str) -> Result<()> {
        // Remove from memory
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(key);
        }

        // Remove from disk if persistence is enabled
        if let Some(ref storage_path) = self.storage_path {
            let file_path = storage_path.join(format!("{}.json", Self::sanitize_key(key)));
            if file_path.exists() {
                tokio::fs::remove_file(&file_path).await?;
            }
        }

        Ok(())
    }

    /// List all session keys.
    ///
    /// Returns session keys from both memory and disk (if persistence is enabled).
    /// Duplicate keys are not included.
    ///
    /// # Errors
    ///
    /// Returns an error if reading the storage directory fails.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::SessionManager;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let manager = SessionManager::new_memory();
    ///     manager.get_or_create("session1").await.unwrap();
    ///     manager.get_or_create("session2").await.unwrap();
    ///
    ///     let keys = manager.list().await.unwrap();
    ///     assert_eq!(keys.len(), 2);
    /// }
    /// ```
    pub async fn list(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();

        // Get keys from memory
        {
            let sessions = self.sessions.read().await;
            keys.extend(sessions.keys().cloned());
        }

        // Get keys from disk if persistence is enabled
        // We read each session file to get the actual key (not the sanitized filename)
        if let Some(ref storage_path) = self.storage_path {
            let mut dir_entries = tokio::fs::read_dir(storage_path).await?;
            while let Some(entry) = dir_entries.next_entry().await? {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    // Read the session file to get the actual key
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        if let Ok(session) = serde_json::from_str::<Session>(&content) {
                            if !keys.contains(&session.key) {
                                keys.push(session.key);
                            }
                        }
                    }
                }
            }
        }

        keys.sort();
        Ok(keys)
    }

    /// Check if a session exists.
    ///
    /// # Arguments
    /// * `key` - Unique session identifier
    ///
    /// # Returns
    ///
    /// `true` if the session exists in memory or on disk.
    pub async fn exists(&self, key: &str) -> bool {
        // Check memory
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(key) {
                return true;
            }
        }

        // Check disk
        if let Some(ref storage_path) = self.storage_path {
            let file_path = storage_path.join(format!("{}.json", Self::sanitize_key(key)));
            return file_path.exists();
        }

        false
    }

    /// Clear all sessions from memory (does not affect disk).
    ///
    /// Use this to free memory while keeping persisted sessions.
    pub async fn clear_cache(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.clear();
    }

    /// Get the number of sessions in memory.
    pub async fn cache_size(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Sanitize a session key for use as a filename.
    ///
    /// Uses percent-encoding to ensure the mapping is bijective (one-to-one).
    /// This prevents collisions where different keys would map to the same filename.
    ///
    /// For example:
    /// - "telegram:chat123" → "telegram%3Achat123"
    /// - "discord/server" → "discord%2Fserver"
    ///
    /// This is reversible via `unsanitize_key`, ensuring keys round-trip correctly.
    fn sanitize_key(key: &str) -> String {
        // Characters that are problematic in filenames across platforms
        // We percent-encode them to make the mapping reversible
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
                '%' => result.push_str("%25"), // Escape % itself to make it reversible
                c => result.push(c),
            }
        }
        result
    }

    /// Reverse the sanitization to recover the original key.
    ///
    /// This is the inverse of `sanitize_key`.
    #[allow(dead_code)]
    fn unsanitize_key(sanitized: &str) -> String {
        let mut result = String::with_capacity(sanitized.len());
        let mut chars = sanitized.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '%' {
                // Try to read two hex digits
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() == 2 {
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte as char);
                        continue;
                    }
                }
                // If parsing failed, just keep the % and the hex chars
                result.push('%');
                result.push_str(&hex);
            } else {
                result.push(c);
            }
        }
        result
    }

    fn maybe_repair_loaded_session(&self, session: &mut Session, source: &str) {
        if !Config::get().session.auto_repair {
            return;
        }
        let (repaired, stats) =
            crate::session::repair::repair_messages(std::mem::take(&mut session.messages));
        if stats.total_repairs() > 0 {
            warn!(
                session_key = %session.key,
                source = source,
                orphan_tool_results_removed = stats.orphan_tool_results_removed,
                empty_messages_removed = stats.empty_messages_removed,
                role_alternation_fixes = stats.role_alternation_fixes,
                duplicate_messages_removed = stats.duplicate_messages_removed,
                truncation_repairs = stats.truncation_repairs,
                "Session history repaired on load"
            );
        }
        session.messages = repaired;
    }
}

impl Clone for SessionManager {
    fn clone(&self) -> Self {
        Self {
            sessions: Arc::clone(&self.sessions),
            storage_path: self.storage_path.clone(),
        }
    }
}

impl Default for SessionManager {
    /// Creates an in-memory session manager.
    ///
    /// Use `SessionManager::new()` for file-based persistence.
    fn default() -> Self {
        Self::new_memory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_session_create_and_retrieve() {
        let manager = SessionManager::new_memory();
        let session = manager.get_or_create("test-session").await.unwrap();
        assert!(session.messages.is_empty());
        assert_eq!(session.key, "test-session");
    }

    #[tokio::test]
    async fn test_session_save_and_load() {
        let manager = SessionManager::new_memory();
        let mut session = manager.get_or_create("test-session").await.unwrap();
        session.add_message(Message::user("Hello"));
        manager.save(&session).await.unwrap();

        let loaded = manager.get_or_create("test-session").await.unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, "Hello");
    }

    #[test]
    fn test_message_creation() {
        let user_msg = Message::user("Hello");
        assert_eq!(user_msg.role, Role::User);
        assert_eq!(user_msg.content, "Hello");

        let assistant_msg = Message::assistant("Hi there");
        assert_eq!(assistant_msg.role, Role::Assistant);
        assert_eq!(assistant_msg.content, "Hi there");

        let system_msg = Message::system("You are helpful");
        assert_eq!(system_msg.role, Role::System);

        let tool_msg = Message::tool_result("call_1", "Success");
        assert_eq!(tool_msg.role, Role::Tool);
        assert_eq!(tool_msg.tool_call_id, Some("call_1".to_string()));
    }

    #[tokio::test]
    async fn test_session_delete() {
        let manager = SessionManager::new_memory();
        manager.get_or_create("test-session").await.unwrap();
        assert!(manager.exists("test-session").await);

        manager.delete("test-session").await.unwrap();
        assert!(!manager.exists("test-session").await);
    }

    #[tokio::test]
    async fn test_session_list() {
        let manager = SessionManager::new_memory();
        manager.get_or_create("session-a").await.unwrap();
        manager.get_or_create("session-b").await.unwrap();
        manager.get_or_create("session-c").await.unwrap();

        let keys = manager.list().await.unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"session-a".to_string()));
        assert!(keys.contains(&"session-b".to_string()));
        assert!(keys.contains(&"session-c".to_string()));
    }

    #[tokio::test]
    async fn test_session_get_nonexistent() {
        let manager = SessionManager::new_memory();
        let result = manager.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_session_manager_clone() {
        let manager1 = SessionManager::new_memory();
        let manager2 = manager1.clone();

        // Create session with manager1
        let mut session = manager1.get_or_create("shared").await.unwrap();
        session.add_message(Message::user("Test"));
        manager1.save(&session).await.unwrap();

        // Should be visible from manager2
        let loaded = manager2.get("shared").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().messages.len(), 1);
    }

    #[tokio::test]
    async fn test_session_clear_cache() {
        let manager = SessionManager::new_memory();
        manager.get_or_create("session1").await.unwrap();
        manager.get_or_create("session2").await.unwrap();

        assert_eq!(manager.cache_size().await, 2);

        manager.clear_cache().await;
        assert_eq!(manager.cache_size().await, 0);
    }

    #[tokio::test]
    async fn test_file_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();

        // Create manager and save a session
        {
            let manager = SessionManager::with_path(storage_path.clone()).unwrap();
            let mut session = manager.get_or_create("persist-test").await.unwrap();
            session.add_message(Message::user("Persisted message"));
            manager.save(&session).await.unwrap();
        }

        // Create new manager instance and load the session
        {
            let manager = SessionManager::with_path(storage_path.clone()).unwrap();
            let session = manager.get_or_create("persist-test").await.unwrap();
            assert_eq!(session.messages.len(), 1);
            assert_eq!(session.messages[0].content, "Persisted message");
        }
    }

    #[tokio::test]
    async fn test_file_persistence_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();

        let manager = SessionManager::with_path(storage_path.clone()).unwrap();

        // Create and save (using a key without special chars to keep filename simple)
        let session = manager.get_or_create("delete-test").await.unwrap();
        manager.save(&session).await.unwrap();

        // Verify file exists (filename matches key since no special chars)
        let file_path = storage_path.join("delete-test.json");
        assert!(file_path.exists(), "Session file should exist after save");

        // Delete
        manager.delete("delete-test").await.unwrap();

        // Verify file is gone
        assert!(!file_path.exists(), "Session file should be deleted");
    }

    #[tokio::test]
    async fn test_file_persistence_list() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();

        let manager = SessionManager::with_path(storage_path).unwrap();

        // Create and save multiple sessions
        for name in ["alpha", "beta", "gamma"] {
            let session = manager.get_or_create(name).await.unwrap();
            manager.save(&session).await.unwrap();
        }

        // Clear cache to force disk reads
        manager.clear_cache().await;

        let keys = manager.list().await.unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"alpha".to_string()));
        assert!(keys.contains(&"beta".to_string()));
        assert!(keys.contains(&"gamma".to_string()));
    }

    #[test]
    fn test_sanitize_key() {
        // Simple keys pass through unchanged
        assert_eq!(SessionManager::sanitize_key("simple"), "simple");
        // Special characters are percent-encoded
        assert_eq!(
            SessionManager::sanitize_key("telegram:chat123"),
            "telegram%3Achat123"
        );
        assert_eq!(
            SessionManager::sanitize_key("path/to/session"),
            "path%2Fto%2Fsession"
        );
        assert_eq!(
            SessionManager::sanitize_key("a:b/c\\d*e?f\"g<h>i|j"),
            "a%3Ab%2Fc%5Cd%2Ae%3Ff%22g%3Ch%3Ei%7Cj"
        );
        // Percent itself is escaped to make encoding reversible
        assert_eq!(SessionManager::sanitize_key("100%done"), "100%25done");
    }

    #[test]
    fn test_unsanitize_key() {
        // Round-trip: sanitize then unsanitize should return original
        let keys = [
            "simple",
            "telegram:chat123",
            "path/to/session",
            "a:b/c\\d*e?f\"g<h>i|j",
            "100%done",
            "multi%percent%%test",
        ];
        for key in &keys {
            let sanitized = SessionManager::sanitize_key(key);
            let unsanitized = SessionManager::unsanitize_key(&sanitized);
            assert_eq!(
                unsanitized, *key,
                "Key '{}' should round-trip through sanitize/unsanitize",
                key
            );
        }
    }

    #[test]
    fn test_sanitize_key_no_collisions() {
        // Keys that would collide with the old underscore-replacement approach
        // should now produce different sanitized values
        let key1 = "a:b";
        let key2 = "a/b";
        let key3 = "a_b"; // This one has an actual underscore

        let sanitized1 = SessionManager::sanitize_key(key1);
        let sanitized2 = SessionManager::sanitize_key(key2);
        let sanitized3 = SessionManager::sanitize_key(key3);

        assert_ne!(sanitized1, sanitized2, "a:b and a/b should not collide");
        assert_ne!(sanitized1, sanitized3, "a:b and a_b should not collide");
        assert_ne!(sanitized2, sanitized3, "a/b and a_b should not collide");

        // Verify the actual values
        assert_eq!(sanitized1, "a%3Ab");
        assert_eq!(sanitized2, "a%2Fb");
        assert_eq!(sanitized3, "a_b");
    }

    #[tokio::test]
    async fn test_list_returns_original_keys_with_special_chars() {
        // Regression test: list() should return original keys, not sanitized filenames
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();

        let manager = SessionManager::with_path(storage_path).unwrap();

        // Create sessions with special characters in keys
        let keys = ["telegram:chat123", "discord/server456", "slack:channel:789"];
        for key in &keys {
            let session = manager.get_or_create(key).await.unwrap();
            manager.save(&session).await.unwrap();
        }

        // Clear cache to force reading from disk
        manager.clear_cache().await;

        // list() should return original keys, not sanitized filenames
        let listed_keys = manager.list().await.unwrap();
        assert_eq!(listed_keys.len(), 3);
        for key in &keys {
            assert!(
                listed_keys.contains(&key.to_string()),
                "list() should contain original key '{}', got {:?}",
                key,
                listed_keys
            );
        }
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let manager = Arc::new(SessionManager::new_memory());
        let mut handles = Vec::new();

        // Spawn multiple tasks accessing the same session
        for i in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let mut session = manager_clone.get_or_create("concurrent").await.unwrap();
                session.add_message(Message::user(&format!("Message {}", i)));
                manager_clone.save(&session).await.unwrap();
            });
            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // Session should exist with some messages (exact count depends on race conditions)
        let session = manager.get("concurrent").await.unwrap().unwrap();
        assert!(!session.messages.is_empty());
    }

    #[tokio::test]
    async fn test_session_with_all_message_types() {
        let manager = SessionManager::new_memory();
        let mut session = manager.get_or_create("all-types").await.unwrap();

        // Add all message types
        session.add_message(Message::system("You are a helpful assistant"));
        session.add_message(Message::user("Search for rust programming"));
        session.add_message(Message::assistant_with_tools(
            "Let me search for that.",
            vec![ToolCall::new("call_1", "search", r#"{"q": "rust"}"#)],
        ));
        session.add_message(Message::tool_result("call_1", "Found 100 results"));
        session.add_message(Message::assistant("I found 100 results about Rust."));

        manager.save(&session).await.unwrap();

        let loaded = manager.get_or_create("all-types").await.unwrap();
        assert_eq!(loaded.messages.len(), 5);
        assert_eq!(loaded.messages[0].role, Role::System);
        assert_eq!(loaded.messages[1].role, Role::User);
        assert_eq!(loaded.messages[2].role, Role::Assistant);
        assert!(loaded.messages[2].has_tool_calls());
        assert_eq!(loaded.messages[3].role, Role::Tool);
        assert!(loaded.messages[3].is_tool_result());
        assert_eq!(loaded.messages[4].role, Role::Assistant);
    }

    #[tokio::test]
    async fn test_session_default() {
        let manager = SessionManager::default();
        let session = manager.get_or_create("test").await.unwrap();
        assert!(session.is_empty());
    }
}
