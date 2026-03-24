# Network Egress Policy Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a unified network egress policy layer that controls which destinations each tool can contact, with deny-by-default semantics.

**Architecture:** New `src/safety/egress.rs` module defines `EgressConfig`, `EgressRule`, `EgressGuard`, and `EgressAction` (all types co-located, following the pattern of `TaintConfig` in `src/safety/taint.rs`). `EgressGuard::check(tool, url)` validates outbound requests. All network-capable tools call the guard before making HTTP requests. Denied attempts emit structured audit events via `src/audit.rs`. Existing per-tool controls (`http_request.allowed_domains`, SSRF checks in `web.rs`) remain as defense-in-depth — the egress guard runs **after** URL parsing/scheme validation but **before** any network I/O.

**Tech Stack:** Rust, serde (JSON config), reqwest::Url, existing audit infrastructure (`src/audit.rs`)

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/safety/egress.rs` | `EgressConfig`, `EgressAction`, `EgressRule`, `EgressGuard` — all types + logic + tests |
| Modify | `src/safety/mod.rs` | Add `pub mod egress;` export + `egress` field on `SafetyConfig` |
| Modify | `src/lib.rs` | Re-export `EgressGuard` |
| Modify | `src/tools/types.rs` | Add `egress_guard: Option<Arc<EgressGuard>>` to `ToolContext` |
| Modify | `src/agent/loop.rs` | Construct `EgressGuard`, pass to all 3 `ToolContext` construction sites |
| Modify | `src/config/mod.rs` | Add env overrides in `apply_safety_env_overrides()` |
| Modify | `src/tools/web.rs` | Wire check into `WebFetchTool`, `WebSearchTool` (Brave), `DdgSearchTool`, `SearxngSearchTool` |
| Modify | `src/tools/http_request.rs` | Wire check (after `validate_url()`, before request) |
| Modify | `src/tools/google.rs` | Wire check |
| Modify | `src/tools/stripe.rs` | Wire check |
| Modify | `src/tools/gsheets.rs` | Wire check |
| Modify | `src/tools/whatsapp.rs` | Wire check |
| Modify | `src/tools/project.rs` | Wire check |
| Modify | `src/tools/screenshot.rs` | Wire check |
| Modify | `src/tools/r8r.rs` | Wire check |
| Modify | `src/tools/transcribe.rs` | Wire check |

**Out of scope:** `src/tools/mcp/transport.rs` — MCP server URLs are user-configured and validated at MCP connection time, not per-request. Adding egress gating here would break the MCP client contract. Can be revisited in Phase B.

## Future Work (B/C — tracked in #371)

These are **NOT** in scope for this plan but must be implemented later:

- **Phase B: Destination-aware approval** — Extend `ApprovalGate` (currently tool-name-only in `src/tools/approval.rs:273`) to prompt for unlisted host+tool combinations. When `on_unlisted: prompt`, instead of denying, invoke the approval handler with destination details. Also add MCP transport egress gating. ~200 LOC.
- **Phase C: Hot-reload + audit log** — Watch policy config for changes and reload without restart. Add structured egress audit log (JSON lines) for compliance. ~300 LOC.

---

### Task 1: Define egress types in `src/safety/egress.rs`

**Files:**
- Create: `src/safety/egress.rs` (config types + tests only, no guard logic yet)
- Modify: `src/safety/mod.rs` (add `pub mod egress;` + `egress` field on `SafetyConfig`)

**Note:** All egress types live in `src/safety/egress.rs`, following the pattern of `TaintConfig` in `src/safety/taint.rs`. They are NOT in `src/config/types.rs`.

- [ ] **Step 1: Create `src/safety/egress.rs` with types and tests**

```rust
//! Network egress policy — controls which destinations each tool may contact.
//!
//! When enabled, all network-capable tools must call [`EgressGuard::check()`]
//! before making HTTP requests. The guard evaluates rules in order; the first
//! matching rule allows the request. If no rule matches, the `default_action`
//! applies.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Default action when no rule matches a network request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EgressAction {
    /// Allow the request (log only).
    Allow,
    /// Deny the request.
    Deny,
}

impl Default for EgressAction {
    fn default() -> Self {
        Self::Allow
    }
}

