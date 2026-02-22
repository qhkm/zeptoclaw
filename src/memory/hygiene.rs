//! Memory hygiene — periodic cleanup of stale entries.
//!
//! Provides [`start_hygiene_scheduler`] to run automated cleanup in a background
//! Tokio task, and [`run_hygiene_cycle_memory_only`] for one-shot use.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::memory::longterm::LongTermMemory;

/// Configuration for the memory hygiene scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HygieneConfig {
    /// Whether the scheduler is enabled.
    pub enabled: bool,
    /// How often to run hygiene (in hours).
    pub interval_hours: u64,
    /// Decay score threshold; entries below this are removed.
    pub expired_threshold: f32,
    /// Maximum number of entries to keep; excess are pruned by least-used.
    pub max_entries: usize,
    /// Maximum recent CLI conversations to keep (reserved for future use).
    pub conversation_keep: usize,
}

impl Default for HygieneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: 12,
            expired_threshold: 0.1,
            max_entries: 1000,
            conversation_keep: 50,
        }
    }
}

/// Summary of one hygiene cycle.
pub struct HygieneReport {
    pub expired_removed: usize,
    pub least_used_removed: usize,
    pub conversations_pruned: usize,
}

impl HygieneReport {
    /// Total entries removed across all categories.
    pub fn total(&self) -> usize {
        self.expired_removed + self.least_used_removed + self.conversations_pruned
    }
}

const LAST_HYGIENE_KEY: &str = "system:last_hygiene_at";

/// Return true if hygiene should run given the last run timestamp and interval.
pub fn should_run(last_run_ts: Option<i64>, interval_hours: u64) -> bool {
    let Some(last) = last_run_ts else {
        return true; // never run
    };
    let now = chrono::Utc::now().timestamp();
    let elapsed_hours = ((now - last).max(0) as u64) / 3600;
    elapsed_hours >= interval_hours
}

/// Get the last hygiene run timestamp from memory (read-only, no access-stat update).
pub fn last_run_timestamp(memory: &LongTermMemory) -> Option<i64> {
    memory
        .get_readonly(LAST_HYGIENE_KEY)
        .and_then(|entry| entry.value.parse::<i64>().ok())
}

/// Run one hygiene cycle on longterm memory only (no conversation history).
///
/// Removes expired entries, caps total count, and records the run timestamp.
pub async fn run_hygiene_cycle_memory_only(
    memory: &mut LongTermMemory,
    config: &HygieneConfig,
) -> HygieneReport {
    let mut report = HygieneReport {
        expired_removed: 0,
        least_used_removed: 0,
        conversations_pruned: 0,
    };

    // 1. Remove expired entries (decay score below threshold)
    match memory.cleanup_expired(config.expired_threshold) {
        Ok(n) => report.expired_removed = n,
        Err(e) => warn!("Hygiene: cleanup_expired failed: {}", e),
    }

    // 2. Cap total entries if over limit
    if memory.count() > config.max_entries {
        match memory.cleanup_least_used(config.max_entries) {
            Ok(n) => report.least_used_removed = n,
            Err(e) => warn!("Hygiene: cleanup_least_used failed: {}", e),
        }
    }

    // 3. Record last run timestamp
    let now_str = chrono::Utc::now().timestamp().to_string();
    if let Err(e) = memory
        .set(LAST_HYGIENE_KEY, &now_str, "system", vec![], 0.1)
        .await
    {
        warn!("Hygiene: failed to record timestamp: {}", e);
    }

    report
}

/// Start the hygiene scheduler as a background task.
///
/// The scheduler checks every hour whether a full cycle is due, runs the
/// cycle when needed, and then sleeps until the next check. Disabled
/// immediately if `config.enabled` is false.
pub fn start_hygiene_scheduler(
    memory: Arc<Mutex<LongTermMemory>>,
    config: HygieneConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if !config.enabled {
            info!("Memory hygiene disabled");
            return;
        }

        loop {
            // Check if a cycle is due
            let should = {
                let mem = memory.lock().await;
                let last = last_run_timestamp(&mem);
                should_run(last, config.interval_hours)
            };

            if should {
                let report = {
                    let mut mem = memory.lock().await;
                    run_hygiene_cycle_memory_only(&mut mem, &config).await
                };

                if report.total() > 0 {
                    info!(
                        "Hygiene: removed {} expired, {} least-used",
                        report.expired_removed, report.least_used_removed
                    );
                }
            }

            // Re-check every hour
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_memory() -> (LongTermMemory, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("longterm.json");
        let mem = LongTermMemory::with_path(path).expect("memory");
        (mem, dir)
    }

    #[test]
    fn test_hygiene_config_defaults() {
        let config = HygieneConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval_hours, 12);
        assert!((config.expired_threshold - 0.1).abs() < f32::EPSILON);
        assert_eq!(config.max_entries, 1000);
        assert_eq!(config.conversation_keep, 50);
    }

    #[test]
    fn test_hygiene_config_disabled() {
        let config = HygieneConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!config.enabled);
    }

    #[tokio::test]
    async fn test_run_hygiene_cycle_empty_memory() {
        let (mut mem, _dir) = temp_memory();
        let config = HygieneConfig::default();
        let report = run_hygiene_cycle_memory_only(&mut mem, &config).await;
        assert_eq!(report.expired_removed, 0);
        assert_eq!(report.least_used_removed, 0);
    }

    #[tokio::test]
    async fn test_run_hygiene_cycle_respects_max_entries() {
        let (mut mem, _dir) = temp_memory();
        let config = HygieneConfig {
            max_entries: 2,
            ..Default::default()
        };
        mem.set("k1", "v1", "general", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "general", vec![], 1.0).await.unwrap();
        mem.set("k3", "v3", "general", vec![], 1.0).await.unwrap();

        let report = run_hygiene_cycle_memory_only(&mut mem, &config).await;
        // system:last_hygiene_at is also added, so count can be max_entries+1
        // (the cleanup runs before timestamp is recorded)
        assert!(
            mem.count() <= config.max_entries + 1,
            "count should be <= {}, got {}",
            config.max_entries + 1,
            mem.count()
        );
        assert!(report.least_used_removed > 0);
    }

    #[test]
    fn test_should_run_no_timestamp() {
        assert!(should_run(None, 12));
    }

    #[test]
    fn test_should_run_recent() {
        let one_hour_ago = chrono::Utc::now().timestamp() - 3600;
        assert!(!should_run(Some(one_hour_ago), 12));
    }

    #[test]
    fn test_should_run_overdue() {
        let day_ago = chrono::Utc::now().timestamp() - 86400;
        assert!(should_run(Some(day_ago), 12));
    }

    #[test]
    fn test_report_total() {
        let report = HygieneReport {
            expired_removed: 3,
            least_used_removed: 5,
            conversations_pruned: 2,
        };
        assert_eq!(report.total(), 10);
    }

    #[tokio::test]
    async fn test_hygiene_records_timestamp() {
        let (mut mem, _dir) = temp_memory();
        let config = HygieneConfig::default();
        assert!(last_run_timestamp(&mem).is_none());
        run_hygiene_cycle_memory_only(&mut mem, &config).await;
        assert!(last_run_timestamp(&mem).is_some());
    }
}
