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
            let warnings = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Warn)
                .count();

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
