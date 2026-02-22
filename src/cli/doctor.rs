//! Doctor — system diagnostics for ZeptoClaw.

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use zeptoclaw::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Err,
}

impl Severity {
    pub fn icon(&self) -> &'static str {
        match self {
            Severity::Ok => "[ok]",
            Severity::Warn => "[warn]",
            Severity::Err => "[ERR]",
        }
    }
}

pub struct DiagItem {
    pub severity: Severity,
    pub category: &'static str,
    pub message: String,
}

pub fn run_diagnostics(config: &Config, online: bool) -> Vec<DiagItem> {
    let mut diags = Vec::new();

    check_config(config, &mut diags);
    check_workspace_writable(&config.workspace_path(), &mut diags);
    check_environment(&mut diags);
    check_providers(config, &mut diags);
    check_channels(config, &mut diags);
    check_memory(&mut diags);

    if online {
        check_provider_connectivity(config, &mut diags);
    }

    diags
}

fn check_config(config: &Config, diags: &mut Vec<DiagItem>) {
    diags.push(DiagItem {
        severity: Severity::Ok,
        category: "config",
        message: "Configuration loaded successfully".into(),
    });

    let temp = config.agents.defaults.temperature;
    if !(0.0..=2.0).contains(&temp) {
        diags.push(DiagItem {
            severity: Severity::Warn,
            category: "config",
            message: format!("Temperature {} is outside typical range 0.0-2.0", temp),
        });
    }
}

fn check_workspace_writable(workspace: &Path, diags: &mut Vec<DiagItem>) {
    if !workspace.exists() {
        diags.push(DiagItem {
            severity: Severity::Err,
            category: "workspace",
            message: format!(
                "Workspace directory does not exist: {}",
                workspace.display()
            ),
        });
        return;
    }

    let probe = workspace.join(".zeptoclaw_doctor_probe");
    match std::fs::write(&probe, b"probe") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            diags.push(DiagItem {
                severity: Severity::Ok,
                category: "workspace",
                message: format!("Workspace writable: {}", workspace.display()),
            });
        }
        Err(e) => {
            diags.push(DiagItem {
                severity: Severity::Err,
                category: "workspace",
                message: format!("Workspace not writable: {} ({})", workspace.display(), e),
            });
        }
    }
}

fn check_environment(diags: &mut Vec<DiagItem>) {
    for binary in &["git", "curl", "sh"] {
        check_binary(binary, diags);
    }
}

pub fn check_binary(name: &str, diags: &mut Vec<DiagItem>) {
    match Command::new("which").arg(name).output() {
        Ok(output) if output.status.success() => {
            diags.push(DiagItem {
                severity: Severity::Ok,
                category: "environment",
                message: format!("{} found", name),
            });
        }
        _ => {
            diags.push(DiagItem {
                severity: Severity::Warn,
                category: "environment",
                message: format!("{} not found in PATH", name),
            });
        }
    }
}

pub fn check_providers(config: &Config, diags: &mut Vec<DiagItem>) {
    let mut any_configured = false;

    let named_providers = [
        ("Anthropic", &config.providers.anthropic),
        ("OpenAI", &config.providers.openai),
        ("OpenRouter", &config.providers.openrouter),
        ("Groq", &config.providers.groq),
    ];

    for (label, provider) in &named_providers {
        if let Some(ref p) = provider {
            if p.api_key.as_ref().is_some_and(|k| !k.is_empty()) {
                any_configured = true;
                diags.push(DiagItem {
                    severity: Severity::Ok,
                    category: "providers",
                    message: format!("{} API key configured", label),
                });
            }
        }
    }

    if !any_configured {
        diags.push(DiagItem {
            severity: Severity::Warn,
            category: "providers",
            message: "No provider API keys configured — add at least one to use the agent".into(),
        });
    }
}

pub fn check_channels(config: &Config, diags: &mut Vec<DiagItem>) {
    let mut any_enabled = false;

    if let Some(ref tg) = config.channels.telegram {
        if tg.enabled {
            any_enabled = true;
            if tg.token.is_empty() {
                diags.push(DiagItem {
                    severity: Severity::Err,
                    category: "channels",
                    message: "Telegram enabled but bot token is empty".into(),
                });
            } else {
                diags.push(DiagItem {
                    severity: Severity::Ok,
                    category: "channels",
                    message: "Telegram configured".into(),
                });
            }
        }
    }

    if let Some(ref dc) = config.channels.discord {
        if dc.enabled {
            any_enabled = true;
            if dc.token.is_empty() {
                diags.push(DiagItem {
                    severity: Severity::Err,
                    category: "channels",
                    message: "Discord enabled but token is empty".into(),
                });
            } else {
                diags.push(DiagItem {
                    severity: Severity::Ok,
                    category: "channels",
                    message: "Discord configured".into(),
                });
            }
        }
    }

    if !any_enabled {
        diags.push(DiagItem {
            severity: Severity::Warn,
            category: "channels",
            message: "No channels enabled (CLI-only mode)".into(),
        });
    }
}

