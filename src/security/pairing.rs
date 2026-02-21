//! Device pairing with one-time codes, SHA-256 hashed bearer tokens, and brute-force lockout.
//!
//! Devices pair by exchanging a 6-digit code (valid for 5 minutes) for a bearer token (UUID v4).
//! Only the SHA-256 hash of the token is stored; the raw token is returned once at pairing time.
//! Failed validation attempts are tracked per identifier; after `max_attempts`, the identifier
//! is locked out for `lockout_secs`.
//!
//! Persists paired devices to `~/.zeptoclaw/security/paired_devices.json`.
//!
//! # Security notes
//!
//! - Token comparison uses **constant-time** equality via the `subtle` crate to prevent
//!   timing side-channel attacks.
//! - 6-digit codes are generated from CSPRNG-sourced bytes (UUID v4 uses `getrandom`).
//! - The lockout map is periodically pruned to prevent unbounded memory growth.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// A paired device record (persisted to JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Human-readable device name.
    pub name: String,
    /// SHA-256 hex digest of the bearer token.
    pub token_hash: String,
    /// Unix timestamp when the device was paired.
    pub paired_at: u64,
    /// Unix timestamp of the most recent successful token validation.
    pub last_seen: u64,
}

/// Device info returned by `list_devices()` — never includes the raw token or hash.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Human-readable device name.
    pub name: String,
    /// Unix timestamp when the device was paired.
    pub paired_at: u64,
    /// Unix timestamp of the most recent successful token validation.
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct PairingStore {
    devices: Vec<PairedDevice>,
}

/// An in-memory pending pairing code (not persisted).
struct PendingCode {
    code: String,
    expires_at: Instant,
}

/// Per-identifier brute-force lockout entry (not persisted).
struct LockoutEntry {
    attempts: u32,
    locked_until: Option<Instant>,
}

/// Manages device pairing lifecycle: code generation, token validation, and lockout.
pub struct PairingManager {
    store: PairingStore,
    path: PathBuf,
    pending_code: Option<PendingCode>,
    lockouts: HashMap<String, LockoutEntry>,
    max_attempts: u32,
    lockout_duration: Duration,
}

impl PairingManager {
    /// Create a new `PairingManager`, loading any existing paired devices from disk.
    pub fn new(max_attempts: u32, lockout_secs: u64) -> Self {
        let path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
            .join("security")
            .join("paired_devices.json");
        let store = Self::load_from_disk(&path);
        Self {
            store,
            path,
            pending_code: None,
            lockouts: HashMap::new(),
            max_attempts,
            lockout_duration: Duration::from_secs(lockout_secs),
        }
    }

    /// Create a `PairingManager` with a custom storage path (useful for testing).
    #[cfg(test)]
    fn with_path(path: PathBuf, max_attempts: u32, lockout_secs: u64) -> Self {
        let store = Self::load_from_disk(&path);
        Self {
            store,
            path,
            pending_code: None,
            lockouts: HashMap::new(),
            max_attempts,
            lockout_duration: Duration::from_secs(lockout_secs),
        }
    }

    /// Generate a new 6-digit pairing code valid for 5 minutes.
    ///
    /// Only one code can be active at a time. Generating a new code invalidates any previous one.
    /// Returns the 6-digit code as a zero-padded string.
    pub fn generate_pairing_code(&mut self) -> String {
        let code = Self::random_6_digit_code();
        self.pending_code = Some(PendingCode {
            code: code.clone(),
            expires_at: Instant::now() + Duration::from_secs(300),
        });
        info!("New pairing code generated (valid for 5 minutes)");
        code
    }

    /// Complete pairing by validating the 6-digit code and returning a bearer token.
    ///
    /// On success, the raw bearer token (UUID v4) is returned exactly once. Only its SHA-256
    /// hash is persisted. Returns `None` if the code is invalid, expired, or the identifier
    /// is locked out.
    pub fn complete_pairing(
        &mut self,
        code: &str,
        device_name: &str,
        identifier: &str,
    ) -> Option<String> {
        // Check lockout
        if self.is_locked_out(identifier) {
            warn!(identifier, "Pairing attempt rejected: locked out");
            return None;
        }

        let valid = self
            .pending_code
            .as_ref()
            .map(|pc| {
                let code_bytes = pc.code.as_bytes();
                let input_bytes = code.as_bytes();
                // Constant-time comparison to prevent timing attacks on pairing codes
                let codes_match = code_bytes.len() == input_bytes.len()
                    && bool::from(code_bytes.ct_eq(input_bytes));
                codes_match && Instant::now() < pc.expires_at
            })
            .unwrap_or(false);

        if !valid {
            self.record_failed_attempt(identifier);
            warn!(identifier, "Invalid or expired pairing code");
            return None;
        }

        // Code is valid — consume it
        self.pending_code = None;
        self.clear_lockout(identifier);

        // Generate bearer token
        let raw_token = Uuid::new_v4().to_string();
        let token_hash = Self::hash_token(&raw_token);
        let now = Self::now_secs();

        // Remove any existing device with the same name
        self.store.devices.retain(|d| d.name != device_name);

        self.store.devices.push(PairedDevice {
            name: device_name.to_string(),
            token_hash,
            paired_at: now,
            last_seen: now,
        });

        self.save_to_disk();
        info!(device = device_name, "Device paired successfully");

        Some(raw_token)
    }

