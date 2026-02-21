//! OAuth authentication module for LLM providers.
//!
//! Provides browser-based OAuth 2.0 + PKCE authentication as an alternative
//! to API keys. OAuth tokens take priority over API keys when both are available.
//!
//! **Warning:** Using OAuth subscription tokens for API access may violate provider
//! Terms of Service. This module includes graceful fallback to API keys when
//! OAuth tokens are rejected.

pub mod oauth;
pub mod refresh;
pub mod store;

use serde::{Deserialize, Serialize};

/// Claude Code's OAuth client ID, used when importing subscription tokens
/// obtained via `claude auth token`.
pub const CLAUDE_CODE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

// ============================================================================
// Auth Method
// ============================================================================

/// Authentication method for a provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// Traditional API key authentication (default).
    #[default]
    #[serde(alias = "api_key")]
    ApiKey,
    /// OAuth 2.0 browser-based authentication.
    OAuth,
    /// Try OAuth first, fall back to API key.
    Auto,
}

impl AuthMethod {
    /// Parse from an optional string (as stored in config).
    pub fn from_option(s: Option<&str>) -> Self {
        match s {
            Some("oauth") => Self::OAuth,
            Some("auto") => Self::Auto,
            Some("api_key") | Some("apikey") => Self::ApiKey,
            _ => Self::ApiKey,
        }
    }
}

// ============================================================================
// Resolved Credential
// ============================================================================

/// A resolved credential ready for use in API calls.
///
/// This is the output of the credential resolution process that considers
/// OAuth tokens, API keys, and the configured auth method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCredential {
    /// Traditional API key.
    ApiKey(String),
    /// OAuth Bearer token with optional expiry.
    BearerToken {
        access_token: String,
        expires_at: Option<i64>,
    },
}

impl ResolvedCredential {
    /// Returns the credential value (API key or access token).
    pub fn value(&self) -> &str {
        match self {
            Self::ApiKey(key) => key,
            Self::BearerToken { access_token, .. } => access_token,
        }
    }

    /// Returns `true` if this is a Bearer token credential.
    pub fn is_bearer(&self) -> bool {
        matches!(self, Self::BearerToken { .. })
    }

    /// Returns `true` if this is an API key credential.
    pub fn is_api_key(&self) -> bool {
        matches!(self, Self::ApiKey(_))
    }
}

// ============================================================================
// OAuth Token Set
// ============================================================================

/// Stored OAuth token set for a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenSet {
    /// Provider name (e.g., "anthropic").
    pub provider: String,
    /// OAuth access token.
    pub access_token: String,
    /// OAuth refresh token (for renewing expired access tokens).
    pub refresh_token: Option<String>,
    /// Unix timestamp when the access token expires.
    pub expires_at: Option<i64>,
    /// Token type (typically "Bearer").
    pub token_type: String,
    /// Granted scopes.
    pub scope: Option<String>,
    /// Unix timestamp when the token was obtained.
    pub obtained_at: i64,
    /// Client ID used for this OAuth session (needed for refresh).
    pub client_id: Option<String>,
}

impl OAuthTokenSet {
    /// Returns `true` if the access token has expired.
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            now >= expires_at
        } else {
            false // No expiry = never expires
        }
    }

    /// Returns `true` if the token will expire within the given number of seconds.
    pub fn expires_within(&self, seconds: i64) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            now + seconds >= expires_at
        } else {
            false
        }
    }

    /// Returns a human-readable description of time until expiry.
    pub fn expires_in_human(&self) -> String {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            let remaining = expires_at - now;
            if remaining <= 0 {
                "expired".to_string()
            } else if remaining < 60 {
                format!("{}s", remaining)
            } else if remaining < 3600 {
                format!("{}m", remaining / 60)
            } else {
                format!("{}h {}m", remaining / 3600, (remaining % 3600) / 60)
            }
        } else {
            "no expiry".to_string()
        }
    }
}

// ============================================================================
// Provider OAuth configuration
// ============================================================================

/// Provider-specific OAuth configuration.
#[derive(Debug, Clone)]
pub struct ProviderOAuthConfig {
    /// Provider name.
    pub provider: String,
    /// OAuth token endpoint URL.
    pub token_url: String,
    /// OAuth authorization endpoint URL.
    pub authorize_url: String,
    /// Client name for dynamic registration.
    pub client_name: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
}

