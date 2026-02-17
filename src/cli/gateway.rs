//! Gateway command handler (multi-channel bot server).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use zeptoclaw::bus::MessageBus;
use zeptoclaw::channels::{register_configured_channels, ChannelManager, WhatsAppChannel};
use zeptoclaw::config::{Config, ContainerAgentBackend};
use zeptoclaw::deps::{fetcher::RealFetcher, DepManager, HasDependencies};
use zeptoclaw::health::{
    health_port, start_health_server, start_periodic_usage_flush, UsageMetrics,
};
use zeptoclaw::heartbeat::{ensure_heartbeat_file, HeartbeatService};
use zeptoclaw::providers::{
    configured_provider_names, resolve_runtime_provider, RUNTIME_SUPPORTED_PROVIDERS,
};

use super::common::create_agent;
use super::heartbeat::heartbeat_file_path;

/// Start multi-channel gateway.
pub(crate) async fn cmd_gateway(
    containerized_flag: Option<String>,
    tunnel_flag: Option<String>,
) -> Result<()> {
    println!("Starting ZeptoClaw Gateway...");

    // Load configuration
    let mut config = Config::load().with_context(|| "Failed to load configuration")?;

    // --containerized [docker|apple] overrides config backend
    let containerized = containerized_flag.is_some();
    if let Some(ref b) = containerized_flag {
        if b != "auto" {
            config.container_agent.backend = match b.to_lowercase().as_str() {
                "docker" => ContainerAgentBackend::Docker,
                #[cfg(target_os = "macos")]
                "apple" => ContainerAgentBackend::Apple,
                "auto" => ContainerAgentBackend::Auto,
                other => {
                    #[cfg(target_os = "macos")]
                    return Err(anyhow::anyhow!(
                        "Unknown backend '{}'. Use: docker or apple",
                        other
                    ));
                    #[cfg(not(target_os = "macos"))]
                    return Err(anyhow::anyhow!("Unknown backend '{}'. Use: docker", other));
                }
            };
        }
    }

    // Start tunnel if requested
    let mut _tunnel: Option<Box<dyn zeptoclaw::tunnel::TunnelProvider>> = None;
    let tunnel_provider = tunnel_flag.or(config.tunnel.provider.clone());
    if let Some(ref provider) = tunnel_provider {
        let mut tunnel_config = config.tunnel.clone();
        tunnel_config.provider = Some(provider.clone());
        let mut t = zeptoclaw::tunnel::create_tunnel(&tunnel_config)
            .with_context(|| format!("Failed to create {} tunnel", provider))?;

        let gateway_port = config.gateway.port;
        let tunnel_url = t
            .start(gateway_port)
            .await
            .with_context(|| format!("Failed to start {} tunnel", provider))?;

        println!("Tunnel active: {}", tunnel_url);
        _tunnel = Some(t);
    }

    // Create message bus
    let bus = Arc::new(MessageBus::new());

    // Create usage metrics tracker
    let metrics = Arc::new(UsageMetrics::new());

    // Start health check server (liveness + readiness)
    let hp = health_port();
    let health_handle = match start_health_server(hp, Arc::clone(&metrics)).await {
        Ok(handle) => {
            info!(
                port = hp,
                "Health endpoints available at /healthz and /readyz"
            );
            Some(handle)
        }
        Err(e) => {
            warn!(error = %e, "Failed to start health server (non-fatal)");
            None
        }
    };

    // Create shutdown watch channel for periodic usage flush
    let (usage_shutdown_tx, usage_shutdown_rx) = tokio::sync::watch::channel(false);
    let usage_flush_handle = start_periodic_usage_flush(Arc::clone(&metrics), usage_shutdown_rx);

    // Determine agent backend: containerized or in-process
    let mut proxy = None;
    let proxy_handle = if containerized {
        info!("Starting gateway with containerized agent mode");

        // Resolve backend (auto-detect or explicit from config)
        let backend = zeptoclaw::gateway::resolve_backend(&config.container_agent)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        info!("Resolved container backend: {}", backend);

        // Validate the resolved backend
        match backend {
            zeptoclaw::gateway::ResolvedBackend::Docker => {
                validate_docker_available(configured_docker_binary(&config.container_agent))
                    .await?;
            }
            #[cfg(target_os = "macos")]
            zeptoclaw::gateway::ResolvedBackend::Apple => {
                validate_apple_available().await?;
            }
        }

        // Check image exists (Docker-specific)
        let image = &config.container_agent.image;
        if backend == zeptoclaw::gateway::ResolvedBackend::Docker {
            let docker_binary = configured_docker_binary(&config.container_agent);
            let image_check = tokio::process::Command::new(docker_binary)
                .args(["image", "inspect", image])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            if !image_check.map(|s| s.success()).unwrap_or(false) {
                eprintln!(
                    "Warning: Docker image '{}' not found (checked via '{}').",
                    image, docker_binary
                );
                eprintln!("Build it with: {} build -t {} .", docker_binary, image);
                return Err(anyhow::anyhow!(
                    "Docker image '{}' not found (checked via '{}')",
                    image,
                    docker_binary
                ));
            }
        }

        info!("Using container image: {} (backend={})", image, backend);

        let proxy_instance = Arc::new(zeptoclaw::gateway::ContainerAgentProxy::new(
            config.clone(),
            bus.clone(),
            backend,
        ));
        proxy_instance.set_usage_metrics(Arc::clone(&metrics));
        let proxy_for_task = Arc::clone(&proxy_instance);
        let proxy_metrics = Arc::clone(&metrics);
        proxy = Some(proxy_instance);

        Some(tokio::spawn(async move {
            if let Err(e) = proxy_for_task.start().await {
                error!("Container agent proxy error: {}", e);
            }
            proxy_metrics.set_ready(false);
            warn!("Container agent proxy stopped; readiness set to false");
        }))
    } else {
        // Validate provider for in-process mode
        let runtime_provider_name = resolve_runtime_provider(&config).map(|provider| provider.name);
        if runtime_provider_name.is_none() {
            let configured = configured_provider_names(&config);
            if configured.is_empty() {
                error!("No AI provider configured. Set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY");
                error!("or add your API key to {:?}", Config::path());
            } else {
                error!(
                    "Configured provider(s) are not supported by this runtime: {}",
                    configured.join(", ")
                );
                error!(
                    "Currently supported runtime providers: {}",
                    RUNTIME_SUPPORTED_PROVIDERS.join(", ")
                );
            }
            std::process::exit(1);
        }
        None
    };

    // Create in-process agent (only needed when not containerized)
    let agent = if !containerized {
        let agent = create_agent(config.clone(), bus.clone()).await?;
        agent.set_usage_metrics(Arc::clone(&metrics)).await;
        Some(agent)
    } else {
        None
    };

    // Create channel manager
    let channel_manager = ChannelManager::new(bus.clone(), config.clone());

    // Install and start channel dependencies (if any)
    let deps_dir = DepManager::default_dir();
    let dep_mgr = DepManager::new(deps_dir, Arc::new(RealFetcher));
    let deps = collect_enabled_channel_deps(&config);

    if !deps.is_empty() {
        info!("Installing {} channel dependencies...", deps.len());
        for dep in &deps {
            install_and_start_dep(&dep_mgr, dep).await;
        }
    }

    // Register channels via factory.
    let channel_count = register_configured_channels(&channel_manager, bus.clone(), &config).await;
    if channel_count == 0 {
        warn!(
            "No channels configured. Enable channels in {:?}",
            Config::path()
        );
        warn!("The agent loop will still run but won't receive messages from external sources.");
    } else {
        info!("Registered {} channel(s)", channel_count);
    }

    // Start all channels
    channel_manager
        .start_all()
        .await
        .with_context(|| "Failed to start channels")?;

    let heartbeat_service = if config.heartbeat.enabled {
        let hb_path = heartbeat_file_path(&config);
        match ensure_heartbeat_file(&hb_path).await {
            Ok(true) => info!("Created heartbeat file template at {:?}", hb_path),
            Ok(false) => {}
            Err(e) => warn!("Failed to initialize heartbeat file {:?}: {}", hb_path, e),
        }

        let service = Arc::new(HeartbeatService::new(
            hb_path,
            config.heartbeat.interval_secs,
            bus.clone(),
            "heartbeat:system",
        ));
        service.start().await?;
        Some(service)
    } else {
        None
    };

    // Start agent loop in background (only for in-process mode)
    let agent_handle = if let Some(ref agent) = agent {
        let agent_clone = Arc::clone(agent);
        let agent_metrics = Arc::clone(&metrics);
        Some(tokio::spawn(async move {
            if let Err(e) = agent_clone.start().await {
                error!("Agent loop error: {}", e);
            }
            agent_metrics.set_ready(false);
            warn!("Agent loop stopped; readiness set to false");
        }))
    } else {
        None
    };

    // Mark gateway as ready for /readyz
    metrics.set_ready(true);

    println!();
    if containerized {
        println!("Gateway is running (containerized mode). Press Ctrl+C to stop.");
    } else {
        println!("Gateway is running. Press Ctrl+C to stop.");
    }
    println!();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .with_context(|| "Failed to listen for Ctrl+C")?;

    println!();
    println!("Shutting down...");

    // Mark not ready immediately
    metrics.set_ready(false);

    // Signal usage flush to emit final summary
    let _ = usage_shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), usage_flush_handle).await;

    if let Some(service) = &heartbeat_service {
        service.stop().await;
    }

    // Stop agent or proxy
    if let Some(ref agent) = agent {
        agent.stop();
    }
    if let Some(ref proxy) = proxy {
        proxy.stop();
    }

    // Stop all channels
    channel_manager
        .stop_all()
        .await
        .with_context(|| "Failed to stop channels")?;

    // Wait for agent/proxy to stop
    if let Some(handle) = agent_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }
    if let Some(handle) = proxy_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    // Stop health server
    if let Some(handle) = health_handle {
        handle.abort();
    }

    println!("Gateway stopped.");
    Ok(())
}

