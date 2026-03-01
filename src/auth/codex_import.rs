//! Import OAuth credentials from Codex CLI (`~/.codex/auth.json` or macOS Keychain).
//!
//! Codex CLI stores OpenAI OAuth tokens in two locations:
//! 1. macOS Keychain (service: "Codex Auth", account: `cli|<sha256(codex_home)[:16]>`)
//! 2. File: `$CODEX_HOME/auth.json` or `~/.codex/auth.json`
//!
//! This module reads from both sources (preferring Keychain on macOS) and converts
//! them into ZeptoClaw's [`OAuthTokenSet`] for use with the OpenAI provider.

use std::path::Path;

use super::OAuthTokenSet;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

/// Duration (in seconds) that a Codex access token is assumed valid.
const CODEX_TOKEN_LIFETIME_SECS: i64 = 3600;

// ============================================================================
// JSON structures matching Codex CLI's auth file format
// ============================================================================

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokens>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

/// Extended format stored in macOS Keychain (includes `last_refresh` timestamp).
#[derive(Debug, Deserialize)]
struct CodexKeychainData {
    tokens: Option<CodexTokens>,
    last_refresh: Option<String>,
}

// ============================================================================
// Public API
// ============================================================================

/// Attempt to read Codex CLI credentials and convert them to an [`OAuthTokenSet`].
///
/// Lookup order:
/// 1. macOS Keychain (if on macOS)
/// 2. `$CODEX_HOME/auth.json` (or `~/.codex/auth.json`)
///
/// Returns `None` if no valid credentials are found.
pub fn read_codex_credentials() -> Option<OAuthTokenSet> {
    let codex_home = resolve_codex_home()?;

    // 1. Try macOS Keychain first
    #[cfg(target_os = "macos")]
    {
        if let Some(token_set) = read_from_keychain(&codex_home) {
            debug!("imported Codex credentials from macOS Keychain");
            return Some(token_set);
        }
    }

    // 2. Fall back to auth.json file
    let auth_path = Path::new(&codex_home).join("auth.json");
    if let Some(token_set) = read_from_auth_file(&auth_path) {
        debug!("imported Codex credentials from auth.json");
        return Some(token_set);
    }

    debug!("no Codex CLI credentials found");
    None
}

// ============================================================================
// Keychain reader (macOS only)
// ============================================================================

#[cfg(target_os = "macos")]
fn read_from_keychain(codex_home: &str) -> Option<OAuthTokenSet> {
    let account = keychain_account(codex_home);

    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Codex Auth",
            "-a",
            &account,
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("Keychain lookup failed (item not found or access denied)");
        return None;
    }

    let json_str = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let data: CodexKeychainData = serde_json::from_str(&json_str)
        .map_err(|e| {
            warn!("failed to parse Codex Keychain data: {e}");
            e
        })
        .ok()?;

    let tokens = data.tokens?;
    let access_token = tokens.access_token.filter(|s| !s.is_empty())?;
    let refresh_token = tokens.refresh_token.filter(|s| !s.is_empty());

    // Compute expiry from last_refresh or fallback to now + lifetime
    let now = chrono::Utc::now().timestamp();
    let obtained_at = data
        .last_refresh
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp())
        .unwrap_or(now);
    let expires_at = obtained_at + CODEX_TOKEN_LIFETIME_SECS;

    Some(OAuthTokenSet {
        provider: "openai".to_string(),
        access_token,
        refresh_token,
        expires_at: Some(expires_at),
        token_type: "Bearer".to_string(),
        scope: None,
        obtained_at,
        client_id: None,
    })
}

// ============================================================================
// File reader (all platforms)
// ============================================================================

/// Read Codex credentials from an auth.json file at the given path.
///
/// This is separated from path resolution so tests can pass an explicit path
/// without relying on environment variables (which race in parallel tests).
fn read_from_auth_file(auth_path: &Path) -> Option<OAuthTokenSet> {
    if !auth_path.exists() {
        debug!("Codex auth file not found at {}", auth_path.display());
        return None;
    }

    let content = std::fs::read_to_string(auth_path)
        .map_err(|e| {
            warn!("failed to read {}: {e}", auth_path.display());
            e
        })
        .ok()?;

    let auth_file: CodexAuthFile = serde_json::from_str(&content)
        .map_err(|e| {
            warn!("failed to parse {}: {e}", auth_path.display());
            e
        })
        .ok()?;

    let tokens = auth_file.tokens?;
    let access_token = tokens.access_token.filter(|s| !s.is_empty())?;
    let refresh_token = tokens.refresh_token.filter(|s| !s.is_empty());

    // No reliable expiry info in auth.json (no timestamp field).
    // Set obtained_at = now and expires_at = None so ZeptoClaw attempts
    // to use the token and handles 401 via the refresh flow if expired.
    let now = chrono::Utc::now().timestamp();

    Some(OAuthTokenSet {
        provider: "openai".to_string(),
        access_token,
        refresh_token,
        expires_at: None,
        token_type: "Bearer".to_string(),
        scope: None,
        obtained_at: now,
        client_id: None,
    })
}

// ============================================================================
// Helpers
// ============================================================================

/// Resolve the Codex home directory.
///
/// Priority: `$CODEX_HOME` env var, then `~/.codex`.
fn resolve_codex_home() -> Option<String> {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        if !codex_home.is_empty() {
            return Some(codex_home);
        }
    }
    dirs::home_dir().map(|h| h.join(".codex").to_string_lossy().into_owned())
}

