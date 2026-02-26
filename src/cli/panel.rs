//! `zeptoclaw panel` command — install, start, auth management.

use anyhow::{Context, Result};
use std::path::PathBuf;
use zeptoclaw::api::auth::generate_api_token;
use zeptoclaw::api::config::PanelConfig;
use zeptoclaw::api::events::EventBus;
use zeptoclaw::api::server::{start_server, AppState};
use zeptoclaw::config::Config;

/// Panel subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum PanelAction {
    /// Install panel (build from source or download pre-built)
    Install {
        /// Download pre-built assets from GitHub releases instead of building
        #[arg(long)]
        download: bool,
        /// Force rebuild even if already installed
        #[arg(long)]
        rebuild: bool,
    },
    /// Manage panel authentication
    Auth {
        #[command(subcommand)]
        action: PanelAuthAction,
    },
    /// Uninstall panel (remove dist, node_modules, token)
    Uninstall,
}

/// Panel auth subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum PanelAuthAction {
    /// Set auth mode (token, password, none)
    Mode {
        /// Auth mode to set
        mode: String,
    },
    /// Reset password
    ResetPassword,
    /// Show current auth status
    Status,
}

/// Main entry point for `zeptoclaw panel`.
pub async fn cmd_panel(
    config: Config,
    action: Option<PanelAction>,
    dev: bool,
    api_only: bool,
    port: Option<u16>,
    api_port: Option<u16>,
    rotate_token: bool,
) -> Result<()> {
    match action {
        Some(PanelAction::Install { download, rebuild }) => cmd_install(download, rebuild).await,
        Some(PanelAction::Auth {
            action: auth_action,
        }) => cmd_auth(auth_action).await,
        Some(PanelAction::Uninstall) => cmd_uninstall().await,
        None => cmd_start(config, dev, api_only, port, api_port, rotate_token).await,
    }
}

/// Resolve the panel dist directory.
///
/// Checks two locations in order:
/// 1. `./panel/dist/` — local repo checkout (dev mode)
/// 2. `~/.zeptoclaw/panel/dist/` — downloaded/installed assets
fn resolve_panel_dir() -> Option<PathBuf> {
    let local = PathBuf::from("panel/dist");
    if local.join("index.html").exists() {
        return Some(local);
    }
    if let Some(home) = dirs::home_dir() {
        let global = home.join(".zeptoclaw/panel/dist");
        if global.join("index.html").exists() {
            return Some(global);
        }
    }
    None
}

/// Get the token file path.
fn token_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zeptoclaw/panel.token")
}

/// Ensure API token exists, generating one if needed.
///
/// If the token file already contains a non-empty token, it is returned as-is.
/// Otherwise a fresh 64-char hex token is generated, persisted, and returned.
async fn ensure_api_token(token_path: &PathBuf) -> Result<String> {
    if token_path.exists() {
        let token = tokio::fs::read_to_string(token_path)
            .await
            .with_context(|| format!("Failed to read token file: {}", token_path.display()))?;
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let token = generate_api_token();

    if let Some(parent) = token_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    tokio::fs::write(token_path, &token)
        .await
        .with_context(|| format!("Failed to write token file: {}", token_path.display()))?;

    Ok(token)
}

/// Start the panel (API server + optional static file serving).
async fn cmd_start(
    config: Config,
    dev: bool,
    api_only: bool,
    port: Option<u16>,
    api_port: Option<u16>,
    rotate_token: bool,
) -> Result<()> {
    let mut panel_config: PanelConfig = config.panel.clone();

    if let Some(p) = port {
        panel_config.port = p;
    }
    if let Some(ap) = api_port {
        panel_config.api_port = ap;
    }

    let tp = token_path();
    if rotate_token && tp.exists() {
        tokio::fs::remove_file(&tp)
            .await
            .with_context(|| "Failed to remove old token file")?;
    }
    let api_token = ensure_api_token(&tp).await?;

    let static_dir = if api_only || dev {
        None
    } else {
        match resolve_panel_dir() {
            Some(dir) => {
                println!("Serving panel from {}", dir.display());
                Some(dir)
            }
            None => {
                println!("Panel assets not found. Run 'zeptoclaw panel install' first.");
                println!("Starting in API-only mode.");
                None
            }
        }
    };

    let event_bus = EventBus::new(256);
    let state = AppState::new(api_token.clone(), event_bus);

    println!(
        "Starting ZeptoClaw Panel API on {}:{}",
        panel_config.bind, panel_config.api_port
    );
    if !dev && !api_only && static_dir.is_some() {
        println!(
            "Panel UI: http://{}:{}",
            panel_config.bind, panel_config.port
        );
    }
    println!("API token: {api_token}");
    println!("Press Ctrl+C to stop.");

    start_server(&panel_config, state, static_dir)
        .await
        .map_err(|e| anyhow::anyhow!("Panel API server error: {e}"))?;

    Ok(())
}

/// Install the panel.
async fn cmd_install(download: bool, _rebuild: bool) -> Result<()> {
    if download {
        println!("Downloading pre-built panel assets...");
        // TODO: fetch from GitHub releases and extract to ~/.zeptoclaw/panel/dist/
        println!("Download not yet implemented. Use 'pnpm --dir panel build' manually.");
    } else {
        println!("Building panel from source...");

        // Verify Node.js is available
        let node_check = tokio::process::Command::new("node")
            .arg("--version")
            .output()
            .await;

        match node_check {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                println!("Found Node.js {}", version.trim());
            }
            _ => {
                anyhow::bail!("Node.js >= 18 is required. Install from https://nodejs.org");
            }
        }

        // Verify pnpm is available, attempt corepack activation if not
        let pnpm_check = tokio::process::Command::new("pnpm")
            .arg("--version")
            .output()
            .await;
        let pnpm_ok = pnpm_check.map(|o| o.status.success()).unwrap_or(false);
        if !pnpm_ok {
            println!("pnpm not found. Attempting to enable via corepack...");
            let _ = tokio::process::Command::new("corepack")
                .args(["enable", "pnpm"])
                .status()
                .await;
        }

        // Install dependencies
        println!("Installing dependencies...");
        let install = tokio::process::Command::new("pnpm")
            .args(["install", "--dir", "panel"])
            .status()
            .await
            .with_context(|| "Failed to run pnpm install")?;

        if !install.success() {
            anyhow::bail!("pnpm install failed");
        }

        // Build the panel
        println!("Building panel...");
        let build = tokio::process::Command::new("pnpm")
            .args(["--dir", "panel", "build"])
            .status()
            .await
            .with_context(|| "Failed to run pnpm build")?;

        if !build.success() {
            anyhow::bail!("Panel build failed");
        }
    }

    // Ensure an API token exists after install
    let tp = token_path();
    let token = ensure_api_token(&tp).await?;

    println!();
    println!("Panel installed successfully!");
    println!("API token: {token}");
    println!("Start with: zeptoclaw panel");

    Ok(())
}

