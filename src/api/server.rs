//! Axum API server for ZeptoClaw Panel.

use crate::api::config::PanelConfig;
use crate::api::events::EventBus;
use axum::{routing::get, Router};
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
        .route("/api/health", get(super::routes::health::get_health))
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
