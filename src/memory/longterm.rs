//! Long-term memory store for ZeptoClaw.
//!
//! Provides persistent key-value memory across sessions -- facts, preferences,
//! and learnings that the agent remembers between conversations. Stored as a
//! single JSON file at `~/.zeptoclaw/memory/longterm.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Result, ZeptoError};

use super::builtin_searcher::BuiltinSearcher;
use super::traits::MemorySearcher;

/// Returns the current unix epoch timestamp in seconds.
fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Default importance value for new memory entries.
fn default_importance() -> f32 {
    1.0
}

/// A single memory entry with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique key (e.g., "user:name", "preference:language", "fact:project-name").
    pub key: String,
    /// The memory content.
    pub value: String,
    /// Category for grouping (e.g., "user", "preference", "fact", "learning").
    pub category: String,
    /// When this memory was created (unix timestamp).
    pub created_at: u64,
    /// When this memory was last accessed (unix timestamp).
    pub last_accessed: u64,
    /// Number of times this memory has been accessed.
    pub access_count: u64,
    /// Optional tags for search.
    pub tags: Vec<String>,
    /// Importance weight (0.0-1.0+, default 1.0). Higher values decay slower.
    #[serde(default = "default_importance")]
    pub importance: f32,
}

impl MemoryEntry {
    /// Calculate decay score based on age and importance.
    /// Pinned entries (category "pinned", case-insensitive) always return 1.0.
    /// Other entries decay at 50% per 30 days, scaled by importance.
    pub fn decay_score(&self) -> f32 {
        if self.category.eq_ignore_ascii_case("pinned") {
            return 1.0;
        }
        let now = now_timestamp();
        let age_secs = now.saturating_sub(self.last_accessed);
        let age_days = age_secs as f64 / 86400.0;
        self.importance * 0.5_f64.powf(age_days / 30.0) as f32
    }
}

/// Long-term memory store persisted as JSON.
pub struct LongTermMemory {
    entries: HashMap<String, MemoryEntry>,
    storage_path: PathBuf,
    searcher: Arc<dyn MemorySearcher>,
}

impl std::fmt::Debug for LongTermMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LongTermMemory")
            .field("entries", &self.entries)
            .field("storage_path", &self.storage_path)
            .field("searcher", &self.searcher.name())
            .finish()
    }
}

impl LongTermMemory {
    /// Create a new long-term memory store at the default path
    /// (`~/.zeptoclaw/memory/longterm.json`). Creates the file and parent
    /// directories if they do not exist.
    pub fn new() -> Result<Self> {
        let path = Config::dir().join("memory").join("longterm.json");
        Self::with_path(path)
    }

    /// Create a long-term memory store at a custom path. Useful for testing.
    pub fn with_path(path: PathBuf) -> Result<Self> {
        Self::with_path_and_searcher(path, Arc::new(BuiltinSearcher))
    }

    /// Create a long-term memory store with a custom searcher.
    pub fn with_path_and_searcher(
        path: PathBuf,
        searcher: Arc<dyn MemorySearcher>,
    ) -> Result<Self> {
        let entries = Self::load(&path)?;
        Ok(Self {
            entries,
            storage_path: path,
            searcher,
        })
    }

    /// Upsert a memory entry. If the key already exists, the value, category,
    /// tags, and importance are updated and `last_accessed` is refreshed. The entry is
    /// persisted to disk immediately and the searcher index is updated.
    pub async fn set(
        &mut self,
        key: &str,
        value: &str,
        category: &str,
        tags: Vec<String>,
        importance: f32,
    ) -> Result<()> {
        let now = now_timestamp();

        if let Some(existing) = self.entries.get_mut(key) {
            existing.value = value.to_string();
            existing.category = category.to_string();
            existing.tags = tags.clone();
            existing.importance = importance;
            existing.last_accessed = now;
        } else {
            let entry = MemoryEntry {
                key: key.to_string(),
                value: value.to_string(),
                category: category.to_string(),
                created_at: now,
                last_accessed: now,
                access_count: 0,
                tags: tags.clone(),
                importance,
            };
            self.entries.insert(key.to_string(), entry);
        }

        self.save()?;

        // Update searcher index with composite searchable text
        let searchable = format!("{} {} {} {}", key, value, category, tags.join(" "));
        self.searcher.index(key, &searchable).await?;

        Ok(())
    }

