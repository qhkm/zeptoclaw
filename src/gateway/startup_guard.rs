//! Gateway startup guard — degrades to minimal mode after consecutive crashes.
//!
//! Persists crash state to `~/.zeptoclaw/crash_guard.json`. When the gateway
//! experiences N consecutive crashes within a time window, the guard signals
//! degraded mode so the gateway can disable dangerous tools.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{Result, ZeptoError};

/// Persisted crash state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrashState {
    /// Number of consecutive crashes (reset on clean start or stale window).
    pub consecutive_crashes: u32,
    /// Unix timestamp of the last crash.
    pub last_crash_ts: u64,
    /// Lifetime total crashes (never reset).
    pub total_crashes: u32,
}

/// Tracks gateway crashes and determines whether to enter degraded mode.
#[derive(Debug, Clone)]
pub struct StartupGuard {
    path: PathBuf,
    threshold: u32,
    window_secs: u64,
}

impl StartupGuard {
    /// Create a guard using the default path `~/.zeptoclaw/crash_guard.json`.
    pub fn new(threshold: u32, window_secs: u64) -> Self {
        let path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
            .join("crash_guard.json");
        Self::with_path(path, threshold, window_secs)
    }

    /// Create a guard with an explicit path (useful for testing).
    pub fn with_path(path: PathBuf, threshold: u32, window_secs: u64) -> Self {
        Self {
            path,
            threshold,
            window_secs,
        }
    }

    /// Returns `true` if degraded mode should be active.
    ///
    /// Degraded mode triggers when `consecutive_crashes >= threshold` and the
    /// last crash is within `window_secs` of now. A threshold of 0 always
    /// returns `false` (treated as disabled).
    pub fn check(&self) -> Result<bool> {
        if self.threshold == 0 {
            return Ok(false);
        }
        let state = self.load_state()?;
        if state.consecutive_crashes < self.threshold {
            debug!(
                consecutive = state.consecutive_crashes,
                threshold = self.threshold,
                "Startup guard: below threshold"
            );
            return Ok(false);
        }
        let now = now_secs();
        let stale = now.saturating_sub(state.last_crash_ts) > self.window_secs;
        if stale {
            debug!("Startup guard: last crash outside window, not degraded");
            return Ok(false);
        }
        Ok(true)
    }

    /// Record a crash. Resets `consecutive_crashes` if the previous crash is
    /// outside the time window (stale).
    pub fn record_crash(&self) -> Result<CrashState> {
        let mut state = self.load_state()?;
        let now = now_secs();

        // Reset if previous crash is stale
        if state.last_crash_ts > 0 && now.saturating_sub(state.last_crash_ts) > self.window_secs {
            debug!("Startup guard: previous crash stale, resetting consecutive count");
            state.consecutive_crashes = 0;
        }

        state.consecutive_crashes += 1;
        state.total_crashes += 1;
        state.last_crash_ts = now;
        self.save_state(&state)?;

        warn!(
            consecutive = state.consecutive_crashes,
            total = state.total_crashes,
            threshold = self.threshold,
            "Startup guard: recorded gateway crash"
        );
        Ok(state)
    }

    /// Record a clean start — resets consecutive crash counter.
    pub fn record_clean_start(&self) -> Result<()> {
        let mut state = self.load_state()?;
        if state.consecutive_crashes > 0 {
            info!(
                previous_consecutive = state.consecutive_crashes,
                "Startup guard: clean start, resetting crash counter"
            );
        }
        state.consecutive_crashes = 0;
        state.last_crash_ts = 0;
        self.save_state(&state)?;
        Ok(())
    }

    /// Load state from disk. Returns default state if the file is absent or
    /// contains malformed JSON.
    pub fn load_state(&self) -> Result<CrashState> {
        if !self.path.exists() {
            return Ok(CrashState::default());
        }
        let content = std::fs::read_to_string(&self.path).map_err(ZeptoError::Io)?;
        Ok(serde_json::from_str(&content).unwrap_or_default())
    }

