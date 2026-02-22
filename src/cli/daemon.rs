//! Daemon — supervised long-running agent with auto-restart.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use zeptoclaw::config::Config;

pub const INITIAL_BACKOFF_MS: u64 = 1_000;
pub const MAX_BACKOFF_MS: u64 = 300_000; // 5 minutes

pub fn compute_backoff(current_ms: u64) -> u64 {
    current_ms.saturating_mul(2).min(MAX_BACKOFF_MS)
}

pub fn daemon_state_path() -> PathBuf {
    Config::dir().join("daemon_state.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub status: String,
    pub started_at: String,
    pub gateway: String,
    pub components: Vec<ComponentState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentState {
    pub name: String,
    pub running: bool,
    pub restart_count: u64,
    pub last_error: Option<String>,
}

/// Write daemon state to disk.
pub fn write_state(state: &DaemonState) -> Result<()> {
    let path = daemon_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Read daemon state from disk (used by `status` command).
#[allow(dead_code)]
pub fn read_state() -> Option<DaemonState> {
    let path = daemon_state_path();
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Remove daemon state file on clean shutdown.
#[allow(dead_code)]
pub fn remove_state() {
    let _ = std::fs::remove_file(daemon_state_path());
}

/// CLI entry point for `zeptoclaw daemon`.
pub(crate) async fn cmd_daemon() -> Result<()> {
    println!("Starting ZeptoClaw Daemon...");

    let config = Config::load()?;

    let started_at = chrono::Utc::now().to_rfc3339();
    let gateway_addr = format!("{}:{}", config.gateway.host, config.gateway.port);

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Received shutdown signal");
        shutdown_clone.store(true, Ordering::SeqCst);
    });

    let restart_count = Arc::new(AtomicU64::new(0));
    let mut backoff_ms = INITIAL_BACKOFF_MS;

    while !shutdown.load(Ordering::SeqCst) {
        let state = DaemonState {
            status: "running".into(),
            started_at: started_at.clone(),
            gateway: gateway_addr.clone(),
            components: vec![ComponentState {
                name: "gateway".into(),
                running: true,
                restart_count: restart_count.load(Ordering::Relaxed),
                last_error: None,
            }],
        };
        let _ = write_state(&state);

        info!("Starting gateway component");
        match super::gateway::cmd_gateway(None, None).await {
            Ok(()) => {
                info!("Gateway exited cleanly");
                break;
            }
            Err(e) => {
                let count = restart_count.fetch_add(1, Ordering::Relaxed) + 1;
                error!("Gateway failed (attempt {}): {}", count, e);

                let err_state = DaemonState {
                    status: "restarting".into(),
                    started_at: started_at.clone(),
                    gateway: gateway_addr.clone(),
                    components: vec![ComponentState {
                        name: "gateway".into(),
                        running: false,
                        restart_count: count,
                        last_error: Some(e.to_string()),
                    }],
                };
                let _ = write_state(&err_state);

                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                warn!("Restarting in {}ms (attempt {})", backoff_ms, count);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = compute_backoff(backoff_ms);
            }
        }
    }

    info!("Daemon shutting down");
    let final_state = DaemonState {
        status: "stopped".into(),
        started_at,
        gateway: gateway_addr,
        components: vec![ComponentState {
            name: "gateway".into(),
            running: false,
            restart_count: restart_count.load(Ordering::Relaxed),
            last_error: None,
        }],
    };
    let _ = write_state(&final_state);

    println!("Daemon stopped.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_backoff_doubles() {
        assert_eq!(compute_backoff(1000), 2000);
        assert_eq!(compute_backoff(2000), 4000);
    }

    #[test]
    fn test_compute_backoff_caps_at_max() {
        assert_eq!(compute_backoff(200_000), MAX_BACKOFF_MS);
        assert_eq!(compute_backoff(MAX_BACKOFF_MS), MAX_BACKOFF_MS);
    }

    #[test]
    fn test_compute_backoff_initial() {
        assert_eq!(compute_backoff(INITIAL_BACKOFF_MS), INITIAL_BACKOFF_MS * 2);
    }

    #[test]
    fn test_daemon_state_serialize() {
        let state = DaemonState {
            status: "running".into(),
            started_at: "2026-02-22T10:00:00Z".into(),
            gateway: "127.0.0.1:8080".into(),
            components: vec![ComponentState {
                name: "telegram".into(),
                running: true,
                restart_count: 0,
                last_error: None,
            }],
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        assert!(json.contains("telegram"));
        assert!(json.contains("running"));
    }

    #[test]
    fn test_daemon_state_deserialize() {
        let json = r#"{"status":"running","started_at":"2026-02-22T10:00:00Z","gateway":"127.0.0.1:8080","components":[{"name":"gw","running":true,"restart_count":2,"last_error":null}]}"#;
        let state: DaemonState = serde_json::from_str(json).unwrap();
        assert_eq!(state.components.len(), 1);
        assert_eq!(state.components[0].restart_count, 2);
    }

    #[test]
    fn test_daemon_state_path() {
        let path = daemon_state_path();
        assert!(path.ends_with("daemon_state.json"));
    }
}
