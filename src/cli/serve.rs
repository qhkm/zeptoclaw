//! `zeptoclaw serve` — standalone OpenAI-compatible API server.
//!
//! Boots the kernel (provider chain, tools, safety) and exposes:
//! - `POST /v1/chat/completions` (streaming + non-streaming)
//! - `GET  /v1/models`
//!
//! No panel UI, no auth middleware — intended for local or trusted-network use.

use std::sync::Arc;

use anyhow::Result;
use axum::routing::{get, post};
use axum::Router;
use tracing::info;

/// Run the standalone OpenAI-compatible API server.
pub async fn cmd_serve(port: u16, bind: String) -> Result<()> {
    let config = zeptoclaw::config::Config::load()
        .map_err(|e| anyhow::anyhow!("Failed to load configuration: {e}"))?;

    let bus = Arc::new(zeptoclaw::bus::MessageBus::new());
    let kernel = zeptoclaw::kernel::ZeptoKernel::boot(config, bus, None, None).await?;

    let provider = kernel
        .provider()
        .ok_or_else(|| anyhow::anyhow!("No LLM provider configured — cannot serve API"))?;

    let config_arc = kernel.config.clone();

    // Build a minimal AppState with only the fields the OpenAI routes need.
    let event_bus = zeptoclaw::api::events::EventBus::new(4);
    let mut state = zeptoclaw::api::server::AppState::new(String::new(), event_bus);
    state.provider = Some(provider);
    state.config = Some(config_arc);

    let shared = Arc::new(state);

    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(zeptoclaw::api::routes::openai::chat_completions),
        )
        .route(
            "/v1/models",
            get(zeptoclaw::api::routes::openai::list_models),
        )
        .with_state(shared);

    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("OpenAI-compatible API server listening on {addr}");
    info!("Try: curl http://{addr}/v1/models");

    // Graceful shutdown on ctrl-c.
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("Shutting down serve...");
        })
        .await;

    // Always shut down kernel subsystems (MCP clients, etc.), even on error.
    kernel.shutdown().await;

    serve_result?;
    Ok(())
}
