//! Provider chain status command handler.

use anyhow::Result;
use zeptoclaw::config::Config;
use zeptoclaw::providers::{resolve_runtime_providers, QuotaStore};

use super::ProviderSubcommand;

/// Handle `zeptoclaw provider` subcommands.
pub(crate) fn cmd_provider(action: ProviderSubcommand) -> Result<()> {
    match action {
        ProviderSubcommand::Status => print_provider_status(),
    }
}

fn redact_key(key: &str) -> String {
    if key.len() <= 8 {
        return "****".to_string();
    }
    let prefix = &key[..key.len().min(8)];
    format!("{}...****", prefix)
}

fn print_provider_status() -> Result<()> {
    let config = Config::load()?;
    let selections = resolve_runtime_providers(&config);

    if selections.is_empty() {
        println!("No providers configured.");
        println!("Set an API key in ~/.zeptoclaw/config.json or via environment variable.");
        return Ok(());
    }

    let default_model = &config.agents.defaults.model;

    println!("\nResolved Providers:");
    println!(
        "{:<15} {:<12} {:<30} {:<20}",
        "Name", "Backend", "Model", "API Key"
    );
    println!("{}", "-".repeat(77));

    for s in &selections {
        let model = s.model.as_deref().unwrap_or(default_model);
        let key = redact_key(&s.api_key);
        println!("{:<15} {:<12} {:<30} {:<20}", s.name, s.backend, model, key);
        if let Some(ref base) = s.api_base {
            println!("  api_base: {}", base);
        }
    }

    println!("\nWrappers:");
    println!(
        "  retry:    {} (max {}, base {}ms, budget {}ms)",
        if config.providers.retry.enabled {
            "enabled"
        } else {
            "disabled"
        },
        config.providers.retry.max_retries,
        config.providers.retry.base_delay_ms,
        config.providers.retry.retry_budget_ms,
    );
    println!(
        "  fallback: {}{}",
        if config.providers.fallback.enabled {
            "enabled"
        } else {
            "disabled"
        },
        config
            .providers
            .fallback
            .provider
            .as_ref()
            .map(|p| format!(" (preferred: {})", p))
            .unwrap_or_default(),
    );

    let store = QuotaStore::load_or_default();
    let snapshot = store.snapshot();
    if !snapshot.is_empty() {
        println!("\nQuota Usage:");
        println!(
            "{:<15} {:<12} {:<15} {:<15}",
            "Provider", "Period", "Cost Used", "Tokens Used"
        );
        println!("{}", "-".repeat(62));
        let mut entries: Vec<_> = snapshot.iter().collect();
        entries.sort_by_key(|(name, _)| name.as_str());
        for (name, usage) in entries {
            println!(
                "{:<15} {:<12} ${:<14.4} {:<15}",
                name, usage.period_key, usage.cost_usd, usage.tokens,
            );
        }
    }

    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_key_short() {
        assert_eq!(redact_key("abc"), "****");
    }

    #[test]
    fn test_redact_key_normal() {
        assert_eq!(redact_key("sk-ant-api03-abcdefghijk"), "sk-ant-a...****");
    }

    #[test]
    fn test_redact_key_exact_boundary() {
        assert_eq!(redact_key("12345678"), "****");
        assert_eq!(redact_key("123456789"), "12345678...****");
    }
}