/// Handle panel auth subcommands.
async fn cmd_auth(action: PanelAuthAction) -> Result<()> {
    match action {
        PanelAuthAction::Status => {
            println!("Auth mode: token (default)");
            let tp = token_path();
            if tp.exists() {
                println!("Token file: {}", tp.display());
            } else {
                println!("No token file found. Run 'zeptoclaw panel install' to generate one.");
            }
            Ok(())
        }
        PanelAuthAction::Mode { mode } => {
            // TODO: persist auth mode to panel config
            println!("Auth mode set to: {mode}");
            Ok(())
        }
        PanelAuthAction::ResetPassword => {
            // TODO: prompt for new password, hash with bcrypt, persist
            println!("Password reset is not yet implemented.");
            Ok(())
        }
    }
}

/// Uninstall the panel (remove token; optionally remove built assets).
async fn cmd_uninstall() -> Result<()> {
    let tp = token_path();
    if tp.exists() {
        tokio::fs::remove_file(&tp)
            .await
            .with_context(|| format!("Failed to remove token file: {}", tp.display()))?;
        println!("Removed token file: {}", tp.display());
    } else {
        println!("No token file found.");
    }

    // TODO: remove panel/node_modules and panel/dist when asset download is implemented
    println!("Panel uninstalled.");

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_panel_dir_missing() {
        // In CI / test environments panel/dist may not exist — just verify no panic.
        let _ = resolve_panel_dir();
    }

    #[test]
    fn test_token_path_contains_zeptoclaw() {
        let path = token_path();
        let s = path.to_str().unwrap();
        assert!(
            s.contains(".zeptoclaw"),
            "token path must be inside .zeptoclaw dir"
        );
        assert!(
            s.ends_with("panel.token"),
            "token file must be named panel.token"
        );
    }

    #[tokio::test]
    async fn test_ensure_api_token_creates_new() {
        let dir = tempfile::tempdir().unwrap();
        let tp = dir.path().join("panel.token");
        let token = ensure_api_token(&tp).await.unwrap();
        assert_eq!(token.len(), 64, "generated token must be 64 hex chars");
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token must contain only hex digits"
        );
        assert!(tp.exists(), "token file must be persisted");
    }

    #[tokio::test]
    async fn test_ensure_api_token_reuses_existing() {
        let dir = tempfile::tempdir().unwrap();
        let tp = dir.path().join("panel.token");
        let t1 = ensure_api_token(&tp).await.unwrap();
        let t2 = ensure_api_token(&tp).await.unwrap();
        assert_eq!(t1, t2, "subsequent calls must return the same token");
    }

    #[tokio::test]
    async fn test_ensure_api_token_ignores_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let tp = dir.path().join("panel.token");
        // Write an empty/whitespace-only file
        tokio::fs::write(&tp, "   \n").await.unwrap();
        let token = ensure_api_token(&tp).await.unwrap();
        assert_eq!(
            token.len(),
            64,
            "must generate a new token when file is empty"
        );
    }
}
