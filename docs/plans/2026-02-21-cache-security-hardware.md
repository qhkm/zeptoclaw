# Cache, Security & Hardware Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add response cache, agent modes, device pairing, and full hardware/peripheral stack to ZeptoClaw.

**Architecture:** Four independent modules. Response cache sits in the provider call path. Agent modes + device pairing add security layers to the agent loop and gateway. Hardware is fully feature-gated. All follow existing patterns: trait-based abstraction, JSON persistence, config structs with env overrides.

**Tech Stack:** Rust, tokio, serde_json, sha2 (already in dep tree), nusb + tokio-serial + rppal + probe-rs (all feature-gated)

**Design doc:** `docs/plans/2026-02-21-cache-security-hardware-design.md`

---

## Task 1: Response Cache

**Files:**
- Create: `src/cache/mod.rs`
- Create: `src/cache/response_cache.rs`
- Modify: `src/config/types.rs`
- Modify: `src/config/mod.rs`
- Modify: `src/agent/loop.rs`
- Modify: `src/lib.rs`

**Context:** JSON-file-backed LLM response cache at `~/.zeptoclaw/cache/responses.json`. Cache key is SHA-256 of `(model, system_prompt_hash, user_prompt)`. TTL + LRU eviction. Checked in agent loop before calling provider.

---

**Step 1: Add `CacheConfig` to `src/config/types.rs`**

Find the `Config` struct. Add a `cache` field. Add the config struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    pub ttl_secs: u64,
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_secs: 3600,
            max_entries: 500,
        }
    }
}
```

Add to `Config` struct:
```rust
pub cache: CacheConfig,
```

Add to `KNOWN_TOP_LEVEL` in `src/config/mod.rs` (the validate function) if it exists.

Add env overrides in `src/config/mod.rs`:
```rust
fn apply_cache_env_overrides(&mut self) {
    if let Ok(val) = std::env::var("ZEPTOCLAW_CACHE_ENABLED") {
        self.cache.enabled = val.eq_ignore_ascii_case("true") || val == "1";
    }
    if let Ok(val) = std::env::var("ZEPTOCLAW_CACHE_TTL_SECS") {
        if let Ok(n) = val.parse() { self.cache.ttl_secs = n; }
    }
    if let Ok(val) = std::env::var("ZEPTOCLAW_CACHE_MAX_ENTRIES") {
        if let Ok(n) = val.parse() { self.cache.max_entries = n; }
    }
}
```

Call `apply_cache_env_overrides()` from the master `apply_env_overrides()`.

**Step 2: Create `src/cache/response_cache.rs`**

```rust
//! LLM response cache with TTL expiry and LRU eviction.
//! Persists to ~/.zeptoclaw/cache/responses.json.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub response: String,
    pub token_count: u32,
    pub created_at: u64,     // unix timestamp
    pub accessed_at: u64,    // unix timestamp
    pub hit_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheStore {
    entries: HashMap<String, CacheEntry>,
}

pub struct ResponseCache {
    store: CacheStore,
    path: PathBuf,
    ttl_secs: u64,
    max_entries: usize,
}