/// A single egress rule: which endpoints a set of tools may contact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressRule {
    /// Human-readable name for this rule (e.g., "inference-providers").
    pub name: String,
    /// Endpoint patterns to allow. Supports exact match and wildcard prefix
    /// (e.g., `"*.googleapis.com"`). Matched against URL host.
    pub endpoints: Vec<String>,
    /// Tool names this rule applies to. `["*"]` means all tools.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Allowed ports. Empty means any port.
    #[serde(default)]
    pub ports: Vec<u16>,
}

/// Network egress policy configuration.
///
/// When `enabled`, all network-capable tools must pass through the
/// [`EgressGuard`] before making HTTP requests. Default action applies
/// when no rule matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EgressConfig {
    /// Whether egress policy enforcement is enabled.
    pub enabled: bool,
    /// Action when no rule matches.
    pub default_action: EgressAction,
    /// Ordered list of allow rules.
    pub rules: Vec<EgressRule>,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_action: EgressAction::Allow,
            rules: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_egress_config_defaults() {
        let config = EgressConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.default_action, EgressAction::Allow);
        assert!(config.rules.is_empty());
    }

    #[test]
    fn test_egress_config_deserialize() {
        let json = r#"{
            "enabled": true,
            "default_action": "deny",
            "rules": [
                {
                    "name": "inference",
                    "endpoints": ["api.anthropic.com", "api.openai.com"],
                    "tools": ["*"],
                    "ports": [443]
                }
            ]
        }"#;
        let config: EgressConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.default_action, EgressAction::Deny);
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].name, "inference");
        assert_eq!(
            config.rules[0].endpoints,
            vec!["api.anthropic.com", "api.openai.com"]
        );
        assert_eq!(config.rules[0].tools, vec!["*"]);
        assert_eq!(config.rules[0].ports, vec![443]);
    }
}
```

- [ ] **Step 2: Add module export and field to `SafetyConfig`**

In `src/safety/mod.rs`:

1. Add `pub mod egress;` alongside the other module exports (after `pub mod chain_alert;`).

2. Add field to `SafetyConfig` struct:
```rust
    /// Network egress policy configuration.
    #[serde(default)]
    pub egress: egress::EgressConfig,
```

3. Add to `SafetyConfig::default()` impl:
```rust
    egress: egress::EgressConfig::default(),
