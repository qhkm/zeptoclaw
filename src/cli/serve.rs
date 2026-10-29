//! `zeptoclaw serve` — standalone OpenAI-compatible API server.
//!
//! Boots the kernel (provider chain, tools, safety) and exposes:
//! - `POST /v1/chat/completions` (streaming + non-streaming)
//! - `GET  /v1/models`
//!
//! No panel UI. Requests need `Authorization: Bearer <key>` when an API key is
//! set (`--api-key` or `ZEPTOCLAW_SERVE_API_KEY`); without one the server only
//! binds to loopback unless `--insecure-no-auth` is passed.

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};
use zeptoclaw::api::serve::{
    build_serve_router, check_bind_security, resolve_api_key, SERVE_API_KEY_ENV,
};

/// Run the standalone OpenAI-compatible API server.
pub async fn cmd_serve(
    port: u16,
    bind: String,
    api_key: Option<String>,
    insecure_no_auth: bool,
) -> Result<()> {
    let api_key = resolve_api_key(api_key, std::env::var(SERVE_API_KEY_ENV).ok());
    check_bind_security(&bind, api_key.is_some(), insecure_no_auth).map_err(anyhow::Error::msg)?;
    if api_key.is_none() && insecure_no_auth {
        warn!("Serving on {bind} WITHOUT authentication (--insecure-no-auth)");
    }

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

    let auth_enabled = api_key.is_some();
    let app = build_serve_router(shared, api_key);

    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("OpenAI-compatible API server listening on {addr}");
    if auth_enabled {
        info!(
            "API key required. Try: curl -H \"Authorization: Bearer $KEY\" http://{addr}/v1/models"
        );
    } else {
        info!("Try: curl http://{addr}/v1/models");
    }

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
