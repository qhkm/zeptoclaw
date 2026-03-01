//! Per-provider usage quota management.
//!
//! Tracks cost (USD) and token consumption per provider across configurable
//! time periods (monthly or daily). Provides quota checks that return `Ok`,
//! `Warning`, or `Exceeded` based on configurable thresholds.
//!
//! # Example
//!
//! ```rust
//! use zeptoclaw::providers::quota::{QuotaConfig, QuotaStore, QuotaCheckResult, QuotaPeriod};
//!
//! let store = QuotaStore::load_or_default();
//! let config = QuotaConfig {
//!     max_cost_usd: Some(50.0),
//!     max_tokens: None,
//!     period: QuotaPeriod::Monthly,
//!     action: zeptoclaw::providers::quota::QuotaAction::Reject,
//! };
//!
//! // Record some usage
//! store.record("anthropic", &config.period, 10.0, 5000);
//!
//! // Check against limits
//! match store.check("anthropic", &config) {
//!     QuotaCheckResult::Ok => println!("within quota"),
//!     QuotaCheckResult::Warning(pct) => println!("at {:.0}% of quota", pct * 100.0),
//!     QuotaCheckResult::Exceeded => println!("quota exceeded"),
//! }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Reset cadence for quota counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuotaPeriod {
    /// Counters reset at the start of each calendar month (default).
    #[default]
    Monthly,
    /// Counters reset at midnight UTC each day.
    Daily,
}

/// Action to take when a provider's quota is exceeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuotaAction {
    /// Reject the request with `ZeptoError::QuotaExceeded` (default).
    #[default]
    Reject,
    /// Attempt to fall back to a secondary provider instead.
    Fallback,
    /// Log a warning but allow the request to proceed.
    Warn,
}

/// Per-provider quota configuration embedded in `ProviderConfig`.
///
/// All fields are optional — an unconfigured `QuotaConfig` (all `None` limits)
/// always returns `QuotaCheckResult::Ok`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaConfig {
    /// Maximum spend in USD for the period. `None` means no cost limit.
    pub max_cost_usd: Option<f64>,
    /// Maximum token count for the period. `None` means no token limit.
    pub max_tokens: Option<u64>,
    /// When the period counter resets.
    pub period: QuotaPeriod,
    /// What to do when the quota is exceeded.
    pub action: QuotaAction,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            max_cost_usd: None,
            max_tokens: None,
            period: QuotaPeriod::Monthly,
            action: QuotaAction::Reject,
        }
    }
}

/// Persisted usage counter for a single provider within a period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaUsage {
    /// The period this counter belongs to.
    ///
    /// Format: `"2026-03"` for monthly, `"2026-03-01"` for daily.
    pub period_key: String,
    /// Accumulated cost in USD for this period.
    pub cost_usd: f64,
    /// Accumulated token count for this period.
    pub tokens: u64,
}

/// Result of a quota check for a provider.
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaCheckResult {
    /// Usage is below the warning threshold (80%).
    Ok,
    /// Usage is at or above 80% of the limit. Inner value is the highest
    /// utilisation fraction across all configured limits (`0.0..1.0`).
    Warning(f64),
    /// At least one limit is at or above 100%.
    Exceeded,
}

/// Persistent store for per-provider quota usage.
///
/// Thread-safe via an internal `Mutex`. Persists state to
/// `~/.zeptoclaw/quota/usage.json` after every `record()` call (best-effort;
/// write errors are silently ignored).
pub struct QuotaStore {
    /// In-memory state: provider name → usage for the current period.
    state: Mutex<HashMap<String, QuotaUsage>>,
    /// Path of the JSON file used for persistence.
    path: PathBuf,
}

impl QuotaStore {
    /// Load usage state from `~/.zeptoclaw/quota/usage.json`.
    ///
    /// Returns an empty store if the file does not exist or cannot be parsed.
    pub fn load_or_default() -> Self {
        let path = dirs_path();
        let state = load_state(&path);
        Self {
            state: Mutex::new(state),
            path,
        }
    }