impl ResponseCache {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        let path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
            .join("cache")
            .join("responses.json");
        let store = Self::load_from_disk(&path);
        Self { store, path, ttl_secs, max_entries }
    }

    /// Build cache key: SHA-256 of (model, system_prompt_hash, user_prompt)
    pub fn cache_key(model: &str, system_prompt: &str, user_prompt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(model.as_bytes());
        hasher.update(b"|");
        hasher.update(system_prompt.as_bytes());
        hasher.update(b"|");
        hasher.update(user_prompt.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Get cached response if valid (not expired)
    pub fn get(&mut self, key: &str) -> Option<String> {
        let now = Self::now_secs();
        if let Some(entry) = self.store.entries.get_mut(key) {
            if now - entry.created_at > self.ttl_secs {
                self.store.entries.remove(key);
                self.save_to_disk();
                return None;
            }
            entry.accessed_at = now;
            entry.hit_count += 1;
            let response = entry.response.clone();
            self.save_to_disk();
            return Some(response);
        }
        None
    }

    /// Store a response in the cache
    pub fn put(&mut self, key: String, response: String, token_count: u32) {
        let now = Self::now_secs();
        // Evict expired entries first
        self.evict_expired(now);
        // LRU eviction if at capacity
        while self.store.entries.len() >= self.max_entries {
            self.evict_lru();
        }
        self.store.entries.insert(key, CacheEntry {
            response,
            token_count,
            created_at: now,
            accessed_at: now,
            hit_count: 0,
        });
        self.save_to_disk();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let total_hits: u64 = self.store.entries.values().map(|e| e.hit_count as u64).sum();
        let total_tokens_saved: u64 = self.store.entries.values()
            .map(|e| e.hit_count as u64 * e.token_count as u64)
            .sum();
        CacheStats {
            total_entries: self.store.entries.len(),
            total_hits,
            total_tokens_saved,
        }
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.store.entries.clear();
        self.save_to_disk();
    }

    fn evict_expired(&mut self, now: u64) {
        self.store.entries.retain(|_, e| now - e.created_at <= self.ttl_secs);
    }

    fn evict_lru(&mut self) {
        if let Some(lru_key) = self.store.entries.iter()
            .min_by_key(|(_, e)| e.accessed_at)
            .map(|(k, _)| k.clone())
        {
            self.store.entries.remove(&lru_key);
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
    }

    fn load_from_disk(path: &PathBuf) -> CacheStore {
        match std::fs::read_to_string(path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => CacheStore::default(),
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

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: usize,
    pub total_hits: u64,
    pub total_tokens_saved: u64,
}
```

**Step 3: Create `src/cache/mod.rs`**

```rust
pub mod response_cache;
pub use response_cache::{ResponseCache, CacheStats};
```

**Step 4: Export from `src/lib.rs`**

Add:
```rust
pub mod cache;
```

**Step 5: Wire into agent loop**

In `src/agent/loop.rs`, in `AgentLoop::new()` or struct fields, add:
```rust
cache: Option<Arc<Mutex<ResponseCache>>>,
```

Initialize based on config:
```rust
let cache = if config.cache.enabled {
    Some(Arc::new(Mutex::new(ResponseCache::new(
        config.cache.ttl_secs,
        config.cache.max_entries,
    ))))
} else {
    None
};
```

Before calling the provider in the message loop, check cache:
```rust
if let Some(ref cache) = self.cache {
    let key = ResponseCache::cache_key(&model, &system_prompt_hash, &user_message);
    if let Some(cached) = cache.lock().await.get(&key) {
        debug!("Cache hit for prompt");
        // Return cached response instead of calling provider
        return Ok(cached);
    }
}
```

After getting provider response, store in cache:
```rust
if let Some(ref cache) = self.cache {
    let key = ResponseCache::cache_key(&model, &system_prompt_hash, &user_message);
    cache.lock().await.put(key, response.clone(), token_count);
}
```

Note: Study the actual agent loop to find the exact insertion points. The cache should only apply to the initial LLM call, NOT to tool-use follow-up calls. Look for where `provider.chat()` or `provider.chat_stream()` is called with the user's message.

**Step 6: Write tests in `src/cache/response_cache.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_cache() -> ResponseCache {
        ResponseCache {
            store: CacheStore::default(),
            path: PathBuf::from("/tmp/zeptoclaw-test-cache.json"),
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
        // Entry created with now(), ttl=0 means anything older than 0s is expired
        // Need to wait 1s or manipulate created_at
        if let Some(entry) = cache.store.entries.get_mut("key") {
            entry.created_at -= 1; // backdate 1 second
        }
        assert!(cache.get("key").is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut cache = test_cache(); // max 5 entries
        for i in 0..5 {
            cache.put(format!("k{i}"), format!("v{i}"), 10);
        }
        // Access k0 to make it recently used
        cache.get("k0");
        // Add k5 — should evict k1 (least recently accessed)
        cache.put("k5".into(), "v5".into(), 10);
        assert!(cache.get("k0").is_some()); // was accessed, survived
        assert!(cache.get("k1").is_none()); // LRU, evicted
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = test_cache();
        cache.put("k1".into(), "r1".into(), 100);
        cache.put("k2".into(), "r2".into(), 200);
        cache.get("k1"); // 1 hit
        cache.get("k1"); // 2 hits
        cache.get("k2"); // 1 hit
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
    }

    #[test]
    fn test_cache_hit_increments_count() {
        let mut cache = test_cache();
        cache.put("k".into(), "r".into(), 10);
        cache.get("k");
        cache.get("k");
        let entry = cache.store.entries.get("k").unwrap();
        assert_eq!(entry.hit_count, 2);
    }

    #[test]
    fn test_cache_config_defaults() {
        let cfg = CacheConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.ttl_secs, 3600);
        assert_eq!(cfg.max_entries, 500);
    }
}
```

**Step 7: Verify and commit**

```bash
cargo test --lib cache -- --nocapture 2>&1 | tail -10
cargo clippy -- -D warnings 2>&1 | tail -5
git add src/cache/ src/config/ src/agent/loop.rs src/lib.rs
git commit -m "feat(cache): add LLM response cache with TTL + LRU eviction"
```

---

## Task 2: Agent Modes (Category-Based)

**Files:**
- Create: `src/security/agent_mode.rs`
- Modify: `src/security/mod.rs`
- Modify: `src/config/types.rs`
- Modify: `src/config/mod.rs`
- Modify: `src/tools/types.rs`
- Modify: `src/agent/loop.rs`
- Modify: `src/cli/mod.rs`

**Context:** Three agent modes (Observer/Assistant/Autonomous) with category-based tool classification. Each tool gets a `ToolCategory` tag. Modes define which categories are allowed/require-approval/blocked. Checked in agent loop before `ApprovalGate`.

---

**Step 1: Add `ToolCategory` to `src/tools/types.rs`**

Add after the `Tool` trait:

```rust
/// Category for agent mode enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    FilesystemRead,
    FilesystemWrite,
    NetworkRead,
    NetworkWrite,
    Shell,
    Hardware,
    Memory,
    Messaging,
    Destructive,
}
```

Add a method to the `Tool` trait:
```rust
/// Tool category for agent mode enforcement. Defaults to FilesystemRead.
fn category(&self) -> ToolCategory {
    ToolCategory::FilesystemRead
}
```

**Step 2: Tag existing tools with categories**

Each tool's `impl Tool` needs a `category()` override. Study `src/tools/` and tag:
- `ReadFileTool`, `ListFileTool`, `GlobTool` → `FilesystemRead`
- `WriteFileTool`, `EditFileTool` → `FilesystemWrite`
- `WebSearchTool`, `WebFetchTool` → `NetworkRead`
- `HttpRequestTool` → `NetworkWrite`
- `ShellTool` → `Shell`
- `HardwareTool` (future) → `Hardware`
- `MemoryTool`, `LongTermMemoryTool` → `Memory`
- `MessageTool`, `WhatsAppTool` → `Messaging`
- `CronTool` (delete action) → `Destructive`

Note: Some tools have mixed categories. Use the MOST restrictive category for the tool as a whole. The `Shell` category is the most dangerous — it stays in its own category.

**Step 3: Create `src/security/agent_mode.rs`**

```rust
//! Agent mode enforcement — controls what categories of tools the agent can use.

use crate::tools::types::ToolCategory;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Observer,
    Assistant,
    Autonomous,
}

impl Default for AgentMode {
    fn default() -> Self { Self::Autonomous }
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Observer => write!(f, "observer"),
            Self::Assistant => write!(f, "assistant"),
            Self::Autonomous => write!(f, "autonomous"),
        }
    }
}

