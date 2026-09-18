//! Auth routes for the panel API.
//!
//! Provides `POST /api/auth/login` which exchanges a valid password for a
//! short-lived HS256 JWT.  The JWT is subsequently accepted by the auth
//! middleware on all protected endpoints.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::server::AppState;

// ============================================================================
// Request / Response types
// ============================================================================

/// Request body for `POST /api/auth/login`.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

/// Successful response from `POST /api/auth/login`.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    /// HS256 JWT valid for 24 hours.
    pub token: String,
}

/// Successful response from `POST /api/auth/ws-ticket`.
#[derive(Debug, Serialize)]
pub struct WsTicketResponse {
    /// Random ticket accepted once by `/ws/events` for 30 seconds.
    pub ticket: String,
}

// ============================================================================
// Handler
// ============================================================================

/// `POST /api/auth/login` — exchange a password for a JWT.
///
/// If `AppState.password_hash` is `None`, password-based login is not
/// configured and the endpoint returns 404 so callers can fall back to
/// supplying a static API token directly.
///
/// On success returns a 24-hour HS256 JWT that is accepted by all protected
/// endpoints alongside the static API token.
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    match &state.password_hash {
        Some(hash) => {
            let ok = crate::api::auth::verify_password(&body.password, hash)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            if ok {
                let token = crate::api::auth::generate_jwt("admin", &state.jwt_secret, 86_400)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                Ok(Json(LoginResponse { token }))
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        // Password auth is not configured — callers must use a static API token.
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// `POST /api/auth/ws-ticket` — issue a short-lived, single-use WebSocket ticket.
///
/// The normal auth and CSRF middleware protect this endpoint. This exchange
/// prevents the caller's long-lived API token or JWT from appearing in a URL.
pub async fn issue_ws_ticket(State(state): State<Arc<AppState>>) -> axum::response::Response {
    match state.ws_tickets.issue().await {
        Some(ticket) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(WsTicketResponse { ticket }),
        )
            .into_response(),
        None => (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::RETRY_AFTER, "30"),
            ],
            "Too many pending WebSocket tickets",
        )
            .into_response(),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{auth as panel_auth, events::EventBus, server::build_router};
    use axum::{body::Body, extract::ConnectInfo, http::Request, routing::post, Router};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn make_state_with_password(password: &str) -> Arc<AppState> {
        let hash = panel_auth::hash_password(password).expect("hash must succeed");
        let bus = EventBus::new(8);
        let mut state = AppState::new("tok".into(), bus);
        state.password_hash = Some(hash);
        Arc::new(state)
    }

    fn make_state_no_password() -> Arc<AppState> {
        let bus = EventBus::new(8);
        Arc::new(AppState::new("tok".into(), bus))
    }

    fn make_app(state: Arc<AppState>) -> Router {
        build_router((*state).clone(), None, None)
    }

    fn login_request(peer: SocketAddr, password: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .extension(ConnectInfo(peer))
            .body(Body::from(
                serde_json::json!({ "password": password }).to_string(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn test_login_correct_password_returns_token() {
        let state = make_state_with_password("hunter2");
        let app = make_app(state.clone());
        let body = serde_json::json!({ "password": "hunter2" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))))
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["token"].as_str().is_some(), "token must be present");

        // Validate the returned JWT.
        let token = json["token"].as_str().unwrap();
        let claims =
            panel_auth::validate_jwt(token, &state.jwt_secret).expect("returned JWT must be valid");
        assert_eq!(claims.sub, "admin");
    }

    #[tokio::test]
    async fn test_login_wrong_password_returns_401() {
        let state = make_state_with_password("hunter2");
        let app = make_app(state);
        let body = serde_json::json!({ "password": "wrong" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))))
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_login_no_password_configured_returns_404() {
        let state = make_state_no_password();
        let app = make_app(state);
        let body = serde_json::json!({ "password": "anything" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_login_limit_uses_peer_ip_and_preserves_static_auth() {
        let state = make_state_with_password("hunter2");
        let app = make_app(state);
        for attempt in 0..AppState::LOGIN_ATTEMPTS {
            let peer = SocketAddr::from(([127, 0, 0, 1], 4000 + attempt as u16));
            let mut req = login_request(peer, "wrong");
            let spoofed_ip = format!("198.51.100.{}", attempt + 1);
            req.headers_mut()
                .insert("x-forwarded-for", spoofed_ip.parse().unwrap());
            req.headers_mut()
                .insert("x-real-ip", spoofed_ip.parse().unwrap());
            req.headers_mut()
                .insert("forwarded", format!("for={spoofed_ip}").parse().unwrap());
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        let req = login_request(SocketAddr::from(([127, 0, 0, 1], 5000)), "hunter2");
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "60");
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");

        let req = login_request(SocketAddr::from(([127, 0, 0, 2], 5000)), "hunter2");
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        for (token, expected) in [
            ("tok", StatusCode::OK),
            ("invalid", StatusCode::UNAUTHORIZED),
        ] {
            let req = Request::builder()
                .uri("/api/channels")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected);
        }
    }

    #[tokio::test]
    async fn test_successful_logins_consume_attempts() {
        let mut state = AppState::new("tok".into(), EventBus::new(8));
        state.password_hash = Some(bcrypt::hash("hunter2", 4).unwrap());
        let app = make_app(Arc::new(state));
        for attempt in 0..=AppState::LOGIN_ATTEMPTS {
            let req = login_request(SocketAddr::from(([127, 0, 0, 1], 4000)), "hunter2");
            let resp = app.clone().oneshot(req).await.unwrap();
            let expected = if attempt < AppState::LOGIN_ATTEMPTS {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            };
            assert_eq!(resp.status(), expected);
        }
    }

    #[tokio::test]
    async fn test_malformed_login_is_limited_before_body_parsing() {
        let mut state = AppState::new("tok".into(), EventBus::new(8));
        state.password_hash = Some("unused".into());
        let app = make_app(Arc::new(state));
        for attempt in 0..=AppState::LOGIN_ATTEMPTS {
            let req = Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))))
                .body(Body::from("not json"))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            let expected = if attempt < AppState::LOGIN_ATTEMPTS {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::TOO_MANY_REQUESTS
            };
            assert_eq!(resp.status(), expected);
        }
    }

    #[tokio::test]
    async fn test_password_login_without_peer_fails_closed() {
        let mut state = AppState::new("tok".into(), EventBus::new(8));
        state.password_hash = Some("unused".into());
        let app = make_app(Arc::new(state));
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_disabled_login_remains_not_found_without_peer() {
        let app = make_app(make_state_no_password());
        for _ in 0..=AppState::LOGIN_ATTEMPTS {
            let req = Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"password":"anything"}"#))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn test_issue_ws_ticket_returns_single_use_ticket() {
        let state = make_state_no_password();
        let app = Router::new()
            .route("/api/auth/ws-ticket", post(issue_ws_ticket))
            .with_state(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/ws-ticket")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let ticket = json["ticket"].as_str().expect("ticket must be present");
        assert!(state.ws_tickets.consume(ticket).await);
        assert!(!state.ws_tickets.consume(ticket).await);
    }

    #[tokio::test]
    async fn test_issue_ws_ticket_returns_429_when_store_is_full() {
        let state = make_state_no_password();
        for _ in 0..crate::api::auth::MAX_PENDING_WS_TICKETS {
            assert!(state.ws_tickets.issue().await.is_some());
        }
        let app = Router::new()
            .route("/api/auth/ws-ticket", post(issue_ws_ticket))
            .with_state(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/ws-ticket")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers().get(header::RETRY_AFTER).unwrap(), "30");
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
}
