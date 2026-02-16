//! Encrypted token persistence for OAuth tokens.
//!
//! Stores OAuth token sets encrypted at rest using the existing
//! XChaCha20-Poly1305 infrastructure from `security::encryption`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{Result, ZeptoError};
use crate::security::encryption::SecretEncryption;

use super::OAuthTokenSet;

// ============================================================================
// Token Store
// ============================================================================

/// Encrypted OAuth token store.
///
/// Persists tokens as JSON encrypted with XChaCha20-Poly1305 at
/// `~/.zeptoclaw/auth/tokens.json.enc`. Each provider's token set is
/// stored as a separate entry keyed by provider name.
pub struct TokenStore {
    path: PathBuf,
    encryption: SecretEncryption,
}

/// Internal structure for the tokens file (before encryption).
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct TokensFile {
    tokens: HashMap<String, OAuthTokenSet>,
}

impl TokenStore {
    /// Create a new token store with the given encryption key.
    ///
    /// The store file is located at `~/.zeptoclaw/auth/tokens.json.enc`.
    pub fn new(encryption: SecretEncryption) -> Self {
        let path = crate::config::Config::dir()
            .join("auth")
            .join("tokens.json.enc");
        Self { path, encryption }
    }

    /// Create a token store at a custom path (for testing).
    pub fn with_path(path: PathBuf, encryption: SecretEncryption) -> Self {
        Self { path, encryption }
    }

    /// Load a token set for a specific provider.
    pub fn load(&self, provider: &str) -> Result<Option<OAuthTokenSet>> {
        let file = self.load_file()?;
        Ok(file.tokens.get(provider).cloned())
    }

    /// Save a token set for a provider.
    pub fn save(&self, tokens: &OAuthTokenSet) -> Result<()> {
        let mut file = self.load_file()?;
        file.tokens.insert(tokens.provider.clone(), tokens.clone());
        self.save_file(&file)
    }

    /// Delete stored tokens for a provider.
    pub fn delete(&self, provider: &str) -> Result<bool> {
        let mut file = self.load_file()?;
        let removed = file.tokens.remove(provider).is_some();
        if removed {
            self.save_file(&file)?;
        }
        Ok(removed)
    }

    /// List all stored provider names and their token summaries.
    pub fn list(&self) -> Result<Vec<(String, TokenSummary)>> {
        let file = self.load_file()?;
        let mut entries: Vec<(String, TokenSummary)> = file
            .tokens
            .iter()
            .map(|(name, token)| {
                (
                    name.clone(),
                    TokenSummary {
                        is_expired: token.is_expired(),
                        expires_in: token.expires_in_human(),
                        has_refresh_token: token.refresh_token.is_some(),
                        obtained_at: token.obtained_at,
                    },
                )
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    /// Returns `true` if the store file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    fn load_file(&self) -> Result<TokensFile> {
        if !self.path.exists() {
            return Ok(TokensFile::default());
        }

        let encrypted = std::fs::read_to_string(&self.path).map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to read token store at {:?}: {}",
                self.path, e
            ))
        })?;

        if encrypted.trim().is_empty() {
            return Ok(TokensFile::default());
        }

        let json = self.encryption.decrypt(encrypted.trim())?;
        serde_json::from_str(&json)
            .map_err(|e| ZeptoError::Config(format!("Failed to parse token store: {}", e)))
    }

    fn save_file(&self, file: &TokensFile) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ZeptoError::Config(format!(
                    "Failed to create auth directory {:?}: {}",
                    parent, e
                ))
            })?;
        }

        let json = serde_json::to_string_pretty(file)
            .map_err(|e| ZeptoError::Config(format!("Failed to serialize tokens: {}", e)))?;

        let encrypted = self.encryption.encrypt(&json)?;

        std::fs::write(&self.path, &encrypted).map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to write token store at {:?}: {}",
                self.path, e
            ))
        })?;

        // Restrict permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }
}