```

- [ ] **Step 3: Run tests**

Run: `cargo nextest run --lib test_egress_config`
Expected: PASS (2 tests)

- [ ] **Step 4: Commit**

```bash
git add src/safety/egress.rs src/safety/mod.rs
git commit -m "feat(safety): add EgressConfig types for network egress policy"
```

---

### Task 2: Implement `EgressGuard` core logic

**Files:**
- Modify: `src/safety/egress.rs` (add guard impl + 11 tests)

- [ ] **Step 1: Add tests to `src/safety/egress.rs` test module**

```rust
    fn test_rule(name: &str, endpoints: &[&str], tools: &[&str], ports: &[u16]) -> EgressRule {
        EgressRule {
            name: name.to_string(),
            endpoints: endpoints.iter().map(|s| s.to_string()).collect(),
            tools: tools.iter().map(|s| s.to_string()).collect(),
            ports: ports.to_vec(),
        }
    }

    fn deny_by_default_config(rules: Vec<EgressRule>) -> EgressConfig {
        EgressConfig {
            enabled: true,
            default_action: EgressAction::Deny,
            rules,
        }
    }

    #[test]
    fn test_disabled_guard_allows_everything() {
        let guard = EgressGuard::new(EgressConfig::default()); // enabled: false
        let result = guard.check("web_fetch", "https://evil.com/data");
        assert!(result.is_ok());
    }

    #[test]
    fn test_deny_by_default_blocks_unlisted() {
        let guard = EgressGuard::new(deny_by_default_config(vec![]));
        let result = guard.check("web_fetch", "https://unknown.com/api");
        assert!(result.is_err());
    }

    #[test]
    fn test_exact_endpoint_match_allows() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("github", &["api.github.com"], &["project"], &[443]),
        ]));
        assert!(guard.check("project", "https://api.github.com/repos").is_ok());
    }

    #[test]
    fn test_wildcard_endpoint_match() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("google", &["*.googleapis.com"], &["web_search"], &[]),
        ]));
        assert!(guard.check("web_search", "https://www.googleapis.com/search").is_ok());
        assert!(guard.check("web_search", "https://maps.googleapis.com/api").is_ok());
        assert!(guard.check("web_search", "https://evil.com").is_err());
    }

    #[test]
    fn test_wildcard_tool_allows_any_tool() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("inference", &["api.anthropic.com"], &["*"], &[443]),
        ]));
        assert!(guard.check("web_fetch", "https://api.anthropic.com/v1/messages").is_ok());
        assert!(guard.check("http_request", "https://api.anthropic.com/v1/messages").is_ok());
    }

    #[test]
    fn test_wrong_tool_denied() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("stripe-only", &["api.stripe.com"], &["stripe"], &[443]),
        ]));
        assert!(guard.check("stripe", "https://api.stripe.com/v1/charges").is_ok());
        assert!(guard.check("web_fetch", "https://api.stripe.com/v1/charges").is_err());
    }

    #[test]
    fn test_wrong_port_denied() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("https-only", &["api.example.com"], &["*"], &[443]),
        ]));
        assert!(guard.check("web_fetch", "https://api.example.com/data").is_ok());
        assert!(guard.check("web_fetch", "http://api.example.com:8080/data").is_err());
    }

    #[test]
    fn test_empty_ports_allows_any_port() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("any-port", &["localhost"], &["whatsapp"], &[]),
        ]));
        assert!(guard.check("whatsapp", "http://localhost:3000/send").is_ok());
        assert!(guard.check("whatsapp", "http://localhost:8080/send").is_ok());
    }

    #[test]
    fn test_allow_by_default_allows_unlisted() {
        let config = EgressConfig {
            enabled: true,
            default_action: EgressAction::Allow,
            rules: vec![],
        };
        let guard = EgressGuard::new(config);
        assert!(guard.check("web_fetch", "https://anything.com").is_ok());
    }

    #[test]
    fn test_invalid_url_returns_error() {
        let guard = EgressGuard::new(deny_by_default_config(vec![]));
        assert!(guard.check("web_fetch", "not-a-url").is_err());
    }

    #[test]
    fn test_multiple_rules_first_match_wins() {
        let guard = EgressGuard::new(deny_by_default_config(vec![
            test_rule("github", &["api.github.com"], &["project"], &[443]),
            test_rule("stripe", &["api.stripe.com"], &["stripe"], &[443]),
        ]));
        assert!(guard.check("project", "https://api.github.com/repos").is_ok());
        assert!(guard.check("stripe", "https://api.stripe.com/v1").is_ok());
        assert!(guard.check("web_fetch", "https://unknown.com").is_err());
    }
```

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cargo nextest run --lib test_disabled_guard`
Expected: FAIL — `EgressGuard` does not exist.

- [ ] **Step 3: Implement `EgressGuard`**

Add above the `#[cfg(test)]` block in `src/safety/egress.rs`:

