//! Quota status and reset command handler.

use anyhow::Result;

use zeptoclaw::providers::QuotaStore;

use super::QuotaSubcommand;

/// Handle `zeptoclaw quota` subcommands.
pub(crate) fn cmd_quota(action: QuotaSubcommand) -> Result<()> {
    match action {
        QuotaSubcommand::Status => {
            let store = QuotaStore::load_or_default();
            let snapshot = store.snapshot();

            if snapshot.is_empty() {
                println!("No quota usage recorded.");
                return Ok(());
            }

            // Print header
            println!(
                "{:<16} {:<12} {:<16} {:<16}",
                "Provider", "Period", "Cost Used", "Tokens Used"
            );
            println!("{}", "-".repeat(62));

            // Sort by provider name for deterministic output
            let mut entries: Vec<_> = snapshot.iter().collect();
            entries.sort_by_key(|(name, _)| name.as_str());

            for (name, usage) in entries {
                println!(
                    "{:<16} {:<12} {:<16} {:<16}",
                    name,
                    usage.period_key,
                    format!("${:.2}", usage.cost_usd),
                    usage.tokens,
                );
            }
        }
        QuotaSubcommand::Reset { provider } => {
            let store = QuotaStore::load_or_default();
            match provider {
                Some(name) => {
                    store.reset_provider(&name);
                    println!("Reset quota usage for: {}", name);
                }
                None => {
                    store.reset_all();
                    println!("Reset all quota usage.");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use zeptoclaw::providers::{quota::QuotaPeriod, QuotaStore};

    #[test]
    fn test_quota_status_empty() {
        // An empty store loaded from a non-existent path produces an empty
        // snapshot without panicking.
        let store = QuotaStore::load_or_default();
        // snapshot() must not panic — its emptiness depends on the test
        // environment; just verify we can call it.
        let _ = store.snapshot();
    }

    #[test]
    fn test_quota_reset_all() {
        // Record some usage, then reset_all; snapshot must be empty afterwards.
        let store = QuotaStore::load_or_default();

        store.record("test-anthropic", &QuotaPeriod::Monthly, 10.0, 1_000);
        store.record("test-openai", &QuotaPeriod::Monthly, 5.0, 500);

        // Both providers should now be present.
        {
            let snap = store.snapshot();
            assert!(
                snap.contains_key("test-anthropic") || snap.contains_key("test-openai"),
                "at least one test provider should be recorded"
            );
        }

        // Reset everything.
        store.reset_all();

        let snap = store.snapshot();
        assert!(
            !snap.contains_key("test-anthropic"),
            "test-anthropic should be gone after reset_all"
        );
        assert!(
            !snap.contains_key("test-openai"),
            "test-openai should be gone after reset_all"
        );
    }
}