/// Summary of a stored token (no secrets exposed).
#[derive(Debug, Clone)]
pub struct TokenSummary {
    /// Whether the access token has expired.
    pub is_expired: bool,
    /// Human-readable time until expiry.
    pub expires_in: String,
    /// Whether a refresh token is available.
    pub has_refresh_token: bool,
    /// When the token was obtained (unix timestamp).
    pub obtained_at: i64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_encryption() -> SecretEncryption {
        SecretEncryption::from_raw_key(&[0x42u8; 32])
    }

    fn test_token(provider: &str) -> OAuthTokenSet {
        OAuthTokenSet {
            provider: provider.to_string(),
            access_token: format!("access-token-{}", provider),
            refresh_token: Some(format!("refresh-token-{}", provider)),
            expires_at: Some(chrono::Utc::now().timestamp() + 28800),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp(),
            client_id: Some("test-client-id".to_string()),
        }
    }

    #[test]
    fn test_store_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = TokenStore::with_path(path, test_encryption());

        // Initially empty
        assert!(store.load("anthropic").unwrap().is_none());

        // Save and load
        let token = test_token("anthropic");
        store.save(&token).unwrap();

        let loaded = store
            .load("anthropic")
            .unwrap()
            .expect("token should exist");
        assert_eq!(loaded.access_token, "access-token-anthropic");
        assert_eq!(loaded.provider, "anthropic");
        assert!(loaded.refresh_token.is_some());
    }

    #[test]
    fn test_store_multiple_providers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = TokenStore::with_path(path, test_encryption());

        store.save(&test_token("anthropic")).unwrap();
        store.save(&test_token("openai")).unwrap();

        assert!(store.load("anthropic").unwrap().is_some());
        assert!(store.load("openai").unwrap().is_some());
        assert!(store.load("groq").unwrap().is_none());
    }

    #[test]
    fn test_store_overwrite() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = TokenStore::with_path(path, test_encryption());

        let mut token = test_token("anthropic");
        store.save(&token).unwrap();

        token.access_token = "updated-token".to_string();
        store.save(&token).unwrap();

        let loaded = store.load("anthropic").unwrap().unwrap();
        assert_eq!(loaded.access_token, "updated-token");
    }

    #[test]
    fn test_store_delete() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = TokenStore::with_path(path, test_encryption());

        store.save(&test_token("anthropic")).unwrap();
        assert!(store.load("anthropic").unwrap().is_some());

        let removed = store.delete("anthropic").unwrap();
        assert!(removed);
        assert!(store.load("anthropic").unwrap().is_none());

        // Deleting non-existent returns false
        let removed = store.delete("anthropic").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_store_list() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = TokenStore::with_path(path, test_encryption());

        store.save(&test_token("anthropic")).unwrap();
        store.save(&test_token("openai")).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "anthropic");
        assert_eq!(list[1].0, "openai");
        assert!(!list[0].1.is_expired);
        assert!(list[0].1.has_refresh_token);
    }

    #[test]
    fn test_store_empty_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");

        // Create empty file
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(&path, "").unwrap();

        let store = TokenStore::with_path(path, test_encryption());
        assert!(store.load("anthropic").unwrap().is_none());
    }

    #[test]
    fn test_store_wrong_key_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");

        // Save with one key
        let store1 = TokenStore::with_path(path.clone(), test_encryption());
        store1.save(&test_token("anthropic")).unwrap();

        // Try to load with different key
        let store2 = TokenStore::with_path(path, SecretEncryption::from_raw_key(&[0x99u8; 32]));
        let result = store2.load("anthropic");
        assert!(result.is_err());
    }

    #[test]
    fn test_store_exists() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = TokenStore::with_path(path, test_encryption());

        assert!(!store.exists());
        store.save(&test_token("anthropic")).unwrap();
        assert!(store.exists());
    }
}