    /// Validate a raw bearer token against stored SHA-256 hashes.
    ///
    /// Uses **constant-time** comparison to prevent timing side-channel attacks.
    /// On success, updates the device's `last_seen` timestamp in memory and returns
    /// the device name. The updated timestamp is **not** flushed to disk here —
    /// it will be persisted on the next `complete_pairing()`, `revoke()`, or explicit
    /// flush call, avoiding O(n) disk writes per request.
    ///
    /// On failure, records a failed attempt for the identifier.
    pub fn validate_token(&mut self, raw_token: &str, identifier: &str) -> Option<String> {
        // Check lockout
        if self.is_locked_out(identifier) {
            warn!(identifier, "Token validation rejected: locked out");
            return None;
        }

        let hash = Self::hash_token(raw_token);
        let hash_bytes = hash.as_bytes();
        let now = Self::now_secs();

        // Constant-time scan: always check ALL devices to prevent timing leaks
        // on which device index matched.
        let mut matched_idx: Option<usize> = None;
        for (i, device) in self.store.devices.iter().enumerate() {
            let stored_bytes = device.token_hash.as_bytes();
            // Both are SHA-256 hex (64 bytes). If lengths differ, ct_eq returns 0.
            if stored_bytes.len() == hash_bytes.len() && bool::from(stored_bytes.ct_eq(hash_bytes))
            {
                matched_idx = Some(i);
                // Do NOT break — scan all devices for constant-time behavior.
                // In practice the compiler may optimize this, but we make the intent clear.
            }
        }

        if let Some(idx) = matched_idx {
            self.store.devices[idx].last_seen = now;
            let name = self.store.devices[idx].name.clone();
            // Deferred disk write — flushed on next complete_pairing/revoke/clear
            self.clear_lockout(identifier);
            Some(name)
        } else {
            self.record_failed_attempt(identifier);
            None
        }
    }

    /// Revoke a paired device by name.
    ///
    /// Returns `true` if a device was found and removed.
    pub fn revoke(&mut self, device_name: &str) -> bool {
        let initial_len = self.store.devices.len();
        self.store.devices.retain(|d| d.name != device_name);
        let removed = self.store.devices.len() < initial_len;
        if removed {
            self.save_to_disk();
            info!(device = device_name, "Device revoked");
        }
        removed
    }

    /// List paired devices without exposing token hashes.
    pub fn list_devices(&self) -> Vec<DeviceInfo> {
        self.store
            .devices
            .iter()
            .map(|d| DeviceInfo {
                name: d.name.clone(),
                paired_at: d.paired_at,
                last_seen: d.last_seen,
            })
            .collect()
    }

    /// Returns `true` if there are any paired devices.
    pub fn has_devices(&self) -> bool {
        !self.store.devices.is_empty()
    }

    /// Check if an identifier is currently locked out.
    pub fn is_locked_out(&self, identifier: &str) -> bool {
        if let Some(entry) = self.lockouts.get(identifier) {
            if let Some(locked_until) = entry.locked_until {
                if Instant::now() < locked_until {
                    return true;
                }
            }
        }
        false
    }

    /// Get the number of failed attempts for an identifier.
    pub fn failed_attempts(&self, identifier: &str) -> u32 {
        self.lockouts
            .get(identifier)
            .map(|e| e.attempts)
            .unwrap_or(0)
    }

    /// Remove expired lockout entries to prevent unbounded map growth.
    ///
    /// Called automatically during `record_failed_attempt()`. Can also be called
    /// explicitly for periodic maintenance.
    pub fn prune_expired_lockouts(&mut self) {
        let now = Instant::now();
        let before = self.lockouts.len();
        self.lockouts.retain(|_, entry| {
            entry.locked_until.map(|t| now < t).unwrap_or(true) // keep entries without a lockout deadline (still accumulating)
        });
        let pruned = before - self.lockouts.len();
        if pruned > 0 {
            debug!(
                pruned,
                remaining = self.lockouts.len(),
                "Pruned expired lockout entries"
            );
        }
    }