    /// Compute the period key for `now()` given a reset cadence.
    ///
    /// Returns `"YYYY-MM"` for `Monthly` and `"YYYY-MM-DD"` for `Daily`.
    pub fn current_period_key(period: &QuotaPeriod) -> String {
        let now = Utc::now();
        match period {
            QuotaPeriod::Monthly => now.format("%Y-%m").to_string(),
            QuotaPeriod::Daily => now.format("%Y-%m-%d").to_string(),
        }
    }

    /// Check whether the named provider is within its configured quota.
    ///
    /// Returns:
    /// - `Ok` when below the 80% warning threshold (or no limits configured).
    /// - `Warning(pct)` when utilisation is ≥ 80% but < 100%.
    /// - `Exceeded` when any limit is ≥ 100%.
    pub fn check(&self, provider: &str, config: &QuotaConfig) -> QuotaCheckResult {
        // No limits configured — always Ok.
        if config.max_cost_usd.is_none() && config.max_tokens.is_none() {
            return QuotaCheckResult::Ok;
        }

        let current_key = Self::current_period_key(&config.period);

        let guard = self.state.lock().expect("quota state lock poisoned");
        let usage = match guard.get(provider) {
            Some(u) if u.period_key == current_key => u,
            // No entry or stale period → nothing recorded yet.
            _ => return QuotaCheckResult::Ok,
        };

        let cost_pct = config
            .max_cost_usd
            .map(|max| if max > 0.0 { usage.cost_usd / max } else { 0.0 })
            .unwrap_or(0.0);

        let token_pct = config
            .max_tokens
            .map(|max| {
                if max > 0 {
                    usage.tokens as f64 / max as f64
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);

        let max_pct = cost_pct.max(token_pct);

        if max_pct >= 1.0 {
            QuotaCheckResult::Exceeded
        } else if max_pct >= 0.8 {
            QuotaCheckResult::Warning(max_pct)
        } else {
            QuotaCheckResult::Ok
        }
    }

    /// Record usage for a provider, resetting the counter if the period rolled over.
    ///
    /// Persists the updated state to disk (best-effort; errors are ignored).
    pub fn record(&self, provider: &str, period: &QuotaPeriod, cost_usd: f64, tokens: u64) {
        let current_key = Self::current_period_key(period);

        let mut guard = self.state.lock().expect("quota state lock poisoned");

        let entry = guard
            .entry(provider.to_string())
            .or_insert_with(|| QuotaUsage {
                period_key: current_key.clone(),
                cost_usd: 0.0,
                tokens: 0,
            });

        // Reset if the period has rolled over.
        if entry.period_key != current_key {
            entry.period_key = current_key;
            entry.cost_usd = 0.0;
            entry.tokens = 0;
        }

        entry.cost_usd += cost_usd;
        entry.tokens += tokens;

        // Persist best-effort — drop the guard first to keep the critical
        // section as short as possible.
        let snapshot: HashMap<String, QuotaUsage> = guard.clone();
        drop(guard);

        persist_state(&self.path, &snapshot);
    }

    /// Return a point-in-time snapshot of all provider usage entries.
    pub fn snapshot(&self) -> HashMap<String, QuotaUsage> {
        self.state
            .lock()
            .expect("quota state lock poisoned")
            .clone()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Canonical path for the usage file: `~/.zeptoclaw/quota/usage.json`.
fn dirs_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".zeptoclaw").join("quota").join("usage.json")
}

/// Load `HashMap<String, QuotaUsage>` from JSON; returns empty map on error.
fn load_state(path: &PathBuf) -> HashMap<String, QuotaUsage> {
    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Persist `state` to `path` (best-effort; ignores all errors).
fn persist_state(path: &PathBuf, state: &HashMap<String, QuotaUsage>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, json);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a `QuotaStore` rooted in a temporary directory.
    fn store_in_tmpdir(tmp: &TempDir) -> QuotaStore {
        QuotaStore {
            state: Mutex::new(HashMap::new()),
            path: tmp.path().join("usage.json"),
        }
    }

    // --- QuotaConfig defaults ---

    #[test]
    fn test_quota_config_default_period_is_monthly() {
        let cfg = QuotaConfig::default();
        assert_eq!(cfg.period, QuotaPeriod::Monthly);
    }

    #[test]
    fn test_quota_config_default_action_is_reject() {
        let cfg = QuotaConfig::default();
        assert_eq!(cfg.action, QuotaAction::Reject);
    }

    #[test]
    fn test_quota_config_no_limits_by_default() {
        let cfg = QuotaConfig::default();
        assert!(cfg.max_cost_usd.is_none());
        assert!(cfg.max_tokens.is_none());
    }

    // --- Period key format ---

    #[test]
    fn test_monthly_period_key_format() {
        let key = QuotaStore::current_period_key(&QuotaPeriod::Monthly);
        // "YYYY-MM" — 7 chars, one dash
        assert_eq!(key.len(), 7, "monthly key should be 7 chars: {key}");
        assert!(key.contains('-'), "monthly key must contain a dash: {key}");
        // Only one dash
        assert_eq!(
            key.chars().filter(|c| *c == '-').count(),
            1,
            "monthly key should have exactly one dash: {key}"
        );
    }

    #[test]
    fn test_daily_period_key_format() {
        let key = QuotaStore::current_period_key(&QuotaPeriod::Daily);
        // "YYYY-MM-DD" — 10 chars, two dashes
        assert_eq!(key.len(), 10, "daily key should be 10 chars: {key}");
        assert_eq!(
            key.chars().filter(|c| *c == '-').count(),
            2,
            "daily key should have exactly two dashes: {key}"
        );
    }

    // --- check() logic ---

    #[test]
    fn test_check_no_usage_is_ok() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: Some(10_000),
            ..Default::default()
        };
        assert_eq!(store.check("openai", &cfg), QuotaCheckResult::Ok);
    }

