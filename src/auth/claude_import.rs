//! Import OAuth credentials from Claude CLI (`~/.claude.json` or macOS Keychain).
//!
//! Claude CLI products (Claude Code, Claude Cowork, and future products) store
//! Anthropic OAuth tokens in two locations:
//! 1. macOS Keychain (services: "Claude Code-credentials", "Claude Cowork-credentials", etc.)
//! 2. File: `~/.claude.json`
//!
//! This module reads from both sources (preferring Keychain on macOS) and converts
//! them into ZeptoClaw's [`OAuthTokenSet`] for use with the Anthropic provider.

use std::io::BufReader;
use std::path::{Path, PathBuf};

use super::OAuthTokenSet;
use serde::Deserialize;
use tracing::{debug, warn};

// ============================================================================
// JSON structures matching Claude CLI's config file format
// ============================================================================

/// Top-level structure of `~/.claude.json`.
#[derive(Debug, Deserialize)]
struct ClaudeConfigFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOAuth>,
}

/// OAuth token fields within the Claude config (camelCase in JSON).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOAuth {
    access_token: Option<String>,
    refresh_token: Option<String>,
    /// Token expiry timestamp in **milliseconds** since epoch.
    expires_at: Option<i64>,
}

/// Keychain blob structure (same shape as config file).
#[cfg(target_os = "macos")]
#[derive(Debug, Deserialize)]
struct ClaudeKeychainData {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOAuth>,
}

// ============================================================================
// Keychain constants (macOS only)
// ============================================================================

/// Keychain service names used by Claude CLI products.
/// Checked in order; first match wins.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICES: &[&str] = &[
    "Claude Code-credentials",
    "Claude Cowork-credentials",
    "Claude-credentials",
];

// ============================================================================
// Public API
// ============================================================================

/// Attempt to read Claude CLI credentials and convert them to an [`OAuthTokenSet`].
///
/// Lookup order:
/// 1. macOS Keychain (if on macOS)
/// 2. `~/.claude.json`
///
/// Returns `None` if no valid credentials are found.
pub fn read_claude_credentials() -> Option<OAuthTokenSet> {
    // 1. Try macOS Keychain first
    #[cfg(target_os = "macos")]
    {
        if let Some(token_set) = read_from_keychain() {
            return Some(token_set);
        }
    }

    // 2. Fall back to ~/.claude.json
    if let Some(config_path) = resolve_config_path() {
        if let Some(token_set) = read_from_config_file(&config_path) {
            debug!("imported Claude credentials from ~/.claude.json");
            return Some(token_set);
        }
    }

    debug!("no Claude CLI credentials found");
    None
}

// ============================================================================
// Keychain reader (macOS only)
// ============================================================================

#[cfg(target_os = "macos")]
fn read_from_keychain() -> Option<OAuthTokenSet> {
    for service in KEYCHAIN_SERVICES {
        let output = std::process::Command::new("security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output();

        let output = match output {
            Ok(o) => o,
            Err(_) => continue,
        };

        if !output.status.success() {
            continue;
        }

        let json_str = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if json_str.is_empty() {
            continue;
        }

        let data: ClaudeKeychainData = match serde_json::from_str(&json_str) {
            Ok(d) => d,
            Err(e) => {
                warn!("failed to parse Claude Keychain data for {service}: {e}");
                continue;
            }
        };

        let oauth = match data.claude_ai_oauth {
            Some(o) => o,
            None => continue,
        };

        let access_token = match oauth.access_token.filter(|s| !s.is_empty()) {
            Some(t) => t,
            None => continue,
        };

        let refresh_token = oauth.refresh_token.filter(|s| !s.is_empty());
        let now = chrono::Utc::now().timestamp();
        let expires_at = oauth.expires_at.map(|ms| ms / 1000);

        debug!("imported Claude credentials from macOS Keychain (service: {service})");
        return Some(OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token,
            refresh_token,
            expires_at,
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: now,
            client_id: Some(super::CLAUDE_CODE_CLIENT_ID.to_string()),
        });
    }

    debug!("no Claude credentials found in macOS Keychain");
    None
}

// ============================================================================
// File reader (all platforms)
// ============================================================================

/// Read Claude credentials from a config file at the given path.
///
/// This is separated from path resolution so tests can pass an explicit path
/// without relying on the user's home directory.
fn read_from_config_file(path: &Path) -> Option<OAuthTokenSet> {
    if !path.exists() {
        debug!("Claude config file not found at {}", path.display());
        return None;
    }

    let file = std::fs::File::open(path)
        .map_err(|e| {
            warn!("failed to open {}: {e}", path.display());
            e
        })
        .ok()?;

    let reader = BufReader::new(file);
    let config: ClaudeConfigFile = serde_json::from_reader(reader)
        .map_err(|e| {
            warn!("failed to parse {}: {e}", path.display());
            e
        })
        .ok()?;

    let oauth = config.claude_ai_oauth?;
    let access_token = oauth.access_token.filter(|s| !s.is_empty())?;
    let refresh_token = oauth.refresh_token.filter(|s| !s.is_empty());
    let now = chrono::Utc::now().timestamp();
    // expiresAt is in milliseconds — convert to seconds
    let expires_at = oauth.expires_at.map(|ms| ms / 1000);

    Some(OAuthTokenSet {
        provider: "anthropic".to_string(),
        access_token,
        refresh_token,
        expires_at,
        token_type: "Bearer".to_string(),
        scope: None,
        obtained_at: now,
        client_id: Some(super::CLAUDE_CODE_CLIENT_ID.to_string()),
    })
}

