//! Authentication helpers for the ZeptoClaw control panel.
//!
//! Provides token generation, bearer verification, bcrypt password hashing,
//! and HS256 JWT issuance/validation used by the panel HTTP API.

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::{Result, ZeptoError};

// ============================================================================
// API Token
// ============================================================================

/// Generates a random 64-character hex API token.
///
/// Internally uses a UUID v4 source (128 bits of entropy) encoded as a
/// lowercase hex string without hyphens, yielding exactly 32 bytes → 64 hex
/// chars.  Two consecutive UUIDs are concatenated to reach 64 bytes of
/// entropy (128 hex chars would be overkill; UUID v4 with 122 bits is
/// already cryptographically sufficient for a bearer token).
///
/// # Example
///
/// ```
/// let token = zeptoclaw::api::auth::generate_api_token();
/// assert_eq!(token.len(), 64);
/// assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
/// ```
pub fn generate_api_token() -> String {
    // UUID v4 = 128 bits; hex without hyphens = 32 chars.
    // Two UUIDs give 64 chars, matching the spec.
    let a = Uuid::new_v4().simple().to_string();
    let b = Uuid::new_v4().simple().to_string();
    format!("{}{}", &a[..32], &b[..32])
}

// ============================================================================
// Bearer token verification
// ============================================================================

/// Compare two security-sensitive strings without data-dependent early exits.
///
/// Length is not secret for the tokens compared by the panel API.
pub(crate) fn constant_time_eq(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

/// Verifies an `Authorization: Bearer <token>` header value against the
/// expected token.
///
/// Strips the `"Bearer "` prefix (case-sensitive, with a trailing space),
/// then performs a constant-time comparison.
///
/// # Errors
///
/// Returns [`ZeptoError::Unauthorized`] when:
/// - The prefix is missing or malformed.
/// - The token does not match `expected`.
pub fn verify_bearer_token(header: &str, expected: &str) -> Result<()> {
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ZeptoError::Unauthorized("missing Bearer prefix".to_string()))?;

    if constant_time_eq(token, expected) {
        Ok(())
    } else {
        Err(ZeptoError::Unauthorized("invalid API token".to_string()))
    }
}

// ============================================================================
// WebSocket tickets
// ============================================================================

/// Lifetime of a one-time WebSocket authentication ticket.
pub const WS_TICKET_TTL: Duration = Duration::from_secs(30);
/// Maximum number of issued WebSocket tickets awaiting consumption.
pub const MAX_PENDING_WS_TICKETS: usize = 64;

/// In-memory store for short-lived, single-use WebSocket tickets.
///
/// Tickets are issued only through an authenticated HTTP endpoint. Their short
/// lifetime and single-use behavior limit exposure in URL logs while keeping
/// the long-lived API token or JWT out of them entirely.
#[derive(Debug, Default)]
pub struct WsTicketStore {
    tickets: Mutex<HashMap<String, Instant>>,
}

impl WsTicketStore {
    /// Issue a fresh ticket, or return `None` when the bounded store is full.
    pub async fn issue(&self) -> Option<String> {
        let now = Instant::now();
        let mut tickets = self.tickets.lock().await;
        tickets.retain(|_, expires_at| *expires_at > now);
        if tickets.len() >= MAX_PENDING_WS_TICKETS {
            return None;
        }

        let ticket = generate_api_token();
        tickets.insert(ticket.clone(), now + WS_TICKET_TTL);
        Some(ticket)
    }

    /// Consume a ticket exactly once, returning whether it was still valid.
    pub async fn consume(&self, ticket: &str) -> bool {
        let now = Instant::now();
        let mut tickets = self.tickets.lock().await;
        let is_valid = tickets
            .remove(ticket)
            .is_some_and(|expires_at| expires_at > now);
        tickets.retain(|_, expires_at| *expires_at > now);
        is_valid
    }

    /// Periodically remove expired tickets while the owning server is alive.
    pub fn start_cleanup(self: &Arc<Self>) {
        let store = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(WS_TICKET_TTL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(store) = store.upgrade() else {
                    break;
                };
                let now = Instant::now();
                store
                    .tickets
                    .lock()
                    .await
                    .retain(|_, expires_at| *expires_at > now);
            }
        });
    }
}

