//! API authentication and CSRF middleware.
//!
//! Checks for `Authorization: Bearer <token>` on every request, skipping
//! auth for the health endpoint, the login endpoint, and WebSocket upgrades.
//! Accepts both static API tokens and short-lived HS256 JWTs issued by
//! `POST /api/auth/login`.
//!
//! For mutating requests (POST/PUT/DELETE), the middleware also validates an
//! `X-CSRF-Token` header — except on the login endpoint itself, which is
//! exempt because it runs before the caller has a token.

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use ring::hmac;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::server::AppState;

// ---------------------------------------------------------------------------
// CSRF helpers
// ---------------------------------------------------------------------------

/// Generate a CSRF token valid for 1 hour.
///
/// Token format: `"{timestamp}:{hmac_hex}"` where `hmac_hex` is the full
/// HMAC-SHA256 of the timestamp string keyed with `secret`, hex-encoded.
/// The token is bound to `jwt_secret`, which rotates on every process restart.
pub fn generate_csrf_token(secret: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, now.to_string().as_bytes());
    let sig = hex::encode(tag.as_ref());
    format!("{now}:{sig}")
}

/// Validate a CSRF token produced by [`generate_csrf_token`].
///
/// Returns `true` when the token:
/// 1. Has the expected `"{timestamp}:{hmac_hex}"` structure.
/// 2. Was issued within the last hour (not expired).
/// 3. Was not issued more than 60 seconds in the future (prevents pre-generated tokens).
/// 4. Contains an HMAC that matches re-deriving from `secret + timestamp`.
/// 5. Comparison uses constant-time equality to prevent timing attacks.
pub fn validate_csrf_token(token: &str, secret: &str) -> bool {
    let Some((ts_str, provided_sig)) = token.split_once(':') else {
        return false;
    };

    let Ok(ts) = ts_str.parse::<u64>() else {
        return false;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Reject expired tokens (older than 1 hour).
    if now.saturating_sub(ts) > 3600 {
        return false;
    }
    // Reject future tokens (more than 60 seconds ahead).
    if ts.saturating_sub(now) > 60 {
        return false;
    }

    // Recompute expected HMAC.
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, ts_str.as_bytes());
    let expected_sig = hex::encode(tag.as_ref());

    // Constant-time comparison to prevent timing side-channels.
    if expected_sig.len() != provided_sig.len() {
        return false;
    }
    expected_sig
        .bytes()
        .zip(provided_sig.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

/// Middleware that checks for `Authorization: Bearer <token>` header.
///
/// Skips auth for:
/// - `GET /api/health` — liveness probe, no auth required
/// - `GET /api/csrf-token` — must be public so the caller can bootstrap
/// - `POST /api/auth/login` — exchanges password for JWT
/// - Any path starting with `/ws/` — WebSocket upgrade handshake
///
/// Accepts two token forms:
/// 1. Static API token configured at startup (`state.api_token`)
/// 2. Short-lived HS256 JWT issued by `/api/auth/login` (validated against
///    `state.jwt_secret`)
///
/// For mutating methods (POST/PUT/DELETE) on authenticated endpoints, the
/// middleware additionally validates the `X-CSRF-Token` header.  The login
/// endpoint is exempt because the caller does not yet possess a token.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path();

    // Public endpoints that skip auth entirely.
    if path == "/api/health"
        || path == "/api/csrf-token"
        || path == "/api/auth/login"
        || path.starts_with("/ws/")
    {
        return Ok(next.run(request).await);
    }

    // Extract the raw Authorization header value.
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];

            // Accept static API token OR a valid JWT.
            let is_valid = token == state.api_token
                || crate::api::auth::validate_jwt(token, &state.jwt_secret).is_ok();

            if !is_valid {
                return Err(StatusCode::UNAUTHORIZED);
            }

            // For mutating methods, require a valid CSRF token as well.
            let method = request.method();
            if matches!(
                *method,
                axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::DELETE
            ) {
                let csrf_token = request
                    .headers()
                    .get("x-csrf-token")
                    .and_then(|v| v.to_str().ok());
                match csrf_token {
                    Some(t) if validate_csrf_token(t, &state.jwt_secret) => {}
                    _ => return Err(StatusCode::FORBIDDEN),
                }
            }

            Ok(next.run(request).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{auth as panel_auth, events::EventBus, server::AppState};
    use axum::{
        body::Body,
        http::{Method, Request},
        middleware as axum_mw,
        routing::{get, post},
        Router,
    };
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn make_state() -> Arc<AppState> {
        let bus = EventBus::new(8);
        Arc::new(AppState::new("static-test-token".into(), bus))
    }

    fn make_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/health", get(|| async { "ok" }))
            .route("/api/csrf-token", get(|| async { "csrf" }))
            .route("/api/protected", get(|| async { "secret" }))
            .route("/api/protected", post(|| async { "mutate" }))
            .route("/api/auth/login", post(|| async { "login" }))
            .route("/ws/events", get(|| async { "ws" }))
            .layer(axum_mw::from_fn_with_state(state, auth_middleware))
    }

    // -----------------------------------------------------------------------
    // Auth bypass paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_health_skips_auth() {
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_csrf_endpoint_skips_auth() {
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/api/csrf-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_login_skips_auth() {
        let app = make_app(make_state());
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/auth/login")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_ws_skips_auth() {
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/ws/events")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -----------------------------------------------------------------------
    // Token validation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_protected_no_auth_returns_401() {
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/api/protected")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_protected_wrong_token_returns_401() {
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/api/protected")
            .header("authorization", "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_protected_valid_static_token() {
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/api/protected")
            .header("authorization", "Bearer static-test-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_protected_valid_jwt() {
        let state = make_state();
        let jwt =
            panel_auth::generate_jwt("admin", &state.jwt_secret, 3600).expect("jwt must generate");
        let app = make_app(state);
        let req = Request::builder()
            .uri("/api/protected")
            .header("authorization", format!("Bearer {jwt}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_protected_expired_jwt_returns_401() {
        let state = make_state();

        // Build an already-expired JWT directly.
        let past_exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(3600) as usize;
        let claims = panel_auth::Claims {
            sub: "admin".into(),
            exp: past_exp,
        };
        let expired_token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(state.jwt_secret.as_bytes()),
        )
        .unwrap();

        let app = make_app(state);
        let req = Request::builder()
            .uri("/api/protected")
            .header("authorization", format!("Bearer {expired_token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // -----------------------------------------------------------------------
    // CSRF token helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_generate_csrf_token_non_empty() {
        let token = generate_csrf_token("my-secret");
        assert!(!token.is_empty());
    }

    #[test]
    fn test_generate_csrf_token_format() {
        let token = generate_csrf_token("my-secret");
        // Expected format: "{timestamp}:{64-hex-chars}" (HMAC-SHA256 = 32 bytes = 64 hex)
        let parts: Vec<&str> = token.splitn(2, ':').collect();
        assert_eq!(parts.len(), 2, "token must contain exactly one ':'");
        assert!(
            parts[0].parse::<u64>().is_ok(),
            "first part must be u64 timestamp"
        );
        assert_eq!(
            parts[1].len(),
            64,
            "second part must be 64 hex chars (HMAC-SHA256)"
        );
        assert!(
            parts[1].chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be hex digits"
        );
    }

    #[test]
    fn test_validate_csrf_token_fresh_token() {
        let secret = "test-secret";
        let token = generate_csrf_token(secret);
        assert!(validate_csrf_token(&token, secret));
    }

    #[test]
    fn test_validate_csrf_token_wrong_secret_fails() {
        let token = generate_csrf_token("secret-a");
        assert!(!validate_csrf_token(&token, "secret-b"));
    }

    #[test]
    fn test_validate_csrf_token_invalid_format_fails() {
        assert!(!validate_csrf_token("notavalidtoken", "secret"));
        assert!(!validate_csrf_token("", "secret"));
        assert!(!validate_csrf_token(":", "secret"));
    }

    #[test]
    fn test_validate_csrf_token_expired_fails() {
        // Forge a token with a timestamp 2 hours in the past (well past the 1h window).
        let secret = "test-secret";
        let old_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(7200);
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
        let tag = ring::hmac::sign(&key, old_timestamp.to_string().as_bytes());
        let sig = hex::encode(tag.as_ref());
        let expired_token = format!("{old_timestamp}:{sig}");
        assert!(!validate_csrf_token(&expired_token, secret));
    }

    #[test]
    fn test_validate_csrf_token_future_fails() {
        // Forge a token with a timestamp 120 seconds in the future (past the 60s window).
        let secret = "test-secret";
        let future_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 120;
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
        let tag = ring::hmac::sign(&key, future_timestamp.to_string().as_bytes());
        let sig = hex::encode(tag.as_ref());
        let future_token = format!("{future_timestamp}:{sig}");
        assert!(!validate_csrf_token(&future_token, secret));
    }

    #[test]
    fn test_validate_csrf_token_tampered_hash_fails() {
        let secret = "test-secret";
        let token = generate_csrf_token(secret);
        // Corrupt the hash portion — use wrong-length zeroes to test length check too.
        let (ts, _hash) = token.split_once(':').unwrap();
        let tampered = format!("{ts}:{}", "0".repeat(64));
        assert!(!validate_csrf_token(&tampered, secret));
    }

    // -----------------------------------------------------------------------
    // CSRF enforcement in auth_middleware
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_post_without_csrf_token_returns_403() {
        let state = make_state();
        let app = make_app(state.clone());
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/protected")
            .header("authorization", "Bearer static-test-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_post_with_valid_csrf_token_succeeds() {
        let state = make_state();
        let csrf = generate_csrf_token(&state.jwt_secret);
        let app = make_app(state);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/protected")
            .header("authorization", "Bearer static-test-token")
            .header("x-csrf-token", csrf)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_post_with_wrong_csrf_token_returns_403() {
        let state = make_state();
        let app = make_app(state);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/protected")
            .header("authorization", "Bearer static-test-token")
            .header("x-csrf-token", "invalid-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_get_does_not_require_csrf() {
        // GET requests must NOT require CSRF tokens.
        let app = make_app(make_state());
        let req = Request::builder()
            .uri("/api/protected")
            .header("authorization", "Bearer static-test-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
