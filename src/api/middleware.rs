//! API authentication middleware.
//!
//! Checks for `Authorization: Bearer <token>` on every request, skipping
//! auth for the health endpoint, the login endpoint, and WebSocket upgrades.
//! Accepts both static API tokens and short-lived HS256 JWTs issued by
//! `POST /api/auth/login`.

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use super::server::AppState;

/// Middleware that checks for `Authorization: Bearer <token>` header.
///
/// Skips auth for:
/// - `GET /api/health` — liveness probe, no auth required
/// - `POST /api/auth/login` — exchanges password for JWT
/// - Any path starting with `/ws/` — WebSocket upgrade handshake
///
/// Accepts two token forms:
/// 1. Static API token configured at startup (`state.api_token`)
/// 2. Short-lived HS256 JWT issued by `/api/auth/login` (validated against
///    `state.jwt_secret`)
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path();

    // Skip auth for health, login, and WebSocket upgrade paths.
    if path == "/api/health" || path == "/api/auth/login" || path.starts_with("/ws/") {
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

            if is_valid {
                Ok(next.run(request).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
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
    use axum::{body::Body, http::Request, middleware as axum_mw, routing::get, Router};
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn make_state() -> Arc<AppState> {
        let bus = EventBus::new(8);
        Arc::new(AppState {
            api_token: "static-test-token".into(),
            event_bus: bus,
            password_hash: None,
            jwt_secret: "test-jwt-secret".into(),
        })
    }

    fn make_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/health", get(|| async { "ok" }))
            .route("/api/protected", get(|| async { "secret" }))
            .route("/api/auth/login", get(|| async { "login" }))
            .route("/ws/events", get(|| async { "ws" }))
            .layer(axum_mw::from_fn_with_state(state, auth_middleware))
    }

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
    async fn test_login_skips_auth() {
        let app = make_app(make_state());
        let req = Request::builder()
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
}