impl std::str::FromStr for AgentMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "observer" => Ok(Self::Observer),
            "assistant" => Ok(Self::Assistant),
            "autonomous" => Ok(Self::Autonomous),
            _ => Err(format!("unknown agent mode: '{}' (expected observer/assistant/autonomous)", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryPermission {
    Allowed,
    RequiresApproval,
    Blocked,
}

pub struct ModePolicy {
    mode: AgentMode,
}

impl ModePolicy {
    pub fn new(mode: AgentMode) -> Self { Self { mode } }

    /// Check what permission a tool category has under the current mode.
    pub fn check(&self, category: ToolCategory) -> CategoryPermission {
        match self.mode {
            AgentMode::Autonomous => CategoryPermission::Allowed,
            AgentMode::Observer => match category {
                ToolCategory::FilesystemRead
                | ToolCategory::NetworkRead
                | ToolCategory::Memory => CategoryPermission::Allowed,
                _ => CategoryPermission::Blocked,
            },
            AgentMode::Assistant => match category {
                ToolCategory::FilesystemRead
                | ToolCategory::FilesystemWrite
                | ToolCategory::NetworkRead
                | ToolCategory::NetworkWrite
                | ToolCategory::Memory
                | ToolCategory::Messaging => CategoryPermission::Allowed,
                ToolCategory::Shell
                | ToolCategory::Hardware
                | ToolCategory::Destructive => CategoryPermission::RequiresApproval,
            },
        }
    }

    /// Get all blocked categories for this mode.
    pub fn blocked_categories(&self) -> HashSet<ToolCategory> {
        use ToolCategory::*;
        let all = [FilesystemRead, FilesystemWrite, NetworkRead, NetworkWrite,
                    Shell, Hardware, Memory, Messaging, Destructive];
        all.into_iter()
            .filter(|c| self.check(*c) == CategoryPermission::Blocked)
            .collect()
    }
}
```

**Step 4: Add config**

In `src/config/types.rs`, add to the security-related section:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentModeConfig {
    pub mode: String,  // "observer", "assistant", "autonomous"
}

impl Default for AgentModeConfig {
    fn default() -> Self {
        Self { mode: "autonomous".into() }
    }
}
```

Add to `Config` struct (or `SecurityConfig` if it exists):
```rust
pub agent_mode: AgentModeConfig,
```

Add env override:
```rust
if let Ok(val) = std::env::var("ZEPTOCLAW_SECURITY_AGENT_MODE") {
    self.agent_mode.mode = val;
}
```

**Step 5: Wire into agent loop**

In `src/agent/loop.rs`, in the tool execution pipeline (around line 516), BEFORE the approval gate check:

```rust
// Agent mode enforcement (before approval gate)
let mode_policy = ModePolicy::new(agent_mode);
let tool_category = tool.category();
match mode_policy.check(tool_category) {
    CategoryPermission::Blocked => {
        return (id, format!(
            "Tool '{}' is blocked in {} mode (category: {:?})",
            name, agent_mode, tool_category
        ));
    }
    CategoryPermission::RequiresApproval => {
        // Fall through to approval gate — it will handle the prompt
        // But FORCE approval even if gate.requires_approval() returns false
        if !gate.requires_approval(&name) {
            return (id, format!(
                "Tool '{}' requires approval in {} mode (category: {:?}). Not executed.",
                name, agent_mode, tool_category
            ));
        }
    }
    CategoryPermission::Allowed => {} // proceed
}
```

**Step 6: Add `--mode` CLI flag**

In `src/cli/mod.rs`, add to the `Agent` command variant:
```rust
#[arg(long)]
mode: Option<String>,
```

Parse and pass through to agent creation.

**Step 7: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observer_allows_read() {
        let p = ModePolicy::new(AgentMode::Observer);
        assert_eq!(p.check(ToolCategory::FilesystemRead), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::NetworkRead), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::Memory), CategoryPermission::Allowed);
    }

    #[test]
    fn test_observer_blocks_write() {
        let p = ModePolicy::new(AgentMode::Observer);
        assert_eq!(p.check(ToolCategory::FilesystemWrite), CategoryPermission::Blocked);
        assert_eq!(p.check(ToolCategory::Shell), CategoryPermission::Blocked);
        assert_eq!(p.check(ToolCategory::Hardware), CategoryPermission::Blocked);
        assert_eq!(p.check(ToolCategory::Messaging), CategoryPermission::Blocked);
    }

    #[test]
    fn test_assistant_allows_readwrite() {
        let p = ModePolicy::new(AgentMode::Assistant);
        assert_eq!(p.check(ToolCategory::FilesystemRead), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::FilesystemWrite), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::NetworkWrite), CategoryPermission::Allowed);
    }

    #[test]
    fn test_assistant_requires_approval_for_dangerous() {
        let p = ModePolicy::new(AgentMode::Assistant);
        assert_eq!(p.check(ToolCategory::Shell), CategoryPermission::RequiresApproval);
        assert_eq!(p.check(ToolCategory::Hardware), CategoryPermission::RequiresApproval);
        assert_eq!(p.check(ToolCategory::Destructive), CategoryPermission::RequiresApproval);
    }

    #[test]
    fn test_autonomous_allows_all() {
        let p = ModePolicy::new(AgentMode::Autonomous);
        assert_eq!(p.check(ToolCategory::Shell), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::Hardware), CategoryPermission::Allowed);
        assert_eq!(p.check(ToolCategory::Destructive), CategoryPermission::Allowed);
    }

    #[test]
    fn test_parse_mode_from_string() {
        assert_eq!("observer".parse::<AgentMode>().unwrap(), AgentMode::Observer);
        assert_eq!("assistant".parse::<AgentMode>().unwrap(), AgentMode::Assistant);
        assert_eq!("autonomous".parse::<AgentMode>().unwrap(), AgentMode::Autonomous);
        assert_eq!("OBSERVER".parse::<AgentMode>().unwrap(), AgentMode::Observer);
        assert!("invalid".parse::<AgentMode>().is_err());
    }

    #[test]
    fn test_observer_blocked_categories() {
        let p = ModePolicy::new(AgentMode::Observer);
        let blocked = p.blocked_categories();
        assert!(blocked.contains(&ToolCategory::Shell));
        assert!(blocked.contains(&ToolCategory::FilesystemWrite));
        assert!(!blocked.contains(&ToolCategory::FilesystemRead));
    }

    #[test]
    fn test_default_mode_is_autonomous() {
        assert_eq!(AgentMode::default(), AgentMode::Autonomous);
    }

    #[test]
    fn test_mode_config_defaults() {
        let cfg = AgentModeConfig::default();
        assert_eq!(cfg.mode, "autonomous");
    }
}
```

**Step 8: Verify and commit**

```bash
cargo test --lib security::agent_mode -- --nocapture 2>&1 | tail -10
cargo clippy -- -D warnings 2>&1 | tail -5
git add src/security/ src/config/ src/tools/types.rs src/agent/loop.rs src/cli/mod.rs
git commit -m "feat(security): add agent modes (observer/assistant/autonomous) with category-based tool control"
```

---

## Task 3: Device Pairing

**Files:**
- Create: `src/security/pairing.rs`
- Modify: `src/security/mod.rs`
- Modify: `src/config/types.rs`
- Modify: `src/config/mod.rs`
- Modify: `src/gateway/container_agent.rs`
- Create: `src/cli/pair.rs`
- Modify: `src/cli/mod.rs`

**Context:** One-time 6-digit pairing codes → SHA-256 hashed bearer tokens. Brute-force lockout. JSON persistence at `~/.zeptoclaw/security/paired_devices.json`. Gateway middleware rejects unpaired requests when enabled.

---

**Step 1: Add `PairingConfig` to `src/config/types.rs`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PairingConfig {
    pub enabled: bool,
    pub max_attempts: u32,
    pub lockout_secs: u64,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_attempts: 5,
            lockout_secs: 300,
        }
    }
}
```

