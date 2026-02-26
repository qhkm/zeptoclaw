//! Axum API server for ZeptoClaw Panel.

use crate::api::config::PanelConfig;
use crate::api::events::EventBus;
use axum::routing::{get, post, put};
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

/// Shared state for all API handlers.
#[derive(Clone)]
pub struct AppState {
    pub api_token: String,
    pub event_bus: EventBus,
}

impl AppState {
    pub fn new(api_token: String, event_bus: EventBus) -> Self {
        Self {
            api_token,
            event_bus,
        }
    }
}

/// Build the axum router with all API routes.
pub fn build_router(state: AppState, static_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
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
        .layer(CorsLayer::permissive()) // Tightened in security task
        .with_state(Arc::new(state));

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
}