```rust
use crate::audit::{log_audit_event, AuditCategory, AuditSeverity};
use crate::error::{Result, ZeptoError};
use reqwest::Url;

// ---------------------------------------------------------------------------
// EgressGuard
// ---------------------------------------------------------------------------

/// Guard that checks outbound network requests against the egress policy.
///
/// Constructed once from [`EgressConfig`] and shared (via `Arc`) across all
/// tool calls. Cheap to query — no allocations on the hot path.
#[derive(Debug, Clone)]
pub struct EgressGuard {
    config: EgressConfig,
}

impl EgressGuard {
    /// Create a new guard from config.
    pub fn new(config: EgressConfig) -> Self {
        Self { config }
    }

    /// Check whether `tool_name` is allowed to contact `raw_url`.
    ///
    /// Returns `Ok(())` if allowed, `Err` if denied or the URL is invalid.
    pub fn check(&self, tool_name: &str, raw_url: &str) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let url = Url::parse(raw_url)
            .map_err(|e| ZeptoError::Tool(format!("Egress check: invalid URL '{raw_url}': {e}")))?;

        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let port = url.port_or_known_default().unwrap_or(0);

        for rule in &self.config.rules {
            if rule_matches(rule, tool_name, &host, port) {
                return Ok(());
            }
        }

        // No rule matched — apply default action
        match self.config.default_action {
            EgressAction::Allow => Ok(()),
            EgressAction::Deny => {
                log_audit_event(
                    AuditCategory::PolicyViolation,
                    AuditSeverity::Warning,
                    "egress_denied",
                    &format!("Tool '{tool_name}' denied access to '{host}:{port}'"),
                    true,
                );
                Err(ZeptoError::SecurityViolation(format!(
                    "Egress denied: tool '{tool_name}' is not allowed to contact '{host}'"
                )))
            }
        }
    }
}

/// Check if a rule matches the given tool, host, and port.
fn rule_matches(rule: &EgressRule, tool_name: &str, host: &str, port: u16) -> bool {
    // Check tool match
    let tool_ok = rule.tools.is_empty()
        || rule.tools.iter().any(|t| t == "*" || t == tool_name);
    if !tool_ok {
        return false;
    }

    // Check port match (empty = any port)
    let port_ok = rule.ports.is_empty() || rule.ports.contains(&port);
    if !port_ok {
        return false;
    }

    // Check endpoint match
    rule.endpoints
        .iter()
        .any(|pattern| endpoint_matches(pattern, host))
}

/// Match a host against an endpoint pattern.
///
/// Supports exact match and wildcard prefix (`*.example.com`).
/// IP addresses are matched exactly (no CIDR support — document this
/// limitation; can be added in Phase B).
fn endpoint_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == pattern
    }
}
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo nextest run --lib -p zeptoclaw egress`
Expected: All 13 tests PASS (2 config + 11 guard).

- [ ] **Step 5: Commit**

```bash
git add src/safety/egress.rs
git commit -m "feat(safety): implement EgressGuard with deny-by-default network policy"
```

---

### Task 3: Wire `EgressGuard` into `ToolContext`

**Files:**
- Modify: `src/tools/types.rs` (add field to `ToolContext`)
- Modify: `src/agent/loop.rs` (construct guard, pass to all 3 `ToolContext` sites)
- Modify: `src/lib.rs` (re-export)

**Important:** `ToolContext` at `src/tools/types.rs:252` derives `Default`. Adding `egress_guard: Option<Arc<EgressGuard>>` is safe because `Option<T>` defaults to `None`. Do NOT add a manual `Default` impl — the derive handles it.

**Important:** There are **3** `ToolContext::new()` sites in `src/agent/loop.rs`:
1. Line 1158 — main tool execution loop
2. Line 1818 — streaming tool execution loop
3. Line 2359 — memory flush tool execution

All three must receive the egress guard.

- [ ] **Step 1: Add field and builder method to `ToolContext`**

In `src/tools/types.rs`, add to `ToolContext` struct (line ~262):

```rust
    /// Network egress guard for policy enforcement. Tools making HTTP
    /// requests should call `guard.check(tool_name, url)` before sending.
    /// `None` means no policy enforcement. Wrapped in `Arc` to avoid
    /// cloning the rule set per tool call.
    pub egress_guard: Option<std::sync::Arc<crate::safety::egress::EgressGuard>>,
```

Add a builder method after `with_batch()`:

```rust
    /// Set the egress guard.
    pub fn with_egress_guard(mut self, guard: std::sync::Arc<crate::safety::egress::EgressGuard>) -> Self {
        self.egress_guard = Some(guard);
        self
    }
```

- [ ] **Step 2: Construct guard and wire into all 3 sites in `src/agent/loop.rs`**

Near the top of the agent loop (where `safety_layer` and `taint_engine` are initialized), add:

```rust
let egress_guard: Option<std::sync::Arc<crate::safety::egress::EgressGuard>> =
    if self.config.safety.egress.enabled {
        Some(std::sync::Arc::new(
            crate::safety::egress::EgressGuard::new(self.config.safety.egress.clone()),
        ))
    } else {
        None
    };
```

Then at each of the 3 `ToolContext::new()` sites (lines 1158, 1818, 2359), chain `.with_egress_guard(guard)` if the guard is `Some`:

