//! Event deduplicator for at-least-once delivery.
//!
//! The r8r bridge may replay events on reconnect.  [`Deduplicator`] keeps an
//! LRU set of recently processed event IDs so the handler can skip duplicates.

use std::collections::VecDeque;
use std::time::Instant;

/// LRU set of recently processed event IDs.
///
/// Prevents duplicate processing when the bridge replays events on reconnect.
pub struct Deduplicator {
    seen: VecDeque<(String, Instant)>,
    max_entries: usize,
    ttl_secs: u64,
}

impl Deduplicator {
    /// Create a new deduplicator.
    ///
    /// * `max_entries` — maximum number of event IDs to track (default: 200).
    /// * `ttl_secs` — seconds before an entry expires and can be evicted (default: 600).
    pub fn new(max_entries: usize, ttl_secs: u64) -> Self {
        Self {
            seen: VecDeque::new(),
            max_entries,
            ttl_secs,
        }
    }

    /// Returns `true` if the event ID has **not** been seen before (i.e. it is new).
    ///
    /// 1. Prunes expired entries from the front.
    /// 2. Checks whether `event_id` already exists in the deque.
    /// 3. If duplicate, returns `false`.
    /// 4. If new, adds to back, evicts the oldest entry when at capacity, and
    ///    returns `true`.
    pub fn is_new(&mut self, event_id: &str) -> bool {
        let now = Instant::now();

        // 1. Prune expired entries from front
        while let Some((_, ts)) = self.seen.front() {
            if now.duration_since(*ts).as_secs() >= self.ttl_secs {
                self.seen.pop_front();
            } else {
                break;
            }
        }

        // 2. Check for duplicate
        if self.seen.iter().any(|(id, _)| id == event_id) {
            return false;
        }

        // 3. Evict oldest if at capacity
        if self.seen.len() >= self.max_entries {
            self.seen.pop_front();
        }

        // 4. Add new entry
        self.seen.push_back((event_id.to_string(), now));
        true
    }
}

impl Default for Deduplicator {
    fn default() -> Self {
        Self::new(200, 600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_new_returns_true_then_false() {
        let mut dedup = Deduplicator::new(10, 600);
        assert!(dedup.is_new("evt_1"));
        assert!(!dedup.is_new("evt_1"));
    }

    #[test]
    fn test_distinct_ids_are_new() {
        let mut dedup = Deduplicator::new(10, 600);
        assert!(dedup.is_new("evt_1"));
        assert!(dedup.is_new("evt_2"));
        assert!(dedup.is_new("evt_3"));
    }

    #[test]
    fn test_evicts_oldest_at_capacity() {
        let mut dedup = Deduplicator::new(3, 600);
        assert!(dedup.is_new("evt_1"));
        assert!(dedup.is_new("evt_2"));
        assert!(dedup.is_new("evt_3"));
        // At capacity — adding evt_4 should evict evt_1
        assert!(dedup.is_new("evt_4"));
        // evt_1 was evicted, so it should appear new again
        assert!(dedup.is_new("evt_1"));
        // evt_2 should have been evicted when evt_1 was re-added
        assert!(dedup.is_new("evt_2"));
    }
}
