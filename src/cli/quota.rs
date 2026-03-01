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
                    let normalized = name.trim().to_lowercase();
                    store.reset_provider(&normalized);
                    println!("Reset quota usage for: {}", normalized);
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
    use tempfile::TempDir;
    use zeptoclaw::providers::{quota::QuotaPeriod, QuotaStore};

    #[test]
    fn test_quota_status_empty() {
        let dir = TempDir::new().unwrap();
        let store = QuotaStore::load_from_dir(dir.path());
        let snap = store.snapshot();
        assert!(snap.is_empty(), "fresh store should be empty");
    }

    #[test]
    fn test_quota_reset_all() {
        let dir = TempDir::new().unwrap();
        let store = QuotaStore::load_from_dir(dir.path());

        store.record("test-anthropic", &QuotaPeriod::Monthly, 10.0, 1_000);
        store.record("test-openai", &QuotaPeriod::Monthly, 5.0, 500);

        {
            let snap = store.snapshot();
            assert!(snap.contains_key("test-anthropic"), "should be recorded");
            assert!(snap.contains_key("test-openai"), "should be recorded");
        }

        store.reset_all();

        let snap = store.snapshot();
        assert!(
            !snap.contains_key("test-anthropic"),
            "should be gone after reset_all"
        );
        assert!(
            !snap.contains_key("test-openai"),
            "should be gone after reset_all"
        );
    }
}