```rust
let tool_ctx = ToolContext::new()
    .with_channel(&msg.channel, &msg.chat_id)
    .with_workspace(&workspace_str)
    .with_batch(msg.metadata.get("is_batch").is_some_and(|v| v == "true"));
// Add egress guard
let tool_ctx = if let Some(ref guard) = egress_guard {
    tool_ctx.with_egress_guard(Arc::clone(guard))
} else {
    tool_ctx
};
```

- [ ] **Step 3: Re-export from `src/lib.rs`**

Add with the other safety re-exports:

```rust
pub use safety::egress::EgressGuard;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo build`
Expected: Compiles without errors.

- [ ] **Step 5: Commit**

```bash
git add src/tools/types.rs src/agent/loop.rs src/lib.rs
git commit -m "feat(safety): wire EgressGuard into ToolContext at all 3 construction sites"
```

---

### Task 4: Wire egress check into web tools

**Files:**
- Modify: `src/tools/web.rs`

There are **4** tool structs in `web.rs`: `WebFetchTool`, `WebSearchTool` (Brave), `DdgSearchTool`, `SearxngSearchTool`.

For search tools, the egress check URL is the **API endpoint** (not the user's query), since that's where the HTTP request actually goes:
- `WebSearchTool`: uses `BRAVE_API_URL` constant
- `DdgSearchTool`: uses `DDG_HTML_URL` constant
- `SearxngSearchTool`: uses `self.api_url` (configurable)

For `WebFetchTool`: the URL is the user-provided `url` argument.

- [ ] **Step 1: Write failing test**

Add to `src/tools/web.rs` test module:

```rust
#[tokio::test]
async fn test_web_fetch_egress_denied() {
    use crate::safety::egress::{EgressConfig, EgressAction, EgressGuard};
    use std::sync::Arc;

    let guard = Arc::new(EgressGuard::new(EgressConfig {
        enabled: true,
        default_action: EgressAction::Deny,
        rules: vec![],
    }));
    let ctx = ToolContext::new().with_egress_guard(guard);

    let tool = WebFetchTool::new();
    let args = serde_json::json!({"url": "https://blocked.example.com"});
    let result = tool.execute(args, &ctx).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.to_lowercase().contains("egress"));
}
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo nextest run --lib test_web_fetch_egress_denied`

- [ ] **Step 3: Add egress check to all 4 tools**

In each tool's `execute()` method, add immediately after URL parsing/validation, **before** any `self.client.get()` or network I/O:

```rust
// Egress policy check
if let Some(ref guard) = ctx.egress_guard {
    guard.check(self.name(), &url_string)?;
}
```

Where `url_string` is:
- `WebFetchTool`: the parsed `url` from args
- `WebSearchTool`: the `BRAVE_API_URL` constant (the actual HTTP destination)
- `DdgSearchTool`: the `DDG_HTML_URL` constant
- `SearxngSearchTool`: `&self.api_url`

- [ ] **Step 4: Run test — expect PASS**

Run: `cargo nextest run --lib test_web_fetch_egress_denied`

- [ ] **Step 5: Commit**

```bash
git add src/tools/web.rs
git commit -m "feat(safety): wire egress check into web_fetch and all web_search variants"
```

---

### Task 5: Wire egress check into `http_request`

**Files:**
- Modify: `src/tools/http_request.rs`

**Ordering note:** The egress check goes **after** `self.validate_url()` (which handles the per-tool `allowed_domains` allowlist + SSRF checks) and **before** the actual `self.client.request()` call. This is defense-in-depth: the tool's own allowlist passes, then the global egress policy is checked.

- [ ] **Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_http_request_egress_denied() {
    use crate::safety::egress::{EgressConfig, EgressAction, EgressGuard};
    use std::sync::Arc;

    let guard = Arc::new(EgressGuard::new(EgressConfig {
        enabled: true,
        default_action: EgressAction::Deny,
        rules: vec![],
    }));
    // Domain IS in allowed_domains (passes tool check) but egress policy blocks it
    let tool = HttpRequestTool::new(
        vec!["blocked.example.com".to_string()],
        30,
        512_000,
    );
    let ctx = ToolContext::new().with_egress_guard(guard);
    let args = serde_json::json!({
        "method": "GET",
        "url": "https://blocked.example.com/api"
    });
    let result = tool.execute(args, &ctx).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.to_lowercase().contains("egress"));
}
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo nextest run --lib test_http_request_egress_denied`

- [ ] **Step 3: Add egress check after `validate_url()` call**

In `HttpRequestTool::execute()`, find where `self.validate_url(&url_str)` is called. Add immediately after:

```rust
// Global egress policy check (defense-in-depth — runs after per-tool allowlist)
if let Some(ref guard) = ctx.egress_guard {
    guard.check(self.name(), &url_str)?;
}
```

- [ ] **Step 4: Run test — expect PASS**

Run: `cargo nextest run --lib test_http_request_egress_denied`

- [ ] **Step 5: Commit**

```bash
git add src/tools/http_request.rs
git commit -m "feat(safety): wire egress check into http_request (defense-in-depth)"
```

---

### Task 6: Wire egress check into remaining network tools

**Files:**
- Modify: `src/tools/google.rs` (Google API — check against constructed API URL)
- Modify: `src/tools/stripe.rs` (always `api.stripe.com` — check against constructed URL)
- Modify: `src/tools/gsheets.rs` (Google Sheets API URL)
- Modify: `src/tools/whatsapp.rs` (bridge URL from config, e.g., `http://localhost:3000`)
- Modify: `src/tools/project.rs` (GitHub/Jira/Linear API URLs)
- Modify: `src/tools/screenshot.rs` (user-provided page URL)
- Modify: `src/tools/r8r.rs` (r8r bridge URL from config)
- Modify: `src/tools/transcribe.rs` (Groq API URL)