    /// Retrieve a memory entry by key, updating its access stats
    /// (`last_accessed` and `access_count`). Does NOT auto-save; call
    /// `save()` periodically to persist access stat changes.
    pub fn get(&mut self, key: &str) -> Option<&MemoryEntry> {
        let now = now_timestamp();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_accessed = now;
            entry.access_count += 1;
        }
        self.entries.get(key)
    }

    /// Retrieve a memory entry by key without updating access stats.
    pub fn get_readonly(&self, key: &str) -> Option<&MemoryEntry> {
        self.entries.get(key)
    }

    /// Delete a memory entry by key. Returns `true` if the entry existed
    /// (and was removed), `false` otherwise. Saves to disk and removes from
    /// searcher index on deletion.
    pub async fn delete(&mut self, key: &str) -> Result<bool> {
        let existed = self.entries.remove(key).is_some();
        if existed {
            self.save()?;
            self.searcher.remove(key).await?;
        }
        Ok(existed)
    }

    /// Search across key, value, category, and tags using the injected searcher.
    /// Results are sorted by relevance: exact key matches first, then by
    /// searcher score descending.
    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let query_lower = query.to_lowercase();
        let mut scored: Vec<(&MemoryEntry, f32)> = self
            .entries
            .values()
            .filter_map(|entry| {
                // Build searchable text from all entry fields
                let text = format!(
                    "{} {} {} {}",
                    entry.key,
                    entry.value,
                    entry.category,
                    entry.tags.join(" ")
                );
                let score = self.searcher.score(&text, query);
                if score > 0.0 {
                    Some((entry, score))
                } else {
                    None
                }
            })
            .collect();

        // Exact key matches still get priority
        scored.sort_by(|a, b| {
            let a_exact = a.0.key.to_lowercase() == query_lower;
            let b_exact = b.0.key.to_lowercase() == query_lower;
            match (a_exact, b_exact) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal),
            }
        });

        scored.into_iter().map(|(entry, _)| entry).collect()
    }

    /// List all entries in a given category, sorted by `last_accessed`
    /// descending (most recently accessed first).
    pub fn list_by_category(&self, category: &str) -> Vec<&MemoryEntry> {
        let cat_lower = category.to_lowercase();
        let mut results: Vec<&MemoryEntry> = self
            .entries
            .values()
            .filter(|entry| entry.category.to_lowercase() == cat_lower)
            .collect();

        results.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        results
    }

    /// List all entries, sorted by `last_accessed` descending.
    pub fn list_all(&self) -> Vec<&MemoryEntry> {
        let mut results: Vec<&MemoryEntry> = self.entries.values().collect();
        results.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        results
    }

    /// Return the number of stored entries.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Return a sorted list of unique category names.
    pub fn categories(&self) -> Vec<String> {
        let mut cats: Vec<String> = self
            .entries
            .values()
            .map(|e| e.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        cats.sort();
        cats
    }

    /// Remove entries with the lowest `decay_score` to keep at most
    /// `keep_count` entries. Returns the number of entries removed.
    pub fn cleanup_least_used(&mut self, keep_count: usize) -> Result<usize> {
        if self.entries.len() <= keep_count {
            return Ok(0);
        }

        let mut entries_vec: Vec<(String, f32)> = self
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.decay_score()))
            .collect();

        // Sort by decay_score ascending so that the lowest-scored are first.
        entries_vec.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let to_remove = entries_vec.len() - keep_count;
        let keys_to_remove: Vec<String> = entries_vec
            .into_iter()
            .take(to_remove)
            .map(|(k, _)| k)
            .collect();

        for key in &keys_to_remove {
            self.entries.remove(key);
        }

        self.save()?;
        Ok(to_remove)
    }

    /// Remove entries whose `decay_score()` has fallen below `threshold`.
    /// Pinned entries are never removed (their decay_score is always 1.0).
    /// Returns the number of entries removed.
    ///
    /// `threshold` must be a finite value in the range `0.0..=1.0`.
    pub fn cleanup_expired(&mut self, threshold: f32) -> Result<usize> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(ZeptoError::Config(format!(
                "cleanup_expired threshold must be 0.0..=1.0, got {}",
                threshold
            )));
        }
        let before = self.entries.len();
        self.entries
            .retain(|_, entry| entry.decay_score() >= threshold);
        let count = before - self.entries.len();

        if count > 0 {
            self.save()?;
        }

        Ok(count)
    }

    /// Return a human-readable summary of the memory store.
    pub fn summary(&self) -> String {
        let count = self.count();
        let cat_count = self.categories().len();
        format!(
            "Long-term memory: {} entries ({} categories)",
            count, cat_count
        )
    }

    /// Persist the current memory state to disk as pretty-printed JSON.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ZeptoError::Config(format!(
                    "Failed to create memory directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let json = serde_json::to_string_pretty(&self.entries).map_err(|e| {
            ZeptoError::Config(format!("Failed to serialize long-term memory: {}", e))
        })?;

        std::fs::write(&self.storage_path, json).map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to write long-term memory to {}: {}",
                self.storage_path.display(),
                e
            ))
        })?;

        Ok(())
    }

    /// Load memory entries from a JSON file on disk. Returns an empty map if
    /// the file does not exist.
    fn load(path: &PathBuf) -> Result<HashMap<String, MemoryEntry>> {
        if !path.exists() {
            return Ok(HashMap::new());
        }

        let content = std::fs::read_to_string(path).map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to read long-term memory from {}: {}",
                path.display(),
                e
            ))
        })?;

        if content.trim().is_empty() {
            return Ok(HashMap::new());
        }

        let entries: HashMap<String, MemoryEntry> =
            serde_json::from_str(&content).map_err(|e| {
                ZeptoError::Config(format!("Failed to parse long-term memory JSON: {}", e))
            })?;

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a LongTermMemory backed by a temp directory.
    fn temp_memory() -> (LongTermMemory, TempDir) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("longterm.json");
        let mem = LongTermMemory::with_path(path).expect("failed to create memory");
        (mem, dir)
    }

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry {
            key: "user:name".to_string(),
            value: "Alice".to_string(),
            category: "user".to_string(),
            created_at: 1000,
            last_accessed: 2000,
            access_count: 5,
            tags: vec!["identity".to_string()],
            importance: 1.0,
        };

        assert_eq!(entry.key, "user:name");
        assert_eq!(entry.value, "Alice");
        assert_eq!(entry.category, "user");
        assert_eq!(entry.created_at, 1000);
        assert_eq!(entry.last_accessed, 2000);
        assert_eq!(entry.access_count, 5);
        assert_eq!(entry.tags, vec!["identity"]);
        assert_eq!(entry.importance, 1.0);
    }

    #[test]
    fn test_longterm_memory_new_empty() {
        let (mem, _dir) = temp_memory();
        assert_eq!(mem.count(), 0);
    }

    #[tokio::test]
    async fn test_set_and_get() {
        let (mut mem, _dir) = temp_memory();
        mem.set(
            "user:name",
            "Alice",
            "user",
            vec!["identity".to_string()],
            1.0,
        )
        .await
        .unwrap();

        let entry = mem.get("user:name").unwrap();
        assert_eq!(entry.value, "Alice");
        assert_eq!(entry.category, "user");
    }

    #[tokio::test]
    async fn test_set_upsert() {
        let (mut mem, _dir) = temp_memory();
        mem.set("user:name", "Alice", "user", vec![], 1.0)
            .await
            .unwrap();
        mem.set("user:name", "Bob", "user", vec!["updated".to_string()], 1.0)
            .await
            .unwrap();

        let entry = mem.get("user:name").unwrap();
        assert_eq!(entry.value, "Bob");
        assert_eq!(entry.tags, vec!["updated"]);
        assert_eq!(mem.count(), 1);
    }

    #[tokio::test]
    async fn test_get_updates_access_stats() {
        let (mut mem, _dir) = temp_memory();
        mem.set("key1", "value1", "test", vec![], 1.0)
            .await
            .unwrap();

        let before_access = mem.get_readonly("key1").unwrap().last_accessed;
        let before_count = mem.get_readonly("key1").unwrap().access_count;

        let _ = mem.get("key1");
        let _ = mem.get("key1");

        let entry = mem.get_readonly("key1").unwrap();
        assert_eq!(entry.access_count, before_count + 2);
        assert!(entry.last_accessed >= before_access);
    }

    #[tokio::test]
    async fn test_get_readonly_no_update() {
        let (mut mem, _dir) = temp_memory();
        mem.set("key1", "value1", "test", vec![], 1.0)
            .await
            .unwrap();

        let before = mem.get_readonly("key1").unwrap().access_count;
        let _ = mem.get_readonly("key1");
        let _ = mem.get_readonly("key1");
        let after = mem.get_readonly("key1").unwrap().access_count;

        assert_eq!(before, after);
    }

    #[test]
    fn test_get_nonexistent() {
        let (mut mem, _dir) = temp_memory();
        assert!(mem.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_delete_existing() {
        let (mut mem, _dir) = temp_memory();
        mem.set("key1", "value1", "test", vec![], 1.0)
            .await
            .unwrap();
        assert_eq!(mem.count(), 1);

        let existed = mem.delete("key1").await.unwrap();
        assert!(existed);
        assert_eq!(mem.count(), 0);
        assert!(mem.get("key1").is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let (mut mem, _dir) = temp_memory();
        let existed = mem.delete("nonexistent").await.unwrap();
        assert!(!existed);
    }

    #[tokio::test]
    async fn test_search_by_key() {
        let (mut mem, _dir) = temp_memory();
        mem.set("user:name", "Alice", "user", vec![], 1.0)
            .await
            .unwrap();
        mem.set("project:name", "ZeptoClaw", "project", vec![], 1.0)
            .await
            .unwrap();

        let results = mem.search("user");
        assert!(!results.is_empty());
        assert!(results.iter().any(|e| e.key == "user:name"));
    }

    #[tokio::test]
    async fn test_search_by_value() {
        let (mut mem, _dir) = temp_memory();
        mem.set("key1", "Rust programming language", "fact", vec![], 1.0)
            .await
            .unwrap();
        mem.set("key2", "Python scripting", "fact", vec![], 1.0)
            .await
            .unwrap();

        let results = mem.search("Rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "key1");
    }

    #[tokio::test]
    async fn test_search_by_tag() {
        let (mut mem, _dir) = temp_memory();
        mem.set(
            "key1",
            "some value",
            "test",
            vec!["important".to_string(), "work".to_string()],
            1.0,
        )
        .await
        .unwrap();
        mem.set(
            "key2",
            "other value",
            "test",
            vec!["personal".to_string()],
            1.0,
        )
        .await
        .unwrap();

        let results = mem.search("important");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "key1");
    }

    #[tokio::test]
    async fn test_search_case_insensitive() {
        let (mut mem, _dir) = temp_memory();
        mem.set(
            "Key1",
            "Hello World",
            "Test",
            vec!["MyTag".to_string()],
            1.0,
        )
        .await
        .unwrap();

        assert!(!mem.search("hello").is_empty());
        assert!(!mem.search("HELLO").is_empty());
        assert!(!mem.search("key1").is_empty());
        assert!(!mem.search("KEY1").is_empty());
        assert!(!mem.search("mytag").is_empty());
        assert!(!mem.search("test").is_empty());
    }

    #[tokio::test]
    async fn test_list_by_category() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "v1", "user", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "user", vec![], 1.0).await.unwrap();
        mem.set("k3", "v3", "project", vec![], 1.0).await.unwrap();

        let user_entries = mem.list_by_category("user");
        assert_eq!(user_entries.len(), 2);
        assert!(user_entries.iter().all(|e| e.category == "user"));

        let project_entries = mem.list_by_category("project");
        assert_eq!(project_entries.len(), 1);
    }

    #[tokio::test]
    async fn test_list_all() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "v1", "a", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "b", vec![], 1.0).await.unwrap();
        mem.set("k3", "v3", "c", vec![], 1.0).await.unwrap();

        let all = mem.list_all();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_count() {
        let (mut mem, _dir) = temp_memory();
        assert_eq!(mem.count(), 0);

        mem.set("k1", "v1", "test", vec![], 1.0).await.unwrap();
        assert_eq!(mem.count(), 1);

        mem.set("k2", "v2", "test", vec![], 1.0).await.unwrap();
        assert_eq!(mem.count(), 2);

        mem.delete("k1").await.unwrap();
        assert_eq!(mem.count(), 1);
    }

    #[tokio::test]
    async fn test_categories() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "v1", "user", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "fact", vec![], 1.0).await.unwrap();
        mem.set("k3", "v3", "user", vec![], 1.0).await.unwrap();
        mem.set("k4", "v4", "preference", vec![], 1.0)
            .await
            .unwrap();

        let cats = mem.categories();
        assert_eq!(cats, vec!["fact", "preference", "user"]);
    }

    #[tokio::test]
    async fn test_cleanup_least_used() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "v1", "test", vec![], 0.5).await.unwrap();
        mem.set("k2", "v2", "test", vec![], 0.3).await.unwrap();
        mem.set("k3", "v3", "test", vec![], 1.0).await.unwrap();

        let removed = mem.cleanup_least_used(2).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(mem.count(), 2);
        assert!(mem.get_readonly("k3").is_some());
        assert!(mem.get_readonly("k1").is_some());
        assert!(mem.get_readonly("k2").is_none());
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("longterm.json");

        {
            let mut mem = LongTermMemory::with_path(path.clone()).unwrap();
            mem.set(
                "user:name",
                "Alice",
                "user",
                vec!["identity".to_string()],
                1.0,
            )
            .await
            .unwrap();
            mem.set("fact:lang", "Rust", "fact", vec!["tech".to_string()], 1.0)
                .await
                .unwrap();
        }

        {
            let mem = LongTermMemory::with_path(path).unwrap();
            assert_eq!(mem.count(), 2);
            let entry = mem.get_readonly("user:name").unwrap();
            assert_eq!(entry.value, "Alice");
            assert_eq!(entry.tags, vec!["identity"]);

            let entry2 = mem.get_readonly("fact:lang").unwrap();
            assert_eq!(entry2.value, "Rust");
        }
    }

    #[tokio::test]
    async fn test_summary() {
        let (mut mem, _dir) = temp_memory();
        assert_eq!(mem.summary(), "Long-term memory: 0 entries (0 categories)");

        mem.set("k1", "v1", "user", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "fact", vec![], 1.0).await.unwrap();
        mem.set("k3", "v3", "fact", vec![], 1.0).await.unwrap();

        assert_eq!(mem.summary(), "Long-term memory: 3 entries (2 categories)");
    }

    #[tokio::test]
    async fn test_decay_score_fresh_entry() {
        let (mut mem, _dir) = temp_memory();
        mem.set("fresh", "value", "test", vec![], 1.0)
            .await
            .unwrap();

        let entry = mem.get_readonly("fresh").unwrap();
        let score = entry.decay_score();
        assert!(
            (score - 1.0).abs() < 0.01,
            "Fresh entry score was {}, expected ~1.0",
            score
        );
    }

    #[tokio::test]
    async fn test_decay_score_pinned_exempt() {
        let (mut mem, _dir) = temp_memory();
        mem.set("pinned_key", "value", "pinned", vec![], 1.0)
            .await
            .unwrap();

        if let Some(entry) = mem.entries.get_mut("pinned_key") {
            entry.last_accessed = now_timestamp() - (365 * 86400);
        }

        let entry = mem.get_readonly("pinned_key").unwrap();
        let score = entry.decay_score();
        assert_eq!(score, 1.0, "Pinned entry should score 1.0, got {}", score);
    }

    #[tokio::test]
    async fn test_decay_score_pinned_case_insensitive() {
        let (mut mem, _dir) = temp_memory();
        mem.set("pinned_key", "value", "Pinned", vec![], 1.0)
            .await
            .unwrap();

        if let Some(entry) = mem.entries.get_mut("pinned_key") {
            entry.last_accessed = now_timestamp() - (365 * 86400);
        }

        let entry = mem.get_readonly("pinned_key").unwrap();
        let score = entry.decay_score();
        assert_eq!(
            score, 1.0,
            "Pinned (capital) entry should score 1.0, got {}",
            score
        );
    }

    #[tokio::test]
    async fn test_decay_score_old_entry_decays() {
        let (mut mem, _dir) = temp_memory();
        mem.set("old", "value", "test", vec![], 1.0).await.unwrap();

        if let Some(entry) = mem.entries.get_mut("old") {
            entry.last_accessed = now_timestamp() - (30 * 86400);
        }

        let entry = mem.get_readonly("old").unwrap();
        let score = entry.decay_score();
        assert!(
            (score - 0.5).abs() < 0.05,
            "30-day-old entry score was {}, expected ~0.5",
            score
        );
    }

    #[tokio::test]
    async fn test_decay_score_importance_scales() {
        let (mut mem, _dir) = temp_memory();
        mem.set("low_importance", "value", "test", vec![], 0.5)
            .await
            .unwrap();

        let entry = mem.get_readonly("low_importance").unwrap();
        let score = entry.decay_score();
        assert!(
            (score - 0.5).abs() < 0.01,
            "Low importance entry score was {}, expected ~0.5",
            score
        );
    }

    #[tokio::test]
    async fn test_search_sorted_by_searcher_score() {
        let (mut mem, _dir) = temp_memory();
        mem.set("fresh", "test value", "test", vec![], 1.0)
            .await
            .unwrap();
        mem.set("old", "test value", "test", vec![], 1.0)
            .await
            .unwrap();

        if let Some(entry) = mem.entries.get_mut("old") {
            entry.last_accessed = now_timestamp() - (60 * 86400);
        }

        let results = mem.search("test");
        assert_eq!(results.len(), 2);

        let keys: Vec<&str> = results.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"fresh"));
        assert!(keys.contains(&"old"));
    }

    #[tokio::test]
    async fn test_cleanup_evicts_by_decay_score() {
        let (mut mem, _dir) = temp_memory();
        mem.set("high", "value", "test", vec![], 2.0).await.unwrap();
        mem.set("medium", "value", "test", vec![], 1.0)
            .await
            .unwrap();
        mem.set("low", "value", "test", vec![], 0.5).await.unwrap();

        let removed = mem.cleanup_least_used(1).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(mem.count(), 1);
        assert!(mem.get_readonly("high").is_some());
        assert!(mem.get_readonly("medium").is_none());
        assert!(mem.get_readonly("low").is_none());
    }

    #[tokio::test]
    async fn test_importance_persists_roundtrip() {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("longterm.json");

        {
            let mut mem = LongTermMemory::with_path(path.clone()).unwrap();
            mem.set("high", "value", "test", vec![], 2.5).await.unwrap();
            mem.set("low", "value", "test", vec![], 0.3).await.unwrap();
        }

        {
            let mem = LongTermMemory::with_path(path).unwrap();
            assert_eq!(mem.count(), 2);
            assert_eq!(mem.get_readonly("high").unwrap().importance, 2.5);
            assert_eq!(mem.get_readonly("low").unwrap().importance, 0.3);
        }
    }

    #[tokio::test]
    async fn test_cleanup_expired_removes_low_score() {
        let (mut mem, _dir) = temp_memory();
        mem.set("high", "value", "test", vec![], 2.0).await.unwrap();
        mem.set("low", "value", "test", vec![], 0.01).await.unwrap();

        if let Some(entry) = mem.entries.get_mut("low") {
            entry.last_accessed = now_timestamp() - (90 * 86400);
            entry.importance = 0.01;
        }

        let removed = mem.cleanup_expired(0.1).unwrap();
        assert_eq!(removed, 1);
        assert!(mem.get_readonly("high").is_some());
        assert!(mem.get_readonly("low").is_none());
    }

    #[tokio::test]
    async fn test_cleanup_expired_keeps_pinned() {
        let (mut mem, _dir) = temp_memory();
        mem.set("pinned_key", "value", "pinned", vec![], 0.01)
            .await
            .unwrap();

        if let Some(entry) = mem.entries.get_mut("pinned_key") {
            entry.last_accessed = now_timestamp() - (365 * 86400);
        }

        let removed = mem.cleanup_expired(0.5).unwrap();
        assert_eq!(removed, 0, "Pinned entries should never be cleaned up");
        assert!(mem.get_readonly("pinned_key").is_some());
    }

    #[tokio::test]
    async fn test_cleanup_expired_no_op_when_all_fresh() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "v1", "test", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "test", vec![], 1.0).await.unwrap();

        let removed = mem.cleanup_expired(0.1).unwrap();
        assert_eq!(removed, 0);
        assert_eq!(mem.count(), 2);
    }

    #[test]
    fn test_cleanup_expired_empty_memory() {
        let (mut mem, _dir) = temp_memory();
        let removed = mem.cleanup_expired(0.5).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_cleanup_expired_rejects_invalid_threshold() {
        let (mut mem, _dir) = temp_memory();

        assert!(mem.cleanup_expired(-0.1).is_err());
        assert!(mem.cleanup_expired(1.1).is_err());
        assert!(mem.cleanup_expired(f32::NAN).is_err());
        assert!(mem.cleanup_expired(f32::INFINITY).is_err());
        assert!(mem.cleanup_expired(f32::NEG_INFINITY).is_err());
        assert!(mem.cleanup_expired(0.0).is_ok());
        assert!(mem.cleanup_expired(1.0).is_ok());
    }

    #[tokio::test]
    async fn test_search_uses_injected_searcher() {
        use crate::memory::traits::MemorySearcher;
        use std::sync::Arc;

        struct MagicSearcher;

        #[async_trait::async_trait]
        impl MemorySearcher for MagicSearcher {
            fn name(&self) -> &str {
                "magic"
            }
            fn score(&self, chunk: &str, _query: &str) -> f32 {
                if chunk.to_lowercase().contains("magic") {
                    1.0
                } else {
                    0.0
                }
            }
        }

        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("lt.json");
        let searcher = Arc::new(MagicSearcher);
        let mut mem = LongTermMemory::with_path_and_searcher(path, searcher).unwrap();

        mem.set("k1", "magic word", "test", vec![], 1.0)
            .await
            .unwrap();
        mem.set("k2", "normal word", "test", vec![], 1.0)
            .await
            .unwrap();

        let results = mem.search("anything");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "k1");
    }
}