pub fn check_memory(diags: &mut Vec<DiagItem>) {
    let ltm_path = Config::dir().join("memory").join("longterm.json");
    if ltm_path.exists() {
        match std::fs::read_to_string(&ltm_path) {
            Ok(_) => {
                diags.push(DiagItem {
                    severity: Severity::Ok,
                    category: "memory",
                    message: "Long-term memory file readable".into(),
                });
            }
            Err(e) => {
                diags.push(DiagItem {
                    severity: Severity::Err,
                    category: "memory",
                    message: format!("Long-term memory file unreadable: {}", e),
                });
            }
        }
    } else {
        diags.push(DiagItem {
            severity: Severity::Ok,
            category: "memory",
            message: "No long-term memory file yet (created on first use)".into(),
        });
    }
}

fn check_provider_connectivity(_config: &Config, diags: &mut Vec<DiagItem>) {
    diags.push(DiagItem {
        severity: Severity::Warn,
        category: "connectivity",
        message: "Online provider connectivity check not yet implemented".into(),
    });
}

/// CLI entry point.
pub(crate) async fn cmd_doctor(online: bool) -> Result<()> {
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            println!("[ERR] config    Failed to load config: {}", e);
            println!();
            println!("Run `zeptoclaw onboard` to create a configuration.");
            return Ok(());
        }
    };

    let diags = run_diagnostics(&config, online);

    println!("ZeptoClaw Doctor");
    println!("================");
    println!();

    let mut current_category = "";
    for diag in &diags {
        if diag.category != current_category {
            if !current_category.is_empty() {
                println!();
            }
            current_category = diag.category;
        }
        println!(
            "{:<6} {:<14} {}",
            diag.severity.icon(),
            diag.category,
            diag.message
        );
    }

    println!();
    let errors = diags.iter().filter(|d| d.severity == Severity::Err).count();
    let warnings = diags
        .iter()
        .filter(|d| d.severity == Severity::Warn)
        .count();
    let ok = diags.iter().filter(|d| d.severity == Severity::Ok).count();
    println!("{} ok, {} warnings, {} errors", ok, warnings, errors);

    if errors > 0 {
        println!();
        println!("Fix the errors above to ensure ZeptoClaw works correctly.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_display() {
        assert_eq!(Severity::Ok.icon(), "[ok]");
        assert_eq!(Severity::Warn.icon(), "[warn]");
        assert_eq!(Severity::Err.icon(), "[ERR]");
    }

    #[test]
    fn test_check_config_exists_ok() {
        let mut diags = Vec::new();
        let config = Config::default();
        check_config(&config, &mut diags);
        assert!(!diags.is_empty());
    }

    #[test]
    fn test_check_workspace_writable() {
        let mut diags = Vec::new();
        let temp = std::env::temp_dir();
        check_workspace_writable(&temp, &mut diags);
        assert!(diags.iter().any(|d| d.severity == Severity::Ok));
    }

    #[test]
    fn test_check_workspace_nonexistent() {
        let mut diags = Vec::new();
        let fake = std::path::PathBuf::from("/nonexistent/path/12345");
        check_workspace_writable(&fake, &mut diags);
        assert!(diags.iter().any(|d| d.severity == Severity::Err));
    }

    #[test]
    fn test_check_binary_present() {
        let mut diags = Vec::new();
        check_binary("sh", &mut diags);
        assert!(diags.iter().any(|d| d.severity == Severity::Ok));
    }

    #[test]
    fn test_check_binary_missing() {
        let mut diags = Vec::new();
        check_binary("nonexistent_binary_xyz_12345", &mut diags);
        assert!(diags.iter().any(|d| d.severity == Severity::Warn));
    }

    #[test]
    fn test_check_provider_no_key() {
        let mut diags = Vec::new();
        let config = Config::default();
        check_providers(&config, &mut diags);
        assert!(diags.iter().any(|d| d.severity == Severity::Warn));
    }

    #[test]
    fn test_check_channels_none_enabled() {
        let mut diags = Vec::new();
        let config = Config::default();
        check_channels(&config, &mut diags);
        assert!(!diags.is_empty());
    }

    #[test]
    fn test_check_memory_accessible() {
        let mut diags = Vec::new();
        check_memory(&mut diags);
        assert!(!diags.is_empty());
    }

    #[test]
    fn test_run_diagnostics_returns_results() {
        let config = Config::default();
        let diags = run_diagnostics(&config, false);
        assert!(!diags.is_empty());
    }
}