Add to Config:
```rust
pub pairing: PairingConfig,
```

Add env overrides:
```rust
fn apply_pairing_env_overrides(&mut self) {
    if let Ok(val) = std::env::var("ZEPTOCLAW_SECURITY_PAIRING_ENABLED") {
        self.pairing.enabled = val.eq_ignore_ascii_case("true") || val == "1";
    }
    if let Ok(val) = std::env::var("ZEPTOCLAW_SECURITY_PAIRING_MAX_ATTEMPTS") {
        if let Ok(n) = val.parse() { self.pairing.max_attempts = n; }
    }
    if let Ok(val) = std::env::var("ZEPTOCLAW_SECURITY_PAIRING_LOCKOUT_SECS") {
        if let Ok(n) = val.parse() { self.pairing.lockout_secs = n; }
    }
}
```

**Step 2: Create `src/security/pairing.rs`**

Key components:
- `PairingManager` — manages codes, tokens, lockouts
- `PairedDevice` — stored device info (hashed token, name, timestamps)
- `generate_pairing_code()` — random 6-digit code, valid for 5 minutes
- `complete_pairing(code, device_name)` — validates code, returns raw token
- `validate_token(raw_token)` — check SHA-256 hash against stored tokens
- `revoke(device_name)` — remove a paired device
- `list_devices()` — list paired devices (no tokens shown)
- Lockout: track failed attempts per identifier, block after max_attempts for lockout_secs