// ============================================================================
// Helpers
// ============================================================================

/// Resolve the path to `~/.claude.json`.
fn resolve_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: create a temp dir, write a config file with given content, return (TempDir, path).
    fn write_config_file(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let config_path = tmp.path().join(".claude.json");
        let mut f = std::fs::File::create(&config_path).expect("create config file");
        f.write_all(content.as_bytes()).expect("write config");
        (tmp, config_path)
    }

    #[test]
    fn test_read_valid_oauth() {
        let now = chrono::Utc::now().timestamp();
        let (_tmp, path) = write_config_file(
            r#"{
                "claudeAiOauth": {
                    "accessToken": "sk-ant-abc123",
                    "refreshToken": "rt-def456",
                    "expiresAt": 1773074160794
                }
            }"#,
        );

        let result = read_from_config_file(&path);
        assert!(result.is_some(), "expected Some for valid config");

        let ts = result.unwrap();
        assert_eq!(ts.provider, "anthropic");
        assert_eq!(ts.access_token, "sk-ant-abc123");
        assert_eq!(ts.refresh_token.as_deref(), Some("rt-def456"));
        assert_eq!(ts.token_type, "Bearer");
        assert_eq!(
            ts.client_id.as_deref(),
            Some(super::super::CLAUDE_CODE_CLIENT_ID)
        );
        assert!(ts.scope.is_none());
        assert!(
            (ts.obtained_at - now).abs() < 5,
            "obtained_at should be close to now"
        );
    }

    #[test]
    fn test_missing_claude_ai_oauth_key() {
        let (_tmp, path) = write_config_file(r#"{"someOtherKey": "value"}"#);

        let result = read_from_config_file(&path);
        assert!(
            result.is_none(),
            "expected None when claudeAiOauth key is missing"
        );
    }

    #[test]
    fn test_missing_access_token() {
        let (_tmp, path) = write_config_file(
            r#"{
                "claudeAiOauth": {
                    "refreshToken": "rt-def456",
                    "expiresAt": 1773074160794
                }
            }"#,
        );

        let result = read_from_config_file(&path);
        assert!(
            result.is_none(),
            "expected None when accessToken is missing"
        );
    }

    #[test]
    fn test_empty_access_token() {
        let (_tmp, path) = write_config_file(
            r#"{
                "claudeAiOauth": {
                    "accessToken": "",
                    "refreshToken": "rt-def456",
                    "expiresAt": 1773074160794
                }
            }"#,
        );

        let result = read_from_config_file(&path);
        assert!(
            result.is_none(),
            "expected None when accessToken is empty string"
        );
    }

    #[test]
    fn test_missing_refresh_token() {
        let (_tmp, path) = write_config_file(
            r#"{
                "claudeAiOauth": {
                    "accessToken": "sk-ant-abc123",
                    "expiresAt": 1773074160794
                }
            }"#,
        );

        let result = read_from_config_file(&path);
        assert!(
            result.is_some(),
            "expected Some when only accessToken is present"
        );
        let ts = result.unwrap();
        assert_eq!(ts.access_token, "sk-ant-abc123");
        assert!(
            ts.refresh_token.is_none(),
            "refresh_token should be None when not in file"
        );
    }

    #[test]
    fn test_malformed_json() {
        let (_tmp, path) = write_config_file("not json at all {{{");

        let result = read_from_config_file(&path);
        assert!(result.is_none(), "expected None for malformed JSON");
    }

    #[test]
    fn test_file_does_not_exist() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let path = tmp.path().join(".claude.json");
        // Do not create the file

        let result = read_from_config_file(&path);
        assert!(
            result.is_none(),
            "expected None when config file does not exist"
        );
    }

    #[test]
    fn test_expires_at_millis_to_secs() {
        let millis: i64 = 1773074160794;
        let expected_secs = millis / 1000; // 1773074160

        let (_tmp, path) = write_config_file(&format!(
            r#"{{
                "claudeAiOauth": {{
                    "accessToken": "sk-ant-abc123",
                    "refreshToken": "rt-def456",
                    "expiresAt": {millis}
                }}
            }}"#,
        ));

        let result = read_from_config_file(&path);
        assert!(result.is_some());
        let ts = result.unwrap();
        assert_eq!(
            ts.expires_at,
            Some(expected_secs),
            "expiresAt should be converted from millis to secs"
        );
    }

    // ========================================================================
    // macOS Keychain tests
    // ========================================================================

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_services_non_empty() {
        assert!(
            !KEYCHAIN_SERVICES.is_empty(),
            "KEYCHAIN_SERVICES should not be empty"
        );
        assert!(
            KEYCHAIN_SERVICES.contains(&"Claude Code-credentials"),
            "should contain Claude Code service"
        );
        assert!(
            KEYCHAIN_SERVICES.contains(&"Claude Cowork-credentials"),
            "should contain Claude Cowork service"
        );
        assert!(
            KEYCHAIN_SERVICES.contains(&"Claude-credentials"),
            "should contain Claude service"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_invalid_service() {
        // Calling read_from_keychain should not panic even when no valid
        // Keychain entries exist. It should gracefully return None.
        // (On CI or machines without Claude installed, this always returns None.)
        let result = read_from_keychain();
        // We can't assert Some or None — depends on machine state.
        // The point is: no panic.
        let _ = result;
    }
}