    /// Save state to disk atomically (write temp file + rename).
    pub fn save_state(&self, state: &CrashState) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(ZeptoError::Io)?;
        }
        let tmp = self.path.with_extension("tmp");
        let json = serde_json::to_string_pretty(state)?;
        std::fs::write(&tmp, json).map_err(ZeptoError::Io)?;
        std::fs::rename(&tmp, &self.path).map_err(ZeptoError::Io)?;
        Ok(())
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn tmp_guard(threshold: u32, window_secs: u64) -> (StartupGuard, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash_guard.json");
        (StartupGuard::with_path(path, threshold, window_secs), dir)
    }

    #[test]
    fn test_default_state_not_degraded() {
        let (g, _d) = tmp_guard(4, 300);
        assert!(!g.check().unwrap());
    }

    #[test]
    fn test_below_threshold_not_degraded() {
        let (g, _d) = tmp_guard(4, 300);
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        assert!(!g.check().unwrap()); // 3 < 4
    }

    #[test]
    fn test_at_threshold_degraded() {
        let (g, _d) = tmp_guard(4, 300);
        for _ in 0..4 {
            g.record_crash().unwrap();
        }
        assert!(g.check().unwrap()); // 4 >= 4, within window
    }

    #[test]
    fn test_above_threshold_degraded() {
        let (g, _d) = tmp_guard(3, 300);
        for _ in 0..5 {
            g.record_crash().unwrap();
        }
        assert!(g.check().unwrap());
    }

    #[test]
    fn test_stale_crash_not_degraded() {
        let (g, _d) = tmp_guard(2, 300);
        // Manually write a state with old timestamp
        let state = CrashState {
            consecutive_crashes: 5,
            last_crash_ts: 1000, // long ago
            total_crashes: 5,
        };
        g.save_state(&state).unwrap();
        assert!(!g.check().unwrap()); // outside window
    }

    #[test]
    fn test_record_crash_resets_stale() {
        let (g, _d) = tmp_guard(3, 1);
        // Record two crashes
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        // Wait for window to expire
        thread::sleep(std::time::Duration::from_secs(2));
        // Next crash should reset consecutive to 1 (stale reset + increment)
        let state = g.record_crash().unwrap();
        assert_eq!(state.consecutive_crashes, 1);
        assert_eq!(state.total_crashes, 3); // lifetime still accumulates
    }

    #[test]
    fn test_clean_start_resets() {
        let (g, _d) = tmp_guard(2, 300);
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        assert!(g.check().unwrap());
        g.record_clean_start().unwrap();
        assert!(!g.check().unwrap());
        let state = g.load_state().unwrap();
        assert_eq!(state.consecutive_crashes, 0);
        assert_eq!(state.last_crash_ts, 0);
    }

    #[test]
    fn test_total_survives_clean_start() {
        let (g, _d) = tmp_guard(4, 300);
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        g.record_clean_start().unwrap();
        g.record_crash().unwrap();
        let state = g.load_state().unwrap();
        assert_eq!(state.consecutive_crashes, 1);
        assert_eq!(state.total_crashes, 3);
    }

    #[test]
    fn test_threshold_zero_always_false() {
        let (g, _d) = tmp_guard(0, 300);
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        assert!(!g.check().unwrap()); // threshold 0 = disabled
    }

    #[test]
    fn test_threshold_one() {
        let (g, _d) = tmp_guard(1, 300);
        assert!(!g.check().unwrap()); // no crashes yet
        g.record_crash().unwrap();
        assert!(g.check().unwrap()); // single crash triggers
    }

    #[test]
    fn test_missing_file_returns_default() {
        let (g, _d) = tmp_guard(4, 300);
        let state = g.load_state().unwrap();
        assert_eq!(state.consecutive_crashes, 0);
        assert_eq!(state.total_crashes, 0);
    }

    #[test]
    fn test_malformed_json_returns_default() {
        let (g, _d) = tmp_guard(4, 300);
        std::fs::write(&g.path, "not valid json {{{").unwrap();
        let state = g.load_state().unwrap();
        assert_eq!(state.consecutive_crashes, 0); // fallback to default
    }

    #[test]
    fn test_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("deep")
            .join("crash_guard.json");
        let g = StartupGuard::with_path(path.clone(), 4, 300);
        g.record_crash().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_roundtrip_persistence() {
        let (g, _d) = tmp_guard(4, 300);
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        let state = g.load_state().unwrap();
        assert_eq!(state.consecutive_crashes, 2);
        assert_eq!(state.total_crashes, 2);
        assert!(state.last_crash_ts > 0);
    }

    #[test]
    fn test_custom_window() {
        let (g, _d) = tmp_guard(2, 1); // 1 second window
        g.record_crash().unwrap();
        g.record_crash().unwrap();
        assert!(g.check().unwrap()); // within window
        thread::sleep(std::time::Duration::from_secs(2));
        assert!(!g.check().unwrap()); // outside window
    }

    #[test]
    fn test_clone_shares_path() {
        let (g, _d) = tmp_guard(2, 300);
        let g2 = g.clone();
        g.record_crash().unwrap();
        let state = g2.load_state().unwrap();
        assert_eq!(state.consecutive_crashes, 1);
    }
}