```rust
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH, Instant};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub name: String,
    pub token_hash: String,  // SHA-256 hex
    pub paired_at: u64,      // unix timestamp
    pub last_seen: u64,      // unix timestamp
}

#[derive(Serialize, Deserialize, Default)]
struct PairingStore {
    devices: Vec<PairedDevice>,
}

struct PendingCode {
    code: String,
    expires_at: Instant,
}

struct LockoutEntry {
    attempts: u32,
    locked_until: Option<Instant>,
}

pub struct PairingManager {
    store: PairingStore,
    path: PathBuf,
    pending_code: Option<PendingCode>,
    lockouts: HashMap<String, LockoutEntry>,
    max_attempts: u32,
    lockout_duration: std::time::Duration,
}
```

Include full CRUD operations, SHA-256 hashing for tokens, JSON persistence.

Generate raw tokens using `uuid::Uuid::new_v4()` (already in deps).

**Step 3: Create `src/cli/pair.rs`**

```rust
pub enum PairAction {
    New,
    List,
    Revoke { device: String },
}

pub async fn cmd_pair(action: PairAction) -> Result<()> {
    match action {
        PairAction::New => { /* generate code, print to terminal, wait for pairing */ }
        PairAction::List => { /* load store, print table */ }
        PairAction::Revoke { device } => { /* remove device, save store */ }
    }
}
```

