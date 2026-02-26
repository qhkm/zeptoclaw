//! Axum API server for ZeptoClaw Panel.

use crate::api::config::PanelConfig;
use crate::api::events::EventBus;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, Method};
use axum::middleware as axum_mw;
use axum::routing::{get, post, put};
use axum::{extract::State, Json, Router};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Shared state for all API handlers.
#[derive(Clone)]
pub struct AppState {
    /// Static API token accepted on all protected endpoints.
    pub api_token: String,
    /// Event bus for WebSocket broadcasts.
    pub event_bus: EventBus,
    /// Optional bcrypt hash of the panel password.
    ///
    /// When `Some`, `POST /api/auth/login` is enabled and exchanges a valid
    /// password for a short-lived HS256 JWT.  When `None`, the login endpoint
    /// returns 404 and callers must use the static `api_token` directly.
    pub password_hash: Option<String>,
    /// Secret used to sign and verify HS256 JWTs.
    ///
    /// Generated randomly at startup via `uuid::Uuid::new_v4()` so it rotates
    /// on every process restart, invalidating any previously issued JWTs.
    pub jwt_secret: String,
    /// Semaphore limiting the number of concurrent WebSocket connections.
    ///
    /// Hard cap of 5.  Each accepted WebSocket upgrade acquires one permit and
    /// holds it for the lifetime of the connection; once the semaphore is
    /// exhausted the handler returns HTTP 503.
    pub ws_semaphore: Arc<tokio::sync::Semaphore>,
    // ── Real data stores (all optional — set when wired from gateway/CLI) ───
    /// Session manager for reading and deleting conversation sessions.
    pub session_manager: Option<Arc<crate::session::SessionManager>>,
    /// Kanban task store for full CRUD on board tasks.
    pub task_store: Option<Arc<crate::api::tasks::TaskStore>>,
    /// Health registry for live component check data.
    pub health_registry: Option<Arc<crate::health::HealthRegistry>>,
    /// Lock-free usage counters (requests, tokens, tool calls, errors).
    pub usage_metrics: Option<Arc<crate::health::UsageMetrics>>,
    /// Per-tool call stats and token tracking for the current session.
    pub metrics_collector: Option<Arc<crate::utils::metrics::MetricsCollector>>,
}

impl AppState {
    /// Maximum number of concurrent WebSocket connections.
    pub const MAX_WS_CONNECTIONS: usize = 5;

    pub fn new(api_token: String, event_bus: EventBus) -> Self {
        Self {
            api_token,
            event_bus,
            password_hash: None,
            jwt_secret: uuid::Uuid::new_v4().to_string(),
            ws_semaphore: Arc::new(tokio::sync::Semaphore::new(Self::MAX_WS_CONNECTIONS)),
            session_manager: None,
            task_store: None,
            health_registry: None,
            usage_metrics: None,
            metrics_collector: None,
        }
    }
}

/// Handler for `GET /api/csrf-token`.
///
/// Returns a fresh CSRF token bound to the server's `jwt_secret`.  The
/// endpoint is public (no `Authorization` header required) so that a browser
/// or CLI client can obtain a token before making any mutating request.
async fn csrf_token_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let token = super::middleware::generate_csrf_token(&state.jwt_secret);
    Json(serde_json::json!({ "token": token }))
}

