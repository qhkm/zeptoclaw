//! Router and startup guard for `zeptoclaw serve`, the standalone
//! OpenAI-compatible API.
//!
//! When an API key is configured, every `/v1/*` request must carry
//! `Authorization: Bearer <key>`. Without a key the server only starts on a
//! loopback address, unless the operator explicitly opts out with
//! `--insecure-no-auth`.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self as axum_mw, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use super::routes::openai;
use super::server::{AppState, API_BODY_LIMIT_BYTES};

/// Environment variable read for the serve API key when `--api-key` is absent.
pub const SERVE_API_KEY_ENV: &str = "ZEPTOCLAW_SERVE_API_KEY";

/// Pick the serve API key from the CLI flag, falling back to the environment.
///
/// Blank values count as unset, so `ZEPTOCLAW_SERVE_API_KEY=` cannot silently
/// enable a key that every client trivially matches.
pub fn resolve_api_key(flag: Option<String>, env: Option<String>) -> Option<String> {
    flag.or(env).filter(|key| !key.trim().is_empty())
}

/// Returns true when `bind` is a loopback address (`127.0.0.0/8`, `::1`, `localhost`).
pub fn is_loopback_bind(bind: &str) -> bool {
    if bind.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let addr = bind
        .strip_prefix('[')
        .and_then(|b| b.strip_suffix(']'))
        .unwrap_or(bind);
    addr.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Refuse to expose an unauthenticated server beyond loopback.
///
/// # Errors
///
/// Returns a message for the operator when `bind` is not loopback, no API key
/// is set, and `insecure_no_auth` is false.
pub fn check_bind_security(
    bind: &str,
    has_api_key: bool,
    insecure_no_auth: bool,
) -> Result<(), String> {
    if has_api_key || insecure_no_auth || is_loopback_bind(bind) {
        return Ok(());
    }
    Err(format!(
        "Refusing to serve on non-loopback address {bind} without authentication. \
         Set {SERVE_API_KEY_ENV} (or pass --api-key), or pass --insecure-no-auth \
         to expose the API unauthenticated."
    ))
}

/// Build the serve router: `/v1/chat/completions` and `/v1/models`, a 1 MiB
/// body limit, and Bearer-key auth when `api_key` is `Some`.
pub fn build_serve_router(state: Arc<AppState>, api_key: Option<String>) -> Router {
    let router = Router::new()
        .route("/v1/chat/completions", post(openai::chat_completions))
        .route("/v1/models", get(openai::list_models))
        .layer(DefaultBodyLimit::max(API_BODY_LIMIT_BYTES));

    let router = match api_key {
        Some(key) => router.layer(axum_mw::from_fn_with_state(Arc::new(key), require_api_key)),
        None => router,
    };

    router.with_state(state)
}

/// Reject requests whose `Authorization: Bearer` token does not match the key.
async fn require_api_key(State(key): State<Arc<String>>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(token) if super::auth::constant_time_eq(token, &key) => next.run(request).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(serde_json::json!({
                "error": {
                    "message": "Invalid or missing API key",
                    "type": "invalid_request_error",
                    "code": "invalid_api_key",
                }
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn app(api_key: Option<&str>) -> Router {
        let state = AppState::new(String::new(), EventBus::new(4));
        build_serve_router(Arc::new(state), api_key.map(str::to_string))
    }

    fn models_request(auth: Option<&str>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri("/v1/models");
        if let Some(auth) = auth {
            builder = builder.header("authorization", auth);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn loopback_binds_are_recognised() {
        for bind in [
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "[::1]",
            "localhost",
            "LOCALHOST",
        ] {
            assert!(is_loopback_bind(bind), "{bind} should be loopback");
        }
        for bind in ["0.0.0.0", "::", "192.168.1.10", "example.com", ""] {
            assert!(!is_loopback_bind(bind), "{bind} should not be loopback");
        }
    }

    #[test]
    fn non_loopback_without_key_is_refused() {
        let err = check_bind_security("0.0.0.0", false, false).unwrap_err();
        assert!(err.contains(SERVE_API_KEY_ENV));
        assert!(err.contains("--insecure-no-auth"));
    }

    #[test]
    fn bind_security_allows_loopback_key_or_explicit_opt_out() {
        assert!(check_bind_security("127.0.0.1", false, false).is_ok());
        assert!(check_bind_security("0.0.0.0", true, false).is_ok());
        assert!(check_bind_security("0.0.0.0", false, true).is_ok());
    }

    #[test]
    fn resolve_api_key_prefers_flag_and_ignores_blank() {
        assert_eq!(
            resolve_api_key(Some("flag".into()), Some("env".into())).as_deref(),
            Some("flag")
        );
        assert_eq!(
            resolve_api_key(None, Some("env".into())).as_deref(),
            Some("env")
        );
        assert_eq!(resolve_api_key(Some("  ".into()), None), None);
        assert_eq!(resolve_api_key(None, Some(String::new())), None);
        assert_eq!(resolve_api_key(None, None), None);
    }

    #[tokio::test]
    async fn no_key_serves_without_auth() {
        let resp = app(None).oneshot(models_request(None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn key_rejects_missing_and_wrong_tokens() {
        for auth in [
            None,
            Some("Bearer wrong"),
            Some("secret"),
            Some("Basic secret"),
        ] {
            let resp = app(Some("secret"))
                .oneshot(models_request(auth))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "auth = {auth:?}");
            assert_eq!(resp.headers()[header::WWW_AUTHENTICATE], "Bearer");
        }
    }

    #[tokio::test]
    async fn key_accepts_matching_bearer_token() {
        let resp = app(Some("secret"))
            .oneshot(models_request(Some("Bearer secret")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let body = vec![b'a'; API_BODY_LIMIT_BYTES + 1];
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app(None).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