**Step 4: Add CLI commands to `src/cli/mod.rs`**

```rust
Pair {
    #[command(subcommand)]
    action: PairAction,
},
```

**Step 5: Wire into gateway**

In the gateway request handler, if `config.pairing.enabled`, extract `Authorization: Bearer <token>` header and validate via `PairingManager::validate_token()`. Reject if invalid.

Study `src/gateway/container_agent.rs` to find the request handling code. The pairing check should be the first thing in the request pipeline.

**Step 6: Write tests (~15)**

Test: code generation (6 digits), code expiry, token hashing, token validation (valid/invalid), brute-force lockout (5 attempts then blocked), lockout expiry, device list, device revoke, config defaults, store persistence roundtrip.

**Step 7: Verify and commit**

```bash
cargo test --lib security::pairing -- --nocapture 2>&1 | tail -10
cargo clippy -- -D warnings 2>&1 | tail -5
git add src/security/ src/config/ src/gateway/ src/cli/
git commit -m "feat(security): add device pairing with bearer tokens and brute-force protection"
```

---

## Task 4: Hardware Support (Full Peripheral Stack)

**Files:**
- Create: `src/hardware/mod.rs`
- Create: `src/hardware/discover.rs`
- Create: `src/hardware/introspect.rs`
- Create: `src/hardware/registry.rs`
- Create: `src/peripherals/mod.rs`
- Create: `src/peripherals/traits.rs`
- Create: `src/peripherals/serial.rs`
- Create: `src/peripherals/arduino.rs`
- Create: `src/peripherals/nucleo.rs`
- Create: `src/peripherals/rpi.rs`
- Create: `src/tools/hardware.rs`
- Modify: `Cargo.toml`
- Modify: `src/tools/mod.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/lib.rs`

**Context:** Feature-gated (`--features hardware`). USB discovery via `nusb`, serial via `tokio-serial`, RPi GPIO via `rppal` (separate `peripheral-rpi` feature). Agent tool exposes device operations. Port structure from `~/ios/zeroclaw2/src/hardware/` and `~/ios/zeroclaw2/src/peripherals/`.

---

**Step 1: Add feature flags and deps to `Cargo.toml`**

```toml
[features]
hardware = ["nusb", "tokio-serial"]
peripheral-rpi = ["rppal"]
probe = ["probe-rs"]

[dependencies]
nusb = { version = "0.1", optional = true }
tokio-serial = { version = "5.4", optional = true }
rppal = { version = "0.19", optional = true }
probe-rs = { version = "0.24", optional = true }
```

Check zeroclaw2's Cargo.toml for exact version numbers. Adapt as needed.

**Step 2: Create `src/hardware/` module**

Port from `~/ios/zeroclaw2/src/hardware/`:

- `discover.rs` — `list_usb_devices()` using `nusb::list_devices()`, returns `Vec<UsbDevice>` with vendor_id, product_id, device class, serial number
- `introspect.rs` — `DeviceCapabilities` struct, detect what a device can do based on USB class
- `registry.rs` — JSON persistence at `~/.zeptoclaw/hardware/devices.json`, track known devices with last-seen timestamps
- `mod.rs` — re-exports, `HardwareManager` orchestrator

