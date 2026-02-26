//! Auth routes for the panel API.
//!
//! Provides `POST /api/auth/login` which exchanges a valid password for a
//! short-lived HS256 JWT.  The JWT is subsequently accepted by the auth
//! middleware on all protected endpoints.

use axum::{extract::State, http::StatusCode, Json};
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{auth as panel_auth, events::EventBus};
    use axum::{body::Body, http::Request, routing::post, Router};
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn make_state_with_password(password: &str) -> Arc<AppState> {
        let hash = panel_auth::hash_password(password).expect("hash must succeed");
        let bus = EventBus::new(8);
        Arc::new(AppState {
            api_token: "tok".into(),
            event_bus: bus,
            password_hash: Some(hash),
            jwt_secret: "test-jwt-secret".into(),
        })
    }

    fn make_state_no_password() -> Arc<AppState> {
        let bus = EventBus::new(8);
        Arc::new(AppState {
            api_token: "tok".into(),
            event_bus: bus,
            password_hash: None,
            jwt_secret: "test-jwt-secret".into(),
        })
    }

    fn make_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/auth/login", post(login))
            .with_state(state)
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
}