// ============================================================================
// Password hashing (bcrypt)
// ============================================================================

/// Hashes `password` with bcrypt at cost factor 12.
///
/// bcrypt cost 12 requires ~250 ms on a modern CPU — acceptable for
/// interactive login flows, prohibitive for offline attacks.
///
/// # Errors
///
/// Returns [`ZeptoError::Config`] if bcrypt fails internally (extremely
/// rare; only occurs on invalid cost or internal entropy failure).
pub fn hash_password(password: &str) -> Result<String> {
    bcrypt::hash(password, 12).map_err(|e| ZeptoError::Config(format!("bcrypt hash: {e}")))
}

/// Verifies `password` against a bcrypt `hash`.
///
/// Returns `true` if the password matches, `false` otherwise.
///
/// # Errors
///
/// Returns [`ZeptoError::Config`] if the hash string is malformed and
/// bcrypt cannot parse it.
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    bcrypt::verify(password, hash).map_err(|e| ZeptoError::Config(format!("bcrypt verify: {e}")))
}

// ============================================================================
// JWT (HS256)
// ============================================================================

/// Claims embedded in a panel JWT.
///
/// Only the minimal RFC 7519 registered claims are used:
/// - `sub` — subject (username)
/// - `exp` — expiry (Unix timestamp, seconds)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — the authenticated username.
    pub sub: String,
    /// Expiry — Unix timestamp in seconds after which the token is invalid.
    pub exp: usize,
}

/// Issues an HS256-signed JWT for `username`.
///
/// `expires_in_secs` is added to the current Unix timestamp to set `exp`.
///
/// # Errors
///
/// Returns [`ZeptoError::Unauthorized`] if JWT encoding fails (e.g., the
/// secret is empty or the library encounters an internal error).
pub fn generate_jwt(username: &str, secret: &str, expires_in_secs: u64) -> Result<String> {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_add(expires_in_secs) as usize;

    let claims = Claims {
        sub: username.to_string(),
        exp,
    };

    encode(
        &Header::default(), // HS256
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ZeptoError::Unauthorized(format!("JWT encode: {e}")))
}