/// Validate that Docker is available.
async fn validate_docker_available(docker_binary: &str) -> Result<()> {
    if !zeptoclaw::gateway::is_docker_available_with_binary(docker_binary).await {
        return Err(anyhow::anyhow!(
            "Docker is not available via '{}'. Install Docker or run without --containerized.",
            docker_binary
        ));
    }
    Ok(())
}

fn configured_docker_binary(config: &zeptoclaw::config::ContainerAgentConfig) -> &str {
    config
        .docker_binary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("docker")
}

/// Validate that Apple Container is available (macOS only).
#[cfg(target_os = "macos")]
async fn validate_apple_available() -> Result<()> {
    if !zeptoclaw::gateway::is_apple_container_available().await {
        return Err(anyhow::anyhow!(
            "Apple Container is not available. Requires macOS 15+ with `container` CLI installed."
        ));
    }
    Ok(())
}

/// Collect dependencies from all enabled channels with bridge_managed=true.
fn collect_enabled_channel_deps(config: &Config) -> Vec<zeptoclaw::deps::Dependency> {
    let mut deps = Vec::new();

    // WhatsApp
    if let Some(ref wa_cfg) = config.channels.whatsapp {
        if wa_cfg.enabled && wa_cfg.bridge_managed {
            // Create temporary channel just to query dependencies
            let temp_bus = Arc::new(MessageBus::new());
            let channel = WhatsAppChannel::new(wa_cfg.clone(), temp_bus);
            deps.extend(channel.dependencies());
        }
    }

    // Future: Add Telegram, Discord, Slack if they declare dependencies

    deps
}

