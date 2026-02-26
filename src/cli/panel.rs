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

/// Resolve the panel dist directory (for serving static assets).
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

/// Resolve the panel source directory (for building — looks for `package.json`).
///
/// Checks two locations in order:
/// 1. `./panel/` — local repo checkout (development)
/// 2. `~/.zeptoclaw/panel/` — user-level installed source
fn resolve_panel_source_dir() -> Option<PathBuf> {
    let local = PathBuf::from("panel");
    if local.join("package.json").exists() {
        return Some(local);
    }
    if let Some(home) = dirs::home_dir() {
        let global = home.join(".zeptoclaw/panel");
        if global.join("package.json").exists() {
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
    let mut state = AppState::new(api_token.clone(), event_bus);

    // Wire in the TaskStore so kanban endpoints return real data.
    let task_store_path = Config::dir().join("tasks.json");
    let task_store = std::sync::Arc::new(zeptoclaw::api::tasks::TaskStore::new(task_store_path));
    if let Err(e) = task_store.load().await {
        tracing::warn!("Failed to load task store: {e}");
    }
    state.task_store = Some(task_store);

    println!(
        "Panel API:      http://{}:{}",
        panel_config.bind, panel_config.api_port
    );
    if !dev && !api_only && static_dir.is_some() {
        println!(
            "Panel Frontend: http://{}:{}",
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
async fn cmd_install(download: bool, rebuild: bool) -> Result<()> {
    if download {
        anyhow::bail!(
            "Downloading pre-built panel is not yet implemented. \
             Use `zeptoclaw panel install` (without --download) to build from source."
        );
    } else {
        println!("Installing ZeptoClaw Panel...\n");

        // ------------------------------------------------------------------
        // 1. Check Node.js >= 18
        // ------------------------------------------------------------------
        let node_output = tokio::process::Command::new("node")
            .arg("--version")
            .output()
            .await;

        match node_output {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();

                // Parse "vMAJOR.MINOR.PATCH" → major integer.
                let major: u32 = version
                    .trim_start_matches('v')
                    .split('.')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                if major < 18 {
                    anyhow::bail!(
                        "Node.js >= 18 required (found {}). \
                         Install the LTS release from https://nodejs.org",
                        version
                    );
                }

                println!("  Node.js: {version}");
            }
            _ => {
                anyhow::bail!(
                    "Node.js not found. \
                     Install Node.js >= 18 from https://nodejs.org"
                );
            }
        }

        // ------------------------------------------------------------------
        // 2. Check pnpm; enable via corepack if absent
        // ------------------------------------------------------------------
        let pnpm_output = tokio::process::Command::new("pnpm")
            .arg("--version")
            .output()
            .await;

        match pnpm_output {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                println!("  pnpm: {version}");
            }
            _ => {
                println!("  pnpm not found, enabling via corepack...");
                let corepack_status = tokio::process::Command::new("corepack")
                    .args(["enable", "pnpm"])
                    .status()
                    .await;

                match corepack_status {
                    Ok(status) if status.success() => {
                        println!("  pnpm enabled via corepack.");
                    }
                    _ => {
                        anyhow::bail!(
                            "Failed to enable pnpm via corepack. \
                             Install pnpm manually: https://pnpm.io/installation"
                        );
                    }
                }
            }
        }

        // ------------------------------------------------------------------
        // 3. Locate panel source directory
        // ------------------------------------------------------------------
        let panel_dir = resolve_panel_source_dir().ok_or_else(|| {
            anyhow::anyhow!(
                "Panel source not found (no package.json in ./panel/ or \
                     ~/.zeptoclaw/panel/). Clone the repo or run from the \
                     ZeptoClaw source directory."
            )
        })?;

        println!("  Panel source: {}", panel_dir.display());

        // ------------------------------------------------------------------
        // 4. Check if already built and skip if rebuild is not requested
        // ------------------------------------------------------------------
        let dist_dir = panel_dir.join("dist");
        let already_built = dist_dir.join("index.html").exists();

        if already_built && !rebuild {
            println!("\n  Panel is already installed. Use --rebuild to force a rebuild.");
        } else {
            // --------------------------------------------------------------
            // 5. Install Node dependencies
            // --------------------------------------------------------------
            println!("\n  Installing dependencies...");
            let install_status = tokio::process::Command::new("pnpm")
                .arg("install")
                .current_dir(&panel_dir)
                .status()
                .await
                .with_context(|| "Failed to spawn pnpm install")?;

            if !install_status.success() {
                anyhow::bail!("pnpm install failed in {}", panel_dir.display());
            }

            // --------------------------------------------------------------
            // 6. Build the frontend
            // --------------------------------------------------------------
            println!("  Building frontend...");
            let build_status = tokio::process::Command::new("pnpm")
                .arg("build")
                .current_dir(&panel_dir)
                .status()
                .await
                .with_context(|| "Failed to spawn pnpm build")?;

            if !build_status.success() {
                anyhow::bail!("pnpm build failed in {}", panel_dir.display());
            }
        }
    }

    // ------------------------------------------------------------------
    // 7. Ensure an API token exists (generate + persist if missing)
    // ------------------------------------------------------------------
    let tp = token_path();
    let token = ensure_api_token(&tp).await?;

    println!("\n  Panel installed successfully!");
    println!("  API token: {token}");
    println!("\n  Start with: zeptoclaw panel");

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
            anyhow::bail!(
                "Setting auth mode to '{mode}' is not yet implemented. \
                 Edit ~/.zeptoclaw/config.json manually to set panel.auth_mode."
            );
        }
        PanelAuthAction::ResetPassword => {
            anyhow::bail!(
                "Password reset is not yet implemented. \
                 Edit ~/.zeptoclaw/config.json manually to set panel.password_hash."
            );
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
    fn test_resolve_panel_source_dir_missing() {
        // In CI / test environments without a panel checkout, this returns None.
        // The important invariant is that it does not panic.
        let _ = resolve_panel_source_dir();
    }

    #[test]
    fn test_resolve_panel_source_dir_local_package_json() {
        // Create a temporary directory tree that mimics `./panel/package.json`
        // and verify the function discovers it.
        use std::env;
        use tempfile::tempdir;

        let tmp = tempdir().unwrap();
        let panel_dir = tmp.path().join("panel");
        std::fs::create_dir_all(&panel_dir).unwrap();
        std::fs::write(panel_dir.join("package.json"), "{}").unwrap();

        // Change CWD so "./panel/package.json" resolves inside `tmp`.
        let original_cwd = env::current_dir().unwrap();
        env::set_current_dir(tmp.path()).unwrap();

        let found = resolve_panel_source_dir();

        // Restore CWD regardless of assertion outcome.
        env::set_current_dir(original_cwd).unwrap();

        assert!(found.is_some(), "must find panel/package.json under CWD");
        assert!(
            found.unwrap().join("package.json").exists(),
            "resolved dir must contain package.json"
        );
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

    /// The Node.js version string parsing used in cmd_install must correctly
    /// extract the major version component from the canonical "vMAJOR.MINOR.PATCH"
    /// format emitted by `node --version`.
    #[test]
    fn test_node_version_major_parsing() {
        let parse_major = |ver: &str| -> u32 {
            ver.trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };

        assert_eq!(parse_major("v20.11.1"), 20);
        assert_eq!(parse_major("v18.0.0"), 18);
        assert_eq!(parse_major("v16.20.2"), 16);
        assert_eq!(
            parse_major("v22.3.0\n"),
            22,
            "trailing newline must be handled"
        );
        // Malformed / absent version falls back to 0, triggering the error path.
        assert_eq!(parse_major("not-a-version"), 0);
        assert_eq!(parse_major(""), 0);
    }

    #[test]
    fn test_node_version_major_minimum_check() {
        // Versions below 18 must be rejected.
        let is_sufficient = |ver: &str| -> bool {
            let major: u32 = ver
                .trim()
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            major >= 18
        };

        assert!(!is_sufficient("v17.9.1"), "v17 is below minimum");
        assert!(!is_sufficient("v0.12.0"), "v0 is below minimum");
        assert!(is_sufficient("v18.0.0"), "v18 meets minimum exactly");
        assert!(is_sufficient("v20.11.1"), "v20 exceeds minimum");
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