All gated behind `#[cfg(feature = "hardware")]`.

**Step 3: Create `src/peripherals/` module**

- `traits.rs` — `Peripheral` async trait:
```rust
#[async_trait]
pub trait Peripheral: Send + Sync {
    fn name(&self) -> &str;
    fn device_type(&self) -> &str;
    async fn connect(&mut self) -> anyhow::Result<()>;
    async fn disconnect(&mut self) -> anyhow::Result<()>;
    async fn send_command(&self, cmd: &str) -> anyhow::Result<String>;
    async fn read_data(&self) -> anyhow::Result<Vec<u8>>;
    fn is_connected(&self) -> bool;
}
```

- `serial.rs` — `SerialPeripheral` using `tokio-serial` (feature: `hardware`)
- `arduino.rs` — `ArduinoPeripheral` with sketch upload/flash via serial
- `nucleo.rs` — `NucleoPeripheral` for STM32 boards
- `rpi.rs` — `RpiPeripheral` using `rppal` (feature: `peripheral-rpi`, `#[cfg(target_os = "linux")]`)

Port from zeroclaw2. Adapt to match ZeptoClaw patterns.

**Step 4: Create `src/tools/hardware.rs`**

Feature-gated tool implementing `Tool` trait:

```rust
#[cfg(feature = "hardware")]
pub struct HardwareTool { ... }

// Without feature: stub that returns rebuild error
#[cfg(not(feature = "hardware"))]
pub struct HardwareTool;

#[async_trait]
impl Tool for HardwareTool {
    fn name(&self) -> &str { "hardware" }
    fn category(&self) -> ToolCategory { ToolCategory::Hardware }
    // ...
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        #[cfg(not(feature = "hardware"))]
        return Err(ZeptoError::Tool(
            "Hardware tool requires 'hardware' build feature. \
             Rebuild with: cargo build --features hardware".into()
        ));

        #[cfg(feature = "hardware")]
        {
            // dispatch based on args.action: list_devices, connect, send_command, read_data
        }
    }
}
```

**Step 5: Register tool and add CLI**

In `src/tools/mod.rs`:
```rust
pub mod hardware;
pub use hardware::HardwareTool;
```

In `src/cli/mod.rs`, add:
```rust
Hardware {
    #[command(subcommand)]
    action: HardwareAction,
},
```

With subcommands: `List`, `Info { device: String }`.

**Step 6: Write tests (~20)**

- USB discovery mock (mock `nusb` results)
- Serial frame parsing
- Device registry CRUD (add, list, remove, persistence)
- Peripheral trait compliance
- Feature-absent error message
- Arduino command formatting
- Config defaults

**Step 7: Verify and commit**

```bash
# Default build (no hardware)
cargo test --lib 2>&1 | tail -5
cargo clippy -- -D warnings 2>&1 | tail -5

# Feature build
cargo build --features hardware 2>&1 | grep "^error" | head -10

git add src/hardware/ src/peripherals/ src/tools/hardware.rs Cargo.toml src/cli/ src/lib.rs src/tools/mod.rs
git commit -m "feat(hardware): add full peripheral stack (USB, serial, Arduino, STM32, RPi), feature-gated"
```

---

## Final Verification

After all four tasks:

```bash
# Full test suite
cargo test 2>&1 | tail -10

# Default build stays lean
cargo build --release 2>&1 | grep "^error"
ls -lh target/release/zeptoclaw

# Feature builds compile
cargo build --release --features hardware 2>&1 | grep "^error"
cargo build --release --features peripheral-rpi 2>&1 | grep "^error"

# Lint
cargo clippy -- -D warnings 2>&1 | grep "^error"
```

Expected: all pass, default binary remains lean.

---

## Execution Order Summary

```
Task 1  Response cache     — new module, zero deps, wires into agent loop
Task 2  Agent modes        — new file, category tags on tools, wires into agent loop
Task 3  Device pairing     — new file + CLI + gateway middleware
Task 4  Hardware support   — new directories + feature flags + tool + CLI
```

Tasks 1-3 are independent. Task 4 depends on Task 2 (needs `ToolCategory::Hardware` to exist).