/// Install and start a dependency with warn-on-fail (non-blocking).
///
/// Performs three phases in order:
/// 1. Install - early return on failure
/// 2. Start - early return on failure
/// 3. Health check (10s timeout) - logs warning but does not block
///
/// All failures are logged as warnings, allowing the gateway to continue.
async fn install_and_start_dep(mgr: &DepManager, dep: &zeptoclaw::deps::Dependency) {
    // Install
    match mgr.ensure_installed(dep).await {
        Ok(_) => info!("✓ Installed {}", dep.name),
        Err(e) => {
            warn!("✗ Failed to install {}: {}", dep.name, e);
            return;
        }
    }

    // Start
    match mgr.start(dep).await {
        Ok(_) => info!("✓ Started {}", dep.name),
        Err(e) => {
            warn!("✗ Failed to start {}: {}", dep.name, e);
            return;
        }
    }

    // Health check (10s timeout)
    let health_timeout = Duration::from_secs(10);
    match mgr.wait_healthy(dep, health_timeout).await {
        Ok(_) => info!("✓ {} is healthy", dep.name),
        Err(e) => warn!("✗ {} health check failed: {}", dep.name, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_enabled_channel_deps_whatsapp_managed() {
        let mut config = Config::default();
        config.channels.whatsapp = Some(zeptoclaw::config::WhatsAppConfig {
            enabled: true,
            bridge_managed: true,
            bridge_url: "ws://localhost:3001".to_string(),
            bridge_token: None,
            allow_from: vec![],
            deny_by_default: false,
        });

        let deps = collect_enabled_channel_deps(&config);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "whatsmeow-bridge");
    }

    #[test]
    fn test_collect_enabled_channel_deps_whatsapp_not_managed() {
        let mut config = Config::default();
        config.channels.whatsapp = Some(zeptoclaw::config::WhatsAppConfig {
            enabled: true,
            bridge_managed: false, // User manages externally
            bridge_url: "ws://localhost:3001".to_string(),
            bridge_token: None,
            allow_from: vec![],
            deny_by_default: false,
        });

        let deps = collect_enabled_channel_deps(&config);
        assert_eq!(deps.len(), 0); // No deps when not managed
    }

    #[test]
    fn test_collect_enabled_channel_deps_whatsapp_disabled() {
        let mut config = Config::default();
        config.channels.whatsapp = Some(zeptoclaw::config::WhatsAppConfig {
            enabled: false,
            bridge_managed: true,
            bridge_url: "ws://localhost:3001".to_string(),
            bridge_token: None,
            allow_from: vec![],
            deny_by_default: false,
        });

        let deps = collect_enabled_channel_deps(&config);
        assert_eq!(deps.len(), 0); // No deps when disabled
    }
}