/// Build the axum router with all API routes.
pub fn build_router(state: AppState, static_dir: Option<PathBuf>) -> Router {
    // Wrap state in Arc once so it can be shared across both the middleware
    // layer and the route handlers without a double-Arc.
    let shared_state = Arc::new(state);

    // CORS: only allow requests from the panel frontend origin.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(
            "http://localhost:9092"
                .parse::<axum::http::HeaderValue>()
                .expect("valid origin"),
        ))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("authorization"),
            HeaderName::from_static("x-csrf-token"),
        ]);

    let api = Router::new()
        // Auth
        .route("/api/auth/login", post(super::routes::auth::login))
        // CSRF bootstrap — public, no auth required
        .route("/api/csrf-token", get(csrf_token_handler))
        // Health & metrics
        .route("/api/health", get(super::routes::health::get_health))
        .route("/api/metrics", get(super::routes::metrics::get_metrics))
        // Sessions
        .route("/api/sessions", get(super::routes::sessions::list_sessions))
        .route(
            "/api/sessions/{key}",
            get(super::routes::sessions::get_session)
                .delete(super::routes::sessions::delete_session),
        )
        // Channels
        .route("/api/channels", get(super::routes::channels::list_channels))
        // Cron
        .route(
            "/api/cron",
            get(super::routes::cron::list_jobs).post(super::routes::cron::create_job),
        )
        .route(
            "/api/cron/{id}",
            put(super::routes::cron::update_job).delete(super::routes::cron::delete_job),
        )
        .route(
            "/api/cron/{id}/trigger",
            post(super::routes::cron::trigger_job),
        )
        // Routines
        .route(
            "/api/routines",
            get(super::routes::routines::list_routines)
                .post(super::routes::routines::create_routine),
        )
        .route(
            "/api/routines/{id}",
            put(super::routes::routines::update_routine)
                .delete(super::routes::routines::delete_routine),
        )
        .route(
            "/api/routines/{id}/toggle",
            post(super::routes::routines::toggle_routine),
        )
        // Tasks (kanban)
        .route(
            "/api/tasks",
            get(super::routes::tasks::list_tasks).post(super::routes::tasks::create_task),
        )
        .route(
            "/api/tasks/{id}",
            put(super::routes::tasks::update_task).delete(super::routes::tasks::delete_task),
        )
        .route(
            "/api/tasks/{id}/move",
            post(super::routes::tasks::move_task),
        )
        // WebSocket
        .route("/ws/events", get(super::routes::ws::ws_events))
        // Body size limit: 1 MiB.  Applied before the auth middleware so we
        // reject oversized payloads cheaply before any token validation.
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(cors)
        .layer(axum_mw::from_fn_with_state(
            shared_state.clone(),
            super::middleware::auth_middleware,
        ))
        .with_state(shared_state);

    if let Some(dir) = static_dir {
        api.fallback_service(tower_http::services::ServeDir::new(dir))
    } else {
        api
    }
}

/// Start the API server.
pub async fn start_server(
    config: &PanelConfig,
    state: AppState,
    static_dir: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_router(state, static_dir);
    let addr = format!("{}:{}", config.bind, config.api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Panel API server listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_new() {
        let bus = EventBus::new(16);
        let state = AppState::new("test-token".into(), bus);
        assert_eq!(state.api_token, "test-token");
        assert!(state.password_hash.is_none());
        assert!(!state.jwt_secret.is_empty());
    }

    #[test]
    fn test_app_state_jwt_secret_rotates() {
        let bus1 = EventBus::new(4);
        let bus2 = EventBus::new(4);
        let s1 = AppState::new("tok".into(), bus1);
        let s2 = AppState::new("tok".into(), bus2);
        // Each instance generates a distinct secret.
        assert_ne!(s1.jwt_secret, s2.jwt_secret);
    }

    #[test]
    fn test_build_router_no_static() {
        let bus = EventBus::new(16);
        let state = AppState::new("tok".into(), bus);
        let _router = build_router(state, None);
    }

    #[test]
    fn test_build_router_with_static() {
        let bus = EventBus::new(16);
        let state = AppState::new("tok".into(), bus);
        let dir = std::env::temp_dir();
        let _router = build_router(state, Some(dir));
    }

    #[test]
    fn test_ws_semaphore_initialized_with_correct_permits() {
        let bus = EventBus::new(4);
        let state = AppState::new("tok".into(), bus);
        // All permits should be available at startup.
        assert_eq!(
            state.ws_semaphore.available_permits(),
            AppState::MAX_WS_CONNECTIONS
        );
    }

    #[test]
    fn test_ws_semaphore_max_connections_constant() {
        // Sanity-check the documented cap.
        assert_eq!(AppState::MAX_WS_CONNECTIONS, 5);
    }
}