/// Resolve the auth.json file path.
///
/// Used by tests and will be used by CLI wiring for `auth login openai`.
#[allow(dead_code)]
pub(crate) fn resolve_auth_path() -> Option<std::path::PathBuf> {
    resolve_codex_home().map(|h| Path::new(&h).join("auth.json"))
}

/// Compute the Keychain account identifier for Codex CLI.
///
/// Format: `cli|<sha256(codex_home_path)[:16]>` (16 hex chars = 8 bytes).
pub(crate) fn keychain_account(codex_home: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(codex_home.as_bytes());
    let hash = hasher.finalize();
    let hex_full = hex::encode(hash);
    format!("cli|{}", &hex_full[..16])
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// RAII guard that removes an environment variable on drop.
    /// Prevents env var leaks between parallel tests.
    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    /// Helper: create a temp dir, write auth.json with given content, return (TempDir, auth_path).
    fn write_auth_file(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let auth_path = tmp.path().join("auth.json");
        let mut f = std::fs::File::create(&auth_path).expect("create auth.json");
        f.write_all(content.as_bytes()).expect("write auth.json");
        (tmp, auth_path)
    }

    #[test]
    fn test_read_from_file_valid() {
        let now = chrono::Utc::now().timestamp();
        let (_tmp, auth_path) = write_auth_file(
            r#"{"tokens": {"access_token": "oat-abc123", "refresh_token": "ort-def456"}}"#,
        );

        let result = read_from_auth_file(&auth_path);
        assert!(result.is_some(), "expected Some for valid auth.json");

        let ts = result.unwrap();
        assert_eq!(ts.provider, "openai");
        assert_eq!(ts.access_token, "oat-abc123");
        assert_eq!(ts.refresh_token.as_deref(), Some("ort-def456"));
        assert_eq!(ts.token_type, "Bearer");
        assert!(ts.scope.is_none());
        assert!(ts.client_id.is_none());
        // expires_at is None — no reliable expiry from auth.json
        assert!(
            ts.expires_at.is_none(),
            "expires_at should be None for file import"
        );
        // obtained_at should be close to now (not file mtime)
        assert!(
            (ts.obtained_at - now).abs() < 5,
            "obtained_at should be close to now"
        );
    }

    #[test]
    fn test_read_from_file_missing() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let auth_path = tmp.path().join("auth.json");
        // Do not create the file

        let result = read_from_auth_file(&auth_path);
        assert!(
            result.is_none(),
            "expected None when auth.json does not exist"
        );
    }

    #[test]
    fn test_read_from_file_malformed_json() {
        let (_tmp, auth_path) = write_auth_file("not json at all {{{");

        let result = read_from_auth_file(&auth_path);
        assert!(result.is_none(), "expected None for malformed JSON");
    }

    #[test]
    fn test_read_from_file_missing_access_token() {
        let (_tmp, auth_path) = write_auth_file(r#"{"tokens": {"refresh_token": "ort-def456"}}"#);

        let result = read_from_auth_file(&auth_path);
        assert!(
            result.is_none(),
            "expected None when access_token is missing"
        );
    }

    #[test]
    fn test_read_from_file_missing_refresh_token() {
        // Only access_token present: should return Some with refresh_token = None
        let (_tmp, auth_path) = write_auth_file(r#"{"tokens": {"access_token": "oat-abc123"}}"#);

        let result = read_from_auth_file(&auth_path);
        assert!(
            result.is_some(),
            "expected Some when only access_token is present"
        );
        let ts = result.unwrap();
        assert_eq!(ts.access_token, "oat-abc123");
        assert!(
            ts.refresh_token.is_none(),
            "refresh_token should be None when not in file"
        );
    }

    #[test]
    fn test_resolve_auth_path_uses_codex_home_env() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let custom_path = tmp.path().to_str().unwrap().to_string();
        std::env::set_var("CODEX_HOME", &custom_path);
        let _guard = EnvGuard("CODEX_HOME");
        let path = resolve_auth_path();
        assert!(path.is_some());
        assert_eq!(path.unwrap(), tmp.path().join("auth.json"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_account_format() {
        let account = keychain_account("/Users/testuser/.codex");
        // Should be "cli|" + 16 hex characters
        assert!(
            account.starts_with("cli|"),
            "account should start with 'cli|': {account}"
        );
        let suffix = &account[4..];
        assert_eq!(
            suffix.len(),
            16,
            "hash suffix should be 16 hex chars: {suffix}"
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix should be hex: {suffix}"
        );
    }

    // Also test keychain_account on all platforms (not gated) for coverage
    #[test]
    fn test_keychain_account_deterministic() {
        let a1 = keychain_account("/Users/test/.codex");
        let a2 = keychain_account("/Users/test/.codex");
        assert_eq!(a1, a2, "same input should produce same account");

        let a3 = keychain_account("/Users/other/.codex");
        assert_ne!(a1, a3, "different input should produce different account");
    }

    #[test]
    fn test_read_from_file_empty_access_token() {
        let (_tmp, auth_path) =
            write_auth_file(r#"{"tokens": {"access_token": "", "refresh_token": "ort-def456"}}"#);

        let result = read_from_auth_file(&auth_path);
        assert!(
            result.is_none(),
            "expected None when access_token is empty string"
        );
    }
}