    #[test]
    fn test_check_under_80pct_is_ok() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: None,
            ..Default::default()
        };
        // Record 70% usage
        store.record("openai", &cfg.period, 70.0, 0);
        assert_eq!(store.check("openai", &cfg), QuotaCheckResult::Ok);
    }

    #[test]
    fn test_check_over_80pct_is_warning() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: None,
            ..Default::default()
        };
        // Record 85% usage
        store.record("openai", &cfg.period, 85.0, 0);
        match store.check("openai", &cfg) {
            QuotaCheckResult::Warning(pct) => {
                assert!(pct >= 0.85, "warning pct should be >= 0.85, got {pct}");
                assert!(pct < 1.0, "warning pct should be < 1.0, got {pct}");
            }
            other => panic!("expected Warning, got {other:?}"),
        }
    }

    #[test]
    fn test_check_at_100pct_is_exceeded() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: None,
            ..Default::default()
        };
        store.record("openai", &cfg.period, 100.0, 0);
        assert_eq!(store.check("openai", &cfg), QuotaCheckResult::Exceeded);
    }

    #[test]
    fn test_check_over_100pct_is_exceeded() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: None,
            ..Default::default()
        };
        store.record("openai", &cfg.period, 150.0, 0);
        assert_eq!(store.check("openai", &cfg), QuotaCheckResult::Exceeded);
    }

    #[test]
    fn test_check_token_limit_exceeded() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: None,
            max_tokens: Some(10_000),
            ..Default::default()
        };
        store.record("openai", &cfg.period, 0.0, 12_000);
        assert_eq!(store.check("openai", &cfg), QuotaCheckResult::Exceeded);
    }

    #[test]
    fn test_check_no_limits_always_ok() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        // Default config has no limits
        let cfg = QuotaConfig::default();
        // Record huge usage — should still be Ok
        store.record("openai", &cfg.period, 999_999.0, u64::MAX / 2);
        assert_eq!(store.check("openai", &cfg), QuotaCheckResult::Ok);
    }

    // --- record() logic ---

    #[test]
    fn test_record_resets_on_period_change() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);

        // Manually insert a stale period entry
        {
            let mut guard = store.state.lock().unwrap();
            guard.insert(
                "anthropic".to_string(),
                QuotaUsage {
                    period_key: "2020-01".to_string(), // old period
                    cost_usd: 999.0,
                    tokens: 999_999,
                },
            );
        }

        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: Some(10_000),
            ..Default::default()
        };

        // Record for the *current* period — should reset first
        store.record("anthropic", &cfg.period, 5.0, 500);

        let snap = store.snapshot();
        let usage = snap.get("anthropic").expect("entry should exist");
        assert_eq!(
            usage.period_key,
            QuotaStore::current_period_key(&cfg.period),
            "period_key should be updated"
        );
        // cost and tokens should reflect only the new record, not the old 999
        assert!(
            (usage.cost_usd - 5.0).abs() < 1e-9,
            "cost_usd should be 5.0 after reset, got {}",
            usage.cost_usd
        );
        assert_eq!(usage.tokens, 500, "tokens should be 500 after reset");
    }

    #[test]
    fn test_snapshot_is_empty_initially() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn test_record_writes_to_disk() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let path = tmp.path().join("usage.json");

        // File should not exist before any record
        assert!(!path.exists(), "file should not exist before record");

        store.record("anthropic", &QuotaPeriod::Monthly, 12.5, 1000);

        assert!(path.exists(), "file should exist after record");

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("anthropic"),
            "file should contain provider name"
        );
        assert!(contents.contains("12.5"), "file should contain cost value");
    }

    // --- Serde roundtrips ---

    #[test]
    fn test_quota_period_serde() {
        let encoded = serde_json::to_string(&QuotaPeriod::Daily).unwrap();
        assert_eq!(encoded, "\"daily\"");
        let decoded: QuotaPeriod = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, QuotaPeriod::Daily);
    }

    #[test]
    fn test_quota_action_serde() {
        let encoded = serde_json::to_string(&QuotaAction::Fallback).unwrap();
        assert_eq!(encoded, "\"fallback\"");
        let decoded: QuotaAction = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, QuotaAction::Fallback);
    }

    #[test]
    fn test_quota_config_serde_roundtrip() {
        let original = QuotaConfig {
            max_cost_usd: Some(50.0),
            max_tokens: Some(100_000),
            period: QuotaPeriod::Daily,
            action: QuotaAction::Warn,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: QuotaConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    // --- Additional robustness tests ---

    #[test]
    fn test_record_accumulates_across_calls() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);

        store.record("openai", &QuotaPeriod::Monthly, 10.0, 1_000);
        store.record("openai", &QuotaPeriod::Monthly, 15.0, 2_000);
        store.record("openai", &QuotaPeriod::Monthly, 5.0, 500);

        let snap = store.snapshot();
        let usage = snap.get("openai").unwrap();
        assert!(
            (usage.cost_usd - 30.0).abs() < 1e-9,
            "expected 30.0, got {}",
            usage.cost_usd
        );
        assert_eq!(usage.tokens, 3_500);
    }

    #[test]
    fn test_check_uses_highest_utilisation_fraction() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0), // cost at 50% → Ok threshold
            max_tokens: Some(1_000),   // tokens at 90% → Warning
            ..Default::default()
        };
        store.record("openai", &cfg.period, 50.0, 900);
        match store.check("openai", &cfg) {
            QuotaCheckResult::Warning(pct) => {
                assert!(
                    pct >= 0.9,
                    "should reflect token utilisation (0.9+), got {pct}"
                );
            }
            other => panic!("expected Warning, got {other:?}"),
        }
    }

    #[test]
    fn test_multiple_providers_tracked_independently() {
        let tmp = TempDir::new().unwrap();
        let store = store_in_tmpdir(&tmp);
        let cfg = QuotaConfig {
            max_cost_usd: Some(100.0),
            max_tokens: None,
            ..Default::default()
        };

        store.record("anthropic", &cfg.period, 5.0, 0);
        store.record("openai", &cfg.period, 95.0, 0);

        assert_eq!(store.check("anthropic", &cfg), QuotaCheckResult::Ok);
        match store.check("openai", &cfg) {
            QuotaCheckResult::Warning(_) => {} // 95% → Warning
            other => panic!("expected Warning for openai, got {other:?}"),
        }
    }
}
