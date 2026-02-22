//! LLM response cache with TTL expiry and LRU eviction.
//!
//! Persists to `~/.zeptoclaw/cache/responses.json`. Cache key is a SHA-256
//! digest of `(model, system_prompt, user_prompt)`. Entries expire after a
//! configurable TTL and are evicted LRU when the store reaches capacity.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// A single cached LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The LLM response text.
    pub response: String,
    /// Estimated token count of the response.
    pub token_count: u32,
    /// Unix timestamp when the entry was created.
    pub created_at: u64,
    /// Unix timestamp when the entry was last accessed.
    pub accessed_at: u64,
    /// Number of cache hits for this entry.
    pub hit_count: u32,
}

/// Persistent store serialized to JSON.
#[derive(Debug, Serialize, Deserialize, Default)]
struct CacheStore {
    entries: HashMap<String, CacheEntry>,
}

/// LLM response cache with TTL expiry, LRU eviction, and JSON persistence.
pub struct ResponseCache {
    store: CacheStore,
    path: PathBuf,
    ttl_secs: u64,
    max_entries: usize,
}

impl ResponseCache {
    /// Create a new response cache with the given TTL and capacity.
    ///
    /// Loads existing entries from `~/.zeptoclaw/cache/responses.json` on disk.
    /// `max_entries` is clamped to a minimum of 1 to prevent infinite loops.
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        let path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
            .join("cache")
            .join("responses.json");
        let store = Self::load_from_disk(&path);
        Self {
            store,
            path,
            ttl_secs,
            max_entries: max_entries.max(1),
        }
    }

    /// Build a deterministic cache key: SHA-256 of `(model, system_prompt, user_prompt)`.
    ///
    /// Uses length-prefixed encoding to prevent separator collision attacks
    /// (e.g. `model="a|b"` vs `model="a", system="|b"`).
    pub fn cache_key(model: &str, system_prompt: &str, user_prompt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update((model.len() as u64).to_le_bytes());
        hasher.update(model.as_bytes());
        hasher.update((system_prompt.len() as u64).to_le_bytes());
        hasher.update(system_prompt.as_bytes());
        hasher.update((user_prompt.len() as u64).to_le_bytes());
        hasher.update(user_prompt.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Look up a cached response. Returns `None` if the key is absent or expired.
    ///
    /// On hit, updates `accessed_at` and increments `hit_count` in memory.
    /// Does NOT persist to disk on hit — bookkeeping fields are flushed on
    /// the next `put()` or `clear()` call, avoiding O(n) disk writes per read.
    pub fn get(&mut self, key: &str) -> Option<String> {
        let now = Self::now_secs();
        // Check expiry with an immutable borrow first to avoid overlapping borrows.
        let expired = self
            .store
            .entries
            .get(key)
            .map(|e| now.saturating_sub(e.created_at) > self.ttl_secs);
        match expired {
            Some(true) => {
                debug!(key = %&key[..8.min(key.len())], "Cache entry expired, removing");
                self.store.entries.remove(key);
                // Deferred disk write — flushed on next put() or clear()
                None
            }
            Some(false) => {
                let entry = self.store.entries.get_mut(key).unwrap();
                entry.accessed_at = now;
                entry.hit_count = entry.hit_count.saturating_add(1);
                Some(entry.response.clone())
            }
            None => None,
        }
    }

    /// Store a response in the cache.
    ///
    /// Evicts expired entries first, then LRU entries if at capacity.
    pub fn put(&mut self, key: String, response: String, token_count: u32) {
        let now = Self::now_secs();
        // Evict expired entries first
        self.evict_expired(now);
        // LRU eviction if at capacity (guard max_entries=0 to prevent infinite loop)
        let effective_max = self.max_entries.max(1);
        while self.store.entries.len() >= effective_max {
            self.evict_lru();
        }
        self.store.entries.insert(
            key,
            CacheEntry {
                response,
                token_count,
                created_at: now,
                accessed_at: now,
                hit_count: 0,
            },
        );
        self.save_to_disk();
    }

    /// Return aggregate statistics about the cache.
    pub fn stats(&self) -> CacheStats {
        let total_hits: u64 = self
            .store
            .entries
            .values()
            .map(|e| u64::from(e.hit_count))
            .sum();
        let total_tokens_saved: u64 = self
            .store
            .entries
            .values()
            .map(|e| u64::from(e.hit_count) * u64::from(e.token_count))
            .sum();
        CacheStats {
            total_entries: self.store.entries.len(),
            total_hits,
            total_tokens_saved,
        }
    }

    /// Remove all entries from the cache.
    pub fn clear(&mut self) {
        self.store.entries.clear();
        self.save_to_disk();
    }

    /// Return the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.store.entries.len()
    }

    /// Return `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.store.entries.is_empty()
    }

    // -- private helpers ---------------------------------------------------

    fn evict_expired(&mut self, now: u64) {
        let ttl = self.ttl_secs;
        self.store
            .entries
            .retain(|_, e| now.saturating_sub(e.created_at) <= ttl);
    }

    fn evict_lru(&mut self) {
        if let Some(lru_key) = self
            .store
            .entries
            .iter()
            .min_by_key(|(_, e)| e.accessed_at)
            .map(|(k, _)| k.clone())
        {
            debug!(key = %&lru_key[..8.min(lru_key.len())], "Evicting LRU cache entry");
            self.store.entries.remove(&lru_key);
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn load_from_disk(path: &Path) -> CacheStore {
        match std::fs::read_to_string(path) {
            Ok(data) => match serde_json::from_str(&data) {
                Ok(store) => store,
                Err(e) => {
                    warn!("Response cache file is corrupt, starting empty: {}", e);
                    CacheStore::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CacheStore::default(),
            Err(e) => {
                warn!("Failed to read response cache, starting empty: {}", e);
                CacheStore::default()
            }
        }
    }

    fn save_to_disk(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(&self.store) {
            if let Err(e) = std::fs::write(&self.path, data) {
                warn!("Failed to save response cache: {}", e);
            }
        }
    }
}

/// Aggregate cache statistics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries currently in the cache.
    pub total_entries: usize,
    /// Cumulative number of cache hits across all entries.
    pub total_hits: u64,
    /// Estimated total tokens saved by cache hits.
    pub total_tokens_saved: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test cache with a unique temp path so parallel tests don't collide.
    fn test_cache() -> ResponseCache {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tid = std::thread::current().id();
        ResponseCache {
            store: CacheStore::default(),
            path: PathBuf::from(format!("/tmp/zeptoclaw-test-cache-{tid:?}-{id}.json")),
            ttl_secs: 3600,
            max_entries: 5,
        }
    }

    #[test]
    fn test_cache_key_deterministic() {
        let k1 = ResponseCache::cache_key("gpt-4", "sys", "hello");
        let k2 = ResponseCache::cache_key("gpt-4", "sys", "hello");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_cache_key_model_aware() {
        let k1 = ResponseCache::cache_key("gpt-4", "sys", "hello");
        let k2 = ResponseCache::cache_key("claude", "sys", "hello");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_prompt_aware() {
        let k1 = ResponseCache::cache_key("gpt-4", "sys", "hello");
        let k2 = ResponseCache::cache_key("gpt-4", "sys", "goodbye");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_system_prompt_aware() {
        let k1 = ResponseCache::cache_key("gpt-4", "system A", "hello");
        let k2 = ResponseCache::cache_key("gpt-4", "system B", "hello");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_hit_miss() {
        let mut cache = test_cache();
        let key = "test-key".to_string();
        assert!(cache.get(&key).is_none());
        cache.put(key.clone(), "response".into(), 100);
        assert_eq!(cache.get(&key), Some("response".into()));
    }

    #[test]
    fn test_cache_ttl_expiry() {
        let mut cache = test_cache();
        cache.ttl_secs = 0; // expire immediately
        cache.put("key".into(), "resp".into(), 10);
        // Backdate created_at by 1 second to guarantee expiry
        if let Some(entry) = cache.store.entries.get_mut("key") {
            entry.created_at -= 1;
        }
        assert!(cache.get("key").is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut cache = test_cache(); // max 5 entries
        for i in 0..5 {
            cache.put(format!("k{i}"), format!("v{i}"), 10);
        }
        // Manually set accessed_at to ensure deterministic ordering:
        // k0 = 1000 (most recent), k1 = 100 (oldest), k2-k4 = 500
        cache.store.entries.get_mut("k0").unwrap().accessed_at = 1000;
        cache.store.entries.get_mut("k1").unwrap().accessed_at = 100;
        for i in 2..5 {
            cache
                .store
                .entries
                .get_mut(&format!("k{i}"))
                .unwrap()
                .accessed_at = 500;
        }
        // Add k5 — should evict k1 (oldest accessed_at = 100)
        cache.put("k5".into(), "v5".into(), 10);
        assert!(
            cache.get("k0").is_some(),
            "k0 had most recent access, should survive LRU"
        );
        assert!(
            cache.store.entries.get("k1").is_none(),
            "k1 had oldest accessed_at, should be evicted"
        );
        assert_eq!(cache.store.entries.len(), 5, "should stay at max capacity");
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = test_cache();
        cache.put("k1".into(), "r1".into(), 100);
        cache.put("k2".into(), "r2".into(), 200);
        let _ = cache.get("k1"); // 1 hit
        let _ = cache.get("k1"); // 2 hits
        let _ = cache.get("k2"); // 1 hit
        let stats = cache.stats();
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.total_hits, 3);
        assert_eq!(stats.total_tokens_saved, 100 * 2 + 200 * 1);
    }

    #[test]
    fn test_cache_clear() {
        let mut cache = test_cache();
        cache.put("k1".into(), "r1".into(), 10);
        cache.clear();
        assert_eq!(cache.stats().total_entries, 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_hit_increments_count() {
        let mut cache = test_cache();
        cache.put("k".into(), "r".into(), 10);
        let _ = cache.get("k");
        let _ = cache.get("k");
        let entry = cache.store.entries.get("k").unwrap();
        assert_eq!(entry.hit_count, 2);
    }

    #[test]
    fn test_cache_len_and_is_empty() {
        let mut cache = test_cache();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        cache.put("a".into(), "b".into(), 1);
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_key_no_separator_collision() {
        // "a|b" as model with empty system should differ from "a" model with "b" system
        let k1 = ResponseCache::cache_key("a|b", "", "c");
        let k2 = ResponseCache::cache_key("a", "b", "c");
        assert_ne!(
            k1, k2,
            "length-prefixed encoding must prevent separator collisions"
        );
    }

    #[test]
    fn test_max_entries_zero_clamped() {
        let cache = ResponseCache {
            store: CacheStore::default(),
            path: PathBuf::from("/tmp/zeptoclaw-test-clamp.json"),
            ttl_secs: 3600,
            max_entries: 0,
        };
        // Direct struct construction bypasses the clamp in new(), but
        // the eviction loop still needs to not infinite-loop. We test
        // via new() which clamps to 1.
        let cache2 = ResponseCache::new(3600, 0);
        assert_eq!(cache2.max_entries, 1);
        // Even with direct construction at 0, we need the guard
        drop(cache);
    }

    #[test]
    fn test_cache_config_defaults() {
        use crate::config::CacheConfig;
        let cfg = CacheConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.ttl_secs, 3600);
        assert_eq!(cfg.max_entries, 500);
    }
}
