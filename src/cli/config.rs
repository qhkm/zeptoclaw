//! Config check command handler.

use anyhow::{Context, Result};

use zeptoclaw::config::Config;

use super::ConfigAction;

/// Validate configuration file.
pub(crate) async fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Check => {
            let config_path = Config::path();
            println!("Config file: {}", config_path.display());

            if !config_path.exists() {
                println!("[OK] No config file found (using defaults)");
                return Ok(());
            }

            let content =
                std::fs::read_to_string(&config_path).context("Failed to read config file")?;

            let raw: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    println!("[ERROR] Invalid JSON: {}", e);
                    anyhow::bail!("Configuration file is not valid JSON");
                }
            };

            let diagnostics = zeptoclaw::config::validate::validate_config(&raw);
            for diag in &diagnostics {
                println!("{}", diag);
            }

            let errors = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Error)
                .count();
            let mut warnings = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Warn)
                .count();

            // Validate custom tool definitions
            let config = Config::load().unwrap_or_default();
            let tool_warnings = zeptoclaw::config::validate::validate_custom_tools(&config);
            for w in &tool_warnings {
                println!("[WARN] {}", w);
            }
            warnings += tool_warnings.len();

            // Hint: workspace configured but coding tools disabled
            let workspace = config.workspace_path();
            if workspace.exists() && !config.tools.coding_tools {
                println!(
                    "[hint] Workspace is set but coding tools (grep, find) are disabled. \
                     Enable with `tools.coding_tools: true` or use `--template coder`."
                );
            }

            if errors == 0 && warnings == 0 {
                println!("\nConfiguration looks good!");
            } else {
                println!("\nFound {} error(s), {} warning(s)", errors, warnings);
            }

            if errors > 0 {
                anyhow::bail!("Configuration validation failed with {} error(s)", errors);
            }
        }
    }
    Ok(())
}
