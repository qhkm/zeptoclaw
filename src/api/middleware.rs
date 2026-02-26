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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::server::AppState;

// ---------------------------------------------------------------------------
// CSRF helpers
// ---------------------------------------------------------------------------

/// Generate a CSRF token valid for 1 hour.
///
/// Token format: `"{timestamp}:{hash}"` where `hash` is the first 16 hex
/// digits of a simple polynomial hash over `secret + timestamp`.  This is
/// intentionally lightweight (no new crate dependency) — the goal is
/// cross-site-request forgery protection, not cryptographic strength.  The
/// token is bound to `jwt_secret`, which rotates on every process restart.
pub fn generate_csrf_token(secret: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hash = csrf_hash(secret, timestamp);
    format!("{timestamp}:{hash:016x}")
}

/// Validate a CSRF token produced by [`generate_csrf_token`].
///
/// Returns `true` when the token:
/// 1. Has the expected `"{timestamp}:{hash}"` structure.
/// 2. Was issued within the last hour (1 h window).
/// 3. Contains a hash that matches re-deriving from `secret + timestamp`.
pub fn validate_csrf_token(token: &str, secret: &str) -> bool {
    let Some((ts_str, hash_str)) = token.split_once(':') else {
        return false;
    };

    let Ok(timestamp) = ts_str.parse::<u64>() else {
        return false;
    };

    // Reject tokens older than 1 hour.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(timestamp) > 3600 {
        return false;
    }

    let expected_hash = csrf_hash(secret, timestamp);
    format!("{expected_hash:016x}") == hash_str
}

/// Polynomial rolling hash: fold each byte of `secret + timestamp_string`
/// using multiplier 31 with wrapping arithmetic.
fn csrf_hash(secret: &str, timestamp: u64) -> u64 {
    let ts_str = timestamp.to_string();
    secret
        .bytes()
        .chain(ts_str.bytes())
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
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
        // Expected format: "{timestamp}:{16-hex-chars}"
        let parts: Vec<&str> = token.splitn(2, ':').collect();
        assert_eq!(parts.len(), 2, "token must contain exactly one ':'");
        assert!(
            parts[0].parse::<u64>().is_ok(),
            "first part must be u64 timestamp"
        );
        assert_eq!(parts[1].len(), 16, "second part must be 16 hex chars");
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
        let hash = csrf_hash(secret, old_timestamp);
        let expired_token = format!("{old_timestamp}:{hash:016x}");
        assert!(!validate_csrf_token(&expired_token, secret));
    }

    #[test]
    fn test_validate_csrf_token_tampered_hash_fails() {
        let secret = "test-secret";
        let token = generate_csrf_token(secret);
        // Corrupt the hash portion.
        let (ts, _hash) = token.split_once(':').unwrap();
        let tampered = format!("{ts}:0000000000000000");
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