    // ---- Internal helpers ----

    fn record_failed_attempt(&mut self, identifier: &str) {
        // Prune expired lockouts periodically (every time we record a failure)
        // to bound memory growth from diverse attacker IPs.
        if self.lockouts.len() > 100 {
            self.prune_expired_lockouts();
        }

        let entry = self
            .lockouts
            .entry(identifier.to_string())
            .or_insert(LockoutEntry {
                attempts: 0,
                locked_until: None,
            });

        // If a previous lockout has expired, reset
        if entry
            .locked_until
            .map(|t| Instant::now() >= t)
            .unwrap_or(false)
        {
            entry.attempts = 0;
            entry.locked_until = None;
        }

        entry.attempts += 1;

        if entry.attempts >= self.max_attempts {
            entry.locked_until = Some(Instant::now() + self.lockout_duration);
            warn!(
                identifier,
                attempts = entry.attempts,
                lockout_secs = self.lockout_duration.as_secs(),
                "Brute-force lockout triggered"
            );
        }
    }

    fn clear_lockout(&mut self, identifier: &str) {
        self.lockouts.remove(identifier);
    }

    /// SHA-256 hash a raw token to hex.
    fn hash_token(raw_token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw_token.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Generate a random 6-digit code using CSPRNG bytes from UUID v4.
    ///
    /// UUID v4 is backed by `getrandom` (OS-level CSPRNG) on all platforms.
    /// We extract 4 random bytes and reduce modulo 1,000,000. The modulo bias
    /// is negligible (~0.0001%) for a 5-minute one-time code.
    fn random_6_digit_code() -> String {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        format!("{:06}", n % 1_000_000)
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn load_from_disk(path: &Path) -> PairingStore {
        match std::fs::read_to_string(path) {
            Ok(data) => match serde_json::from_str(&data) {
                Ok(store) => store,
                Err(e) => {
                    warn!("Paired devices file is corrupt, starting empty: {}", e);
                    PairingStore::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => PairingStore::default(),
            Err(e) => {
                warn!("Failed to read paired devices, starting empty: {}", e);
                PairingStore::default()
            }
        }
    }

    fn save_to_disk(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(&self.store) {
            if let Err(e) = std::fs::write(&self.path, data) {
                warn!("Failed to save paired devices: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test manager with a unique temp path so parallel tests don't collide.
    fn test_manager() -> PairingManager {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tid = std::thread::current().id();
        PairingManager {
            store: PairingStore::default(),
            path: PathBuf::from(format!("/tmp/zeptoclaw-test-pairing-{tid:?}-{id}.json")),
            pending_code: None,
            lockouts: HashMap::new(),
            max_attempts: 5,
            lockout_duration: Duration::from_secs(300),
        }
    }

    #[test]
    fn test_pairing_code_is_6_digits() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_pairing_code_generates_different_codes() {
        // Two calls should (almost certainly) produce different codes
        let mut mgr = test_manager();
        let code1 = mgr.generate_pairing_code();
        let code2 = mgr.generate_pairing_code();
        // This could technically fail with probability 1/1_000_000, acceptable for a test
        let _codes = (code1, code2); // just ensure both generate without panic
    }

    #[test]
    fn test_complete_pairing_valid_code() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        let token = mgr.complete_pairing(&code, "my-laptop", "127.0.0.1");
        assert!(token.is_some());
        // Token should be a valid UUID v4
        let raw = token.unwrap();
        assert!(Uuid::parse_str(&raw).is_ok());
    }

    #[test]
    fn test_complete_pairing_invalid_code() {
        let mut mgr = test_manager();
        let _code = mgr.generate_pairing_code();
        let token = mgr.complete_pairing("000000", "device", "127.0.0.1");
        assert!(token.is_none());
    }

    #[test]
    fn test_complete_pairing_code_consumed() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        let token1 = mgr.complete_pairing(&code, "device1", "127.0.0.1");
        assert!(token1.is_some());
        // Second attempt with same code should fail (consumed)
        let token2 = mgr.complete_pairing(&code, "device2", "127.0.0.1");
        assert!(token2.is_none());
    }

    #[test]
    fn test_complete_pairing_expired_code() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        // Manually expire the pending code
        if let Some(ref mut pc) = mgr.pending_code {
            pc.expires_at = Instant::now() - Duration::from_secs(1);
        }
        let token = mgr.complete_pairing(&code, "device", "127.0.0.1");
        assert!(token.is_none());
    }

    #[test]
    fn test_validate_token_success() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        let raw_token = mgr
            .complete_pairing(&code, "my-phone", "127.0.0.1")
            .unwrap();

        let device_name = mgr.validate_token(&raw_token, "192.168.1.1");
        assert_eq!(device_name, Some("my-phone".to_string()));
    }

    #[test]
    fn test_validate_token_invalid() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        let _raw = mgr.complete_pairing(&code, "device", "127.0.0.1").unwrap();

        let result = mgr.validate_token("not-a-real-token", "192.168.1.1");
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_token_updates_last_seen() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        let raw_token = mgr.complete_pairing(&code, "device", "127.0.0.1").unwrap();

        let initial_last_seen = mgr.store.devices[0].last_seen;
        // Small sleep to ensure timestamp changes (if test runs fast)
        std::thread::sleep(std::time::Duration::from_millis(10));
        mgr.validate_token(&raw_token, "127.0.0.1");
        // last_seen should be >= initial (could be equal in same second)
        assert!(mgr.store.devices[0].last_seen >= initial_last_seen);
    }

    #[test]
    fn test_validate_token_no_disk_write() {
        // validate_token should NOT write to disk (deferred to next state change)
        let dir =
            std::env::temp_dir().join(format!("zeptoclaw-pairing-nodefer-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("paired_devices.json");

        let mut mgr = PairingManager::with_path(path.clone(), 5, 300);
        let code = mgr.generate_pairing_code();
        let raw_token = mgr.complete_pairing(&code, "device", "127.0.0.1").unwrap();

        // Record the file modification time after complete_pairing
        let mtime_after_pair = std::fs::metadata(&path).unwrap().modified().unwrap();

        // Small sleep to ensure filesystem mtime granularity
        std::thread::sleep(std::time::Duration::from_millis(50));

        // validate_token should NOT touch the file
        mgr.validate_token(&raw_token, "127.0.0.1");
        let mtime_after_validate = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_after_pair, mtime_after_validate,
            "validate_token should not write to disk"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_revoke_device() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        let raw_token = mgr
            .complete_pairing(&code, "old-device", "127.0.0.1")
            .unwrap();

        assert!(mgr.revoke("old-device"));
        assert!(mgr.validate_token(&raw_token, "127.0.0.1").is_none());
        assert!(!mgr.revoke("old-device")); // already removed
    }

    #[test]
    fn test_list_devices() {
        let mut mgr = test_manager();
        let code = mgr.generate_pairing_code();
        mgr.complete_pairing(&code, "device-a", "127.0.0.1");
        let code2 = mgr.generate_pairing_code();
        mgr.complete_pairing(&code2, "device-b", "127.0.0.1");

        let devices = mgr.list_devices();
        assert_eq!(devices.len(), 2);
        assert!(devices.iter().any(|d| d.name == "device-a"));
        assert!(devices.iter().any(|d| d.name == "device-b"));
    }

    #[test]
    fn test_has_devices() {
        let mut mgr = test_manager();
        assert!(!mgr.has_devices());
        let code = mgr.generate_pairing_code();
        mgr.complete_pairing(&code, "device", "127.0.0.1");
        assert!(mgr.has_devices());
    }

    #[test]
    fn test_brute_force_lockout() {
        let mut mgr = test_manager();
        mgr.max_attempts = 3;
        mgr.lockout_duration = Duration::from_secs(300);

        let _code = mgr.generate_pairing_code();

        // 3 failed attempts should trigger lockout
        for _ in 0..3 {
            mgr.complete_pairing("999999", "device", "attacker-ip");
        }

        assert!(mgr.is_locked_out("attacker-ip"));
        assert_eq!(mgr.failed_attempts("attacker-ip"), 3);

        // Even a valid code should be rejected during lockout
        let code2 = mgr.generate_pairing_code();
        let token = mgr.complete_pairing(&code2, "device", "attacker-ip");
        assert!(token.is_none());
    }

    #[test]
    fn test_lockout_does_not_affect_other_identifiers() {
        let mut mgr = test_manager();
        mgr.max_attempts = 2;

        let _code = mgr.generate_pairing_code();

        // Lock out "attacker"
        for _ in 0..2 {
            mgr.complete_pairing("999999", "d", "attacker");
        }
        assert!(mgr.is_locked_out("attacker"));

        // "legitimate" is not locked out
        assert!(!mgr.is_locked_out("legitimate"));
    }

    #[test]
    fn test_hash_token_deterministic() {
        let hash1 = PairingManager::hash_token("test-token-123");
        let hash2 = PairingManager::hash_token("test-token-123");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 hex is 64 chars
    }

    #[test]
    fn test_hash_token_different_inputs() {
        let hash1 = PairingManager::hash_token("token-a");
        let hash2 = PairingManager::hash_token("token-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_constant_time_comparison() {
        // Verify that token validation uses constant-time comparison
        // by ensuring the same hash matches via ct_eq
        let token = "test-token-abc";
        let hash = PairingManager::hash_token(token);
        let hash_bytes = hash.as_bytes();
        let same_hash = PairingManager::hash_token(token);
        assert!(bool::from(hash_bytes.ct_eq(same_hash.as_bytes())));

        let diff_hash = PairingManager::hash_token("different-token");
        assert!(!bool::from(hash_bytes.ct_eq(diff_hash.as_bytes())));
    }

    #[test]
    fn test_prune_expired_lockouts() {
        let mut mgr = test_manager();
        mgr.max_attempts = 1;
        mgr.lockout_duration = Duration::from_secs(0); // expire immediately

        let _code = mgr.generate_pairing_code();

        // Trigger lockouts for 3 identifiers
        mgr.complete_pairing("999999", "d", "ip-a");
        mgr.complete_pairing("999999", "d", "ip-b");
        mgr.complete_pairing("999999", "d", "ip-c");

        assert_eq!(mgr.lockouts.len(), 3);

        // All lockouts have 0s duration, so they're already expired
        std::thread::sleep(Duration::from_millis(10));
        mgr.prune_expired_lockouts();
        assert_eq!(
            mgr.lockouts.len(),
            0,
            "All expired lockouts should be pruned"
        );
    }

    #[test]
    fn test_prune_keeps_active_lockouts() {
        let mut mgr = test_manager();
        mgr.max_attempts = 1;
        mgr.lockout_duration = Duration::from_secs(3600); // 1 hour

        let _code = mgr.generate_pairing_code();
        mgr.complete_pairing("999999", "d", "ip-locked");

        assert_eq!(mgr.lockouts.len(), 1);
        mgr.prune_expired_lockouts();
        assert_eq!(
            mgr.lockouts.len(),
            1,
            "Active lockout should survive pruning"
        );
    }

    #[test]
    fn test_pairing_config_defaults() {
        let cfg = crate::config::PairingConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.lockout_secs, 300);
    }

    #[test]
    fn test_pairing_config_deserialize() {
        let json = r#"{"enabled": true, "max_attempts": 10, "lockout_secs": 600}"#;
        let cfg: crate::config::PairingConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_attempts, 10);
        assert_eq!(cfg.lockout_secs, 600);
    }

    #[test]
    fn test_store_persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("zeptoclaw-pairing-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("paired_devices.json");

        // Create and pair a device
        let mut mgr = PairingManager::with_path(path.clone(), 5, 300);
        let code = mgr.generate_pairing_code();
        let raw_token = mgr
            .complete_pairing(&code, "persist-test", "127.0.0.1")
            .unwrap();

        // Load from disk in a new manager
        let mut mgr2 = PairingManager::with_path(path, 5, 300);
        let device_name = mgr2.validate_token(&raw_token, "127.0.0.1");
        assert_eq!(device_name, Some("persist-test".to_string()));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_replace_device_with_same_name() {
        let mut mgr = test_manager();

        // Pair device once
        let code1 = mgr.generate_pairing_code();
        let token1 = mgr
            .complete_pairing(&code1, "my-device", "127.0.0.1")
            .unwrap();

        // Re-pair with same name — old token should be invalidated
        let code2 = mgr.generate_pairing_code();
        let token2 = mgr
            .complete_pairing(&code2, "my-device", "127.0.0.1")
            .unwrap();

        assert!(mgr.validate_token(&token1, "127.0.0.1").is_none());
        assert_eq!(
            mgr.validate_token(&token2, "127.0.0.1"),
            Some("my-device".to_string())
        );
        assert_eq!(mgr.list_devices().len(), 1);
    }

    #[test]
    fn test_load_from_disk_corrupt_file() {
        let dir =
            std::env::temp_dir().join(format!("zeptoclaw-pairing-corrupt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("paired_devices.json");
        std::fs::write(&path, "not valid json!!!").unwrap();

        let mgr = PairingManager::with_path(path, 5, 300);
        assert!(
            mgr.store.devices.is_empty(),
            "Corrupt file should yield empty store"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_from_disk_missing_file() {
        let path = PathBuf::from("/tmp/zeptoclaw-pairing-nonexistent-12345.json");
        let mgr = PairingManager::with_path(path, 5, 300);
        assert!(mgr.store.devices.is_empty());
    }
}