/// Returns the OAuth configuration for a supported provider.
///
/// Note: Some providers may not have a publicly available OAuth flow for API
/// access yet. If a provider's OAuth endpoints or behavior change upstream,
/// `auth login` may fail and users should fall back to API key authentication.
pub fn provider_oauth_config(provider: &str) -> Option<ProviderOAuthConfig> {
    match provider {
        "anthropic" => Some(ProviderOAuthConfig {
            provider: "anthropic".to_string(),
            token_url: "https://console.anthropic.com/v1/oauth/token".to_string(),
            authorize_url: "https://console.anthropic.com/oauth/authorize".to_string(),
            client_name: "ZeptoClaw".to_string(),
            scopes: vec![],
        }),
        _ => None,
    }
}

/// Returns a list of providers that support OAuth authentication.
pub fn oauth_supported_providers() -> &'static [&'static str] {
    &["anthropic"]
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_method_default() {
        assert_eq!(AuthMethod::default(), AuthMethod::ApiKey);
    }

    #[test]
    fn test_auth_method_from_option() {
        assert_eq!(AuthMethod::from_option(None), AuthMethod::ApiKey);
        assert_eq!(AuthMethod::from_option(Some("oauth")), AuthMethod::OAuth);
        assert_eq!(AuthMethod::from_option(Some("auto")), AuthMethod::Auto);
        assert_eq!(AuthMethod::from_option(Some("api_key")), AuthMethod::ApiKey);
        assert_eq!(AuthMethod::from_option(Some("apikey")), AuthMethod::ApiKey);
        assert_eq!(AuthMethod::from_option(Some("unknown")), AuthMethod::ApiKey);
    }

    #[test]
    fn test_auth_method_serde_roundtrip() {
        let methods = vec![AuthMethod::ApiKey, AuthMethod::OAuth, AuthMethod::Auto];
        for method in methods {
            let json = serde_json::to_string(&method).unwrap();
            let parsed: AuthMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, method);
        }
    }

    #[test]
    fn test_resolved_credential_api_key() {
        let cred = ResolvedCredential::ApiKey("sk-test".to_string());
        assert!(cred.is_api_key());
        assert!(!cred.is_bearer());
        assert_eq!(cred.value(), "sk-test");
    }

    #[test]
    fn test_resolved_credential_bearer() {
        let cred = ResolvedCredential::BearerToken {
            access_token: "token-123".to_string(),
            expires_at: Some(9999999999),
        };
        assert!(cred.is_bearer());
        assert!(!cred.is_api_key());
        assert_eq!(cred.value(), "token-123");
    }

    #[test]
    fn test_token_set_not_expired() {
        let token = OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now().timestamp() + 3600),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp(),
            client_id: None,
        };
        assert!(!token.is_expired());
        assert!(!token.expires_within(300));
    }

    #[test]
    fn test_token_set_expired() {
        let token = OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now().timestamp() - 100),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp() - 4000,
            client_id: None,
        };
        assert!(token.is_expired());
    }

    #[test]
    fn test_token_set_expires_within() {
        let token = OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now().timestamp() + 200),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp(),
            client_id: None,
        };
        assert!(!token.is_expired());
        assert!(token.expires_within(300)); // 200s left < 300s threshold
        assert!(!token.expires_within(100)); // 200s left > 100s threshold
    }

    #[test]
    fn test_token_set_no_expiry() {
        let token = OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp(),
            client_id: None,
        };
        assert!(!token.is_expired());
        assert!(!token.expires_within(99999));
        assert_eq!(token.expires_in_human(), "no expiry");
    }

    #[test]
    fn test_expires_in_human() {
        let now = chrono::Utc::now().timestamp();

        let token = OAuthTokenSet {
            provider: "test".to_string(),
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: Some(now - 10),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: now,
            client_id: None,
        };
        assert_eq!(token.expires_in_human(), "expired");

        let token2 = OAuthTokenSet {
            expires_at: Some(now + 7200 + 1800),
            ..token.clone()
        };
        assert!(token2.expires_in_human().contains("h"));
    }

    #[test]
    fn test_provider_oauth_config_anthropic() {
        let config = provider_oauth_config("anthropic");
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.provider, "anthropic");
        assert!(config.token_url.contains("anthropic.com"));
    }

    #[test]
    fn test_provider_oauth_config_unsupported() {
        assert!(provider_oauth_config("openai").is_none());
        assert!(provider_oauth_config("unknown").is_none());
    }

    #[test]
    fn test_oauth_supported_providers() {
        let providers = oauth_supported_providers();
        assert!(providers.contains(&"anthropic"));
        assert!(!providers.contains(&"openai"));
    }
}