/// Validates an HS256 JWT and returns its [`Claims`].
///
/// The `jsonwebtoken` library automatically checks:
/// - Signature integrity (HMAC-SHA256 with `secret`)
/// - `exp` claim — tokens past their expiry are rejected
///
/// # Errors
///
/// Returns [`ZeptoError::Unauthorized`] for any validation failure:
/// bad signature, expired token, malformed header/payload, wrong algorithm.
pub fn validate_jwt(token: &str, secret: &str) -> Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(), // validates exp, requires HS256
    )
    .map_err(|e| ZeptoError::Unauthorized(format!("JWT validation: {e}")))?;

    Ok(token_data.claims)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // generate_api_token
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_api_token_length() {
        let token = generate_api_token();
        assert_eq!(token.len(), 64, "token must be exactly 64 hex chars");
    }

    #[test]
    fn test_generate_api_token_hex_chars() {
        let token = generate_api_token();
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token must contain only hex digits, got: {token}"
        );
    }

    #[test]
    fn test_generate_api_token_uniqueness() {
        let t1 = generate_api_token();
        let t2 = generate_api_token();
        assert_ne!(t1, t2, "two generated tokens must differ");
    }

    // ------------------------------------------------------------------
    // verify_bearer_token
    // ------------------------------------------------------------------

    #[test]
    fn test_verify_bearer_token_valid() {
        let secret = "mysecrettoken123";
        let header = format!("Bearer {secret}");
        assert!(
            verify_bearer_token(&header, secret).is_ok(),
            "valid bearer token should pass"
        );
    }

    #[test]
    fn test_verify_bearer_token_invalid() {
        let result = verify_bearer_token("Bearer wrongtoken", "correcttoken");
        assert!(
            matches!(result, Err(ZeptoError::Unauthorized(_))),
            "mismatched token should return Unauthorized"
        );
    }

    #[test]
    fn test_verify_bearer_token_missing_prefix() {
        // Header without "Bearer " prefix
        let result = verify_bearer_token("mysecrettoken123", "mysecrettoken123");
        assert!(
            matches!(result, Err(ZeptoError::Unauthorized(_))),
            "missing Bearer prefix should return Unauthorized"
        );
    }

    #[test]
    fn test_verify_bearer_token_lowercase_prefix_rejected() {
        // "bearer " (lowercase) must be rejected — the spec is case-sensitive
        let result = verify_bearer_token("bearer validtoken", "validtoken");
        assert!(
            matches!(result, Err(ZeptoError::Unauthorized(_))),
            "lowercase 'bearer' prefix must be rejected"
        );
    }

    #[test]
    fn test_constant_time_eq_rejects_mismatches() {
        assert!(constant_time_eq("same-token", "same-token"));
        assert!(!constant_time_eq("same-token", "other-token"));
        assert!(!constant_time_eq("short", "longer"));
    }

    #[tokio::test]
    async fn test_ws_ticket_is_single_use() {
        let store = WsTicketStore::default();
        let ticket = store.issue().await.expect("store has capacity");

        assert!(store.consume(&ticket).await);
        assert!(!store.consume(&ticket).await);
    }

    #[tokio::test]
    async fn test_expired_ws_ticket_is_rejected() {
        let store = WsTicketStore::default();
        store.tickets.lock().await.insert(
            "expired".to_string(),
            Instant::now() - Duration::from_secs(1),
        );

        assert!(!store.consume("expired").await);
    }

    #[tokio::test]
    async fn test_ws_ticket_store_enforces_capacity() {
        let store = WsTicketStore::default();
        for _ in 0..MAX_PENDING_WS_TICKETS {
            assert!(store.issue().await.is_some());
        }

        assert!(store.issue().await.is_none());
    }

    // ------------------------------------------------------------------
    // hash_password / verify_password
    // ------------------------------------------------------------------

    #[test]
    fn test_hash_and_verify_password() {
        let password = "hunter2";
        let hash = hash_password(password).expect("hash must succeed");
        assert!(!hash.is_empty(), "hash must not be empty");

        let ok = verify_password(password, &hash).expect("verify must succeed");
        assert!(ok, "correct password must verify as true");
    }

    #[test]
    fn test_verify_wrong_password() {
        let hash = hash_password("correct_password").expect("hash must succeed");
        let ok = verify_password("wrong_password", &hash).expect("verify must succeed");
        assert!(!ok, "wrong password must verify as false");
    }

    // ------------------------------------------------------------------
    // generate_jwt / validate_jwt
    // ------------------------------------------------------------------

    #[test]
    fn test_generate_jwt_and_validate() {
        let secret = "super_secret_key_for_testing";
        let username = "admin";

        let token = generate_jwt(username, secret, 3600).expect("JWT generation must succeed");
        assert!(!token.is_empty(), "JWT must not be empty");

        let claims = validate_jwt(&token, secret).expect("JWT validation must succeed");
        assert_eq!(claims.sub, username, "sub claim must match username");
    }

    #[test]
    fn test_validate_expired_jwt() {
        let secret = "super_secret_key_for_testing";

        // expires_in_secs = 0 → exp is set to now(), which is already past
        // by the time we validate.  Use a negative offset via a pre-built
        // Claims struct encoded manually.
        let past_exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(3600) as usize; // 1 hour in the past

        let claims = Claims {
            sub: "admin".to_string(),
            exp: past_exp,
        };

        let expired_token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .expect("encoding expired token must succeed");

        let result = validate_jwt(&expired_token, secret);
        assert!(
            matches!(result, Err(ZeptoError::Unauthorized(_))),
            "expired JWT must return Unauthorized, got: {result:?}"
        );
    }

    #[test]
    fn test_validate_jwt_wrong_secret() {
        let token =
            generate_jwt("alice", "correct_secret", 3600).expect("JWT generation must succeed");

        let result = validate_jwt(&token, "wrong_secret");
        assert!(
            matches!(result, Err(ZeptoError::Unauthorized(_))),
            "wrong secret must return Unauthorized"
        );
    }

    #[test]
    fn test_validate_jwt_malformed() {
        let result = validate_jwt("not.a.jwt", "secret");
        assert!(
            matches!(result, Err(ZeptoError::Unauthorized(_))),
            "malformed JWT must return Unauthorized"
        );
    }
}