- [ ] **Step 1: Add egress check to each tool**

For each tool, in the `execute()` method, add after URL construction but before any `self.client` HTTP call:

```rust
if let Some(ref guard) = ctx.egress_guard {
    guard.check(self.name(), &url)?;
}
```

The URL to check varies per tool:
- `google.rs`: the constructed Google API URL
- `stripe.rs`: the constructed `https://api.stripe.com/...` URL
- `gsheets.rs`: the constructed Sheets API URL
- `whatsapp.rs`: the bridge URL (e.g., `http://localhost:3000/api/sendMessage`)
- `project.rs`: depends on backend — `https://api.github.com/...`, Jira URL, or Linear URL
- `screenshot.rs`: the user-provided page URL argument
- `r8r.rs`: the r8r bridge URL
- `transcribe.rs`: the Groq API URL

- [ ] **Step 2: Write one test per tool**

Add a test to each tool's test module using the same deny-by-default pattern:

```rust
#[tokio::test]
async fn test_<tool>_egress_denied() {
    use crate::safety::egress::{EgressConfig, EgressAction, EgressGuard};
    use std::sync::Arc;

    let guard = Arc::new(EgressGuard::new(EgressConfig {
        enabled: true,
        default_action: EgressAction::Deny,
        rules: vec![],
    }));
    let ctx = ToolContext::new().with_egress_guard(guard);
    // ... construct tool and args appropriate to each tool
    let result = tool.execute(args, &ctx).await;
    assert!(result.is_err());
}
```

- [ ] **Step 3: Run all egress tests**

Run: `cargo nextest run --lib egress`
Expected: All tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/tools/google.rs src/tools/stripe.rs src/tools/gsheets.rs \
       src/tools/whatsapp.rs src/tools/project.rs src/tools/screenshot.rs \
       src/tools/r8r.rs src/tools/transcribe.rs
git commit -m "feat(safety): wire egress check into all remaining network tools"
```

---

### Task 7: Add env overrides for egress config

**Files:**
- Modify: `src/config/mod.rs` (add to `apply_safety_env_overrides()` method)

**Important:** Env overrides go in `apply_safety_env_overrides(&mut self)` (line 1182 of `src/config/mod.rs`), which is an instance method on `Config`. Follow the existing pattern with `self.safety.egress.*`.

- [ ] **Step 1: Add env overrides**

In `src/config/mod.rs`, at the end of `apply_safety_env_overrides()` (after the taint overrides around line 1202):

```rust
        if let Ok(val) = std::env::var("ZEPTOCLAW_SAFETY_EGRESS_ENABLED") {
            self.safety.egress.enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_SAFETY_EGRESS_DEFAULT_ACTION") {
            match val.to_ascii_lowercase().as_str() {
                "deny" => self.safety.egress.default_action = crate::safety::egress::EgressAction::Deny,
                "allow" => self.safety.egress.default_action = crate::safety::egress::EgressAction::Allow,
                _ => {}
            }
        }
