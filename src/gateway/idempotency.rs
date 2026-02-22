use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// In-memory idempotency store for webhook deduplication.
pub struct IdempotencyStore {
    entries: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
    max_entries: usize,
}

impl IdempotencyStore {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    /// Check if the key is new. Returns true if new (process it),
    /// false if duplicate (skip it). Records the key if new.
    pub fn check_and_record(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();

        // Check for existing non-expired entry
        if let Some(&recorded_at) = entries.get(key) {
            if now.duration_since(recorded_at) < self.ttl {
                return false; // duplicate
            }
        }

        // Evict expired entries first
        let cutoff = now - self.ttl;
        entries.retain(|_, &mut recorded_at| recorded_at > cutoff);

        // Evict oldest if at capacity
        if entries.len() >= self.max_entries {
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, t)| *t)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }

        entries.insert(key.to_string(), now);
        true
    }

    /// Number of tracked entries (for testing/metrics).
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_key_allowed() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 100);
        assert!(store.check_and_record("key1"));
    }

    #[test]
    fn test_duplicate_rejected() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 100);
        assert!(store.check_and_record("key1"));
        assert!(!store.check_and_record("key1")); // duplicate
    }

    #[test]
    fn test_different_keys_independent() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 100);
        assert!(store.check_and_record("key1"));
        assert!(store.check_and_record("key2"));
    }

    #[test]
    fn test_expired_key_reusable() {
        let store = IdempotencyStore::new(Duration::from_millis(50), 100);
        assert!(store.check_and_record("key1"));
        std::thread::sleep(Duration::from_millis(100));
        assert!(store.check_and_record("key1")); // expired, allowed again
    }

    #[test]
    fn test_max_entries_eviction() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 2);
        assert!(store.check_and_record("key1"));
        assert!(store.check_and_record("key2"));
        assert!(store.check_and_record("key3")); // evicts oldest (key1)
        assert!(store.check_and_record("key1")); // key1 was evicted, allowed again
    }

    #[test]
    fn test_entry_count() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 100);
        store.check_and_record("a");
        store.check_and_record("b");
        assert_eq!(store.len(), 2);
    }
}
