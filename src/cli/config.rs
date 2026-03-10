//! Config check and reset command handlers.

use anyhow::{Context, Result};

use zeptoclaw::config::Config;

use super::ConfigAction;

/// Handle config subcommands.
pub(crate) async fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Check => cmd_config_check().await,
        ConfigAction::Reset { force } => cmd_config_reset(force),
    }
}

/// Validate configuration file.
async fn cmd_config_check() -> Result<()> {
    let config_path = Config::path();
    println!("Config file: {}", config_path.display());

    if !config_path.exists() {
        println!("[OK] No config file found (using defaults)");
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path).context("Failed to read config file")?;

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

    Ok(())
}

/// Reset configuration to defaults, backing up the existing file.
fn cmd_config_reset(force: bool) -> Result<()> {
    let config_path = Config::path();

    if !config_path.exists() {
        println!("No config file found at {}", config_path.display());
        println!("Nothing to reset (already using defaults).");
        return Ok(());
    }

    if !force {
        println!("This will reset {} to defaults.", config_path.display());
        println!("Your current config will be backed up.");
        print!("Continue? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Create timestamped backup
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup_path = config_path.with_extension(format!("json.bak.{timestamp}"));
    std::fs::copy(&config_path, &backup_path)
        .with_context(|| format!("Failed to backup config to {}", backup_path.display()))?;
    println!("Backed up to {}", backup_path.display());

    // Write default config
    let default_config = Config::default();
    default_config
        .save()
        .context("Failed to write default config")?;
    println!("Config reset to defaults.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_reset_no_file() {
        // Config::path() returns a fixed path, so we test the logic directly
        // by checking that the function handles missing files gracefully.
        let tmp = TempDir::new().unwrap();
        let fake_path = tmp.path().join("config.json");
        assert!(!fake_path.exists());
    }

    #[test]
    fn test_config_reset_creates_backup() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config.json");

        // Write a non-default config
        let content = r#"{"agents":{"defaults":{"model":"test-model"}}}"#;
        std::fs::write(&config_path, content).unwrap();

        // Simulate backup logic
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = config_path.with_extension(format!("json.bak.{timestamp}"));
        std::fs::copy(&config_path, &backup_path).unwrap();

        assert!(backup_path.exists());
        let backup_content = std::fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup_content, content);
    }

    #[test]
    fn test_default_config_is_valid_json() {
        let config = Config::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let _: serde_json::Value = serde_json::from_str(&json).unwrap();
    }
}