```

- [ ] **Step 2: Write test**

Add to the existing config tests (note: uses `self.apply_env_overrides()` instance method, consistent with existing env override tests):

```rust
#[test]
fn test_egress_env_override() {
    // NOTE: env var tests are inherently racy in parallel; this is consistent
    // with existing tests in this module (e.g., test_env_override).
    std::env::set_var("ZEPTOCLAW_SAFETY_EGRESS_ENABLED", "true");
    std::env::set_var("ZEPTOCLAW_SAFETY_EGRESS_DEFAULT_ACTION", "deny");
    let mut config = Config::default();
    config.apply_env_overrides();
    assert!(config.safety.egress.enabled);
    assert_eq!(
        config.safety.egress.default_action,
        crate::safety::egress::EgressAction::Deny
    );
    std::env::remove_var("ZEPTOCLAW_SAFETY_EGRESS_ENABLED");
    std::env::remove_var("ZEPTOCLAW_SAFETY_EGRESS_DEFAULT_ACTION");
}
```

- [ ] **Step 3: Run test — expect PASS**

Run: `cargo nextest run --lib test_egress_env_override`

- [ ] **Step 4: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(config): add env overrides for egress policy"
```

---

### Task 8: Pre-push checklist

**Files:** All modified files

- [ ] **Step 1: Run full test suite**

```bash
cargo nextest run --lib
```

Expected: All tests pass, including ~25 new egress tests.

- [ ] **Step 2: Run lint and format**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo fmt -- --check
```

Expected: No warnings, no format issues.

- [ ] **Step 3: Run doc tests**

```bash
cargo test --doc
```

Expected: All doc tests pass.

- [ ] **Step 4: Verify binary size**

```bash
cargo build --release
ls -la target/release/zeptoclaw | awk '{print $5}'
```

Expected: Still under 11MB.

- [ ] **Step 5: Final fixup commit (if needed)**

```bash
git add -A
git commit -m "chore: fixups from pre-push checklist"
```

---

## Config Example

```json
{
  "safety": {
    "egress": {
      "enabled": true,
      "default_action": "deny",
      "rules": [
        {
          "name": "inference-providers",
          "endpoints": ["api.anthropic.com", "api.openai.com"],
          "tools": ["*"],
          "ports": [443]
        },
        {
          "name": "github",
          "endpoints": ["api.github.com", "github.com"],
          "tools": ["project", "web_fetch"],
          "ports": [443]
        },
        {
          "name": "whatsapp-bridge",
          "endpoints": ["localhost"],
          "tools": ["whatsapp"],
          "ports": [3000]
        },
        {
          "name": "stripe",
          "endpoints": ["api.stripe.com"],
          "tools": ["stripe"],
          "ports": [443]
        },
        {
          "name": "google-apis",
          "endpoints": ["*.googleapis.com", "accounts.google.com"],
          "tools": ["google", "google_sheets", "web_search"],
          "ports": [443]
        },
        {
          "name": "r8r-bridge",
          "endpoints": ["localhost"],
          "tools": ["r8r"],
          "ports": [8080]
        }
      ]
    }
  }
}
```

## Limitations (Phase A)

- **No CIDR/IP-range matching** — IP endpoints are exact match only
- **No MCP transport gating** — MCP server URLs bypass egress (deferred to Phase B)
- **No hot-reload** — policy changes require restart (deferred to Phase C)
- **No approval flow for unlisted hosts** — denied, not prompted (deferred to Phase B)

## Test Count Estimate

- Task 1: 2 tests (config defaults + deserialization)
- Task 2: 11 tests (core guard logic)
- Task 4: 1 test (web_fetch egress denied)
- Task 5: 1 test (http_request egress denied)
- Task 6: ~8 tests (one per remaining tool)
- Task 7: 1 test (env override)

**Total: ~24 new tests**
