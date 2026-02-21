//! CLI commands for device pairing management.

use anyhow::Result;
use zeptoclaw::config::Config;
use zeptoclaw::security::PairingManager;

use super::PairAction;

/// Dispatch pairing subcommands.
pub(crate) async fn cmd_pair(action: PairAction) -> Result<()> {
    let config = Config::load()?;

    match action {
        PairAction::New => cmd_pair_new(&config).await,
        PairAction::List => cmd_pair_list(&config).await,
        PairAction::Revoke { device } => cmd_pair_revoke(&config, &device).await,
    }
}

/// Generate a new pairing code and display it.
async fn cmd_pair_new(config: &Config) -> Result<()> {
    let mut mgr = PairingManager::new(config.pairing.max_attempts, config.pairing.lockout_secs);

    let code = mgr.generate_pairing_code();
    println!("Pairing code: {}", code);
    println!();
    println!("This code is valid for 5 minutes.");
    println!("Use it to pair a device by sending:");
    println!("  Authorization: Bearer <token-from-pairing>");
    println!();

    // Wait for the device to complete pairing (interactive mode)
    println!("Waiting for device to pair (press Ctrl+C to cancel)...");
    println!();
    println!("To complete pairing from another terminal or device:");
    println!(
        "  curl -X POST http://localhost:{}/pair \\",
        config.gateway.port
    );
    println!("    -H 'Content-Type: application/json' \\");
    println!(
        "    -d '{{\"code\": \"{}\", \"device_name\": \"my-device\"}}'",
        code
    );

    Ok(())
}

/// List all paired devices.
async fn cmd_pair_list(config: &Config) -> Result<()> {
    let mgr = PairingManager::new(config.pairing.max_attempts, config.pairing.lockout_secs);

    let devices = mgr.list_devices();
    if devices.is_empty() {
        println!("No paired devices.");
        return Ok(());
    }

    println!("{:<20} {:<24} {:<24}", "DEVICE", "PAIRED AT", "LAST SEEN");
    println!("{}", "-".repeat(68));
    for device in &devices {
        let paired = format_timestamp(device.paired_at);
        let seen = format_timestamp(device.last_seen);
        println!("{:<20} {:<24} {:<24}", device.name, paired, seen);
    }
    println!();
    println!("{} device(s) paired.", devices.len());

    Ok(())
}

/// Revoke a paired device by name.
async fn cmd_pair_revoke(config: &Config, device_name: &str) -> Result<()> {
    let mut mgr = PairingManager::new(config.pairing.max_attempts, config.pairing.lockout_secs);

    if mgr.revoke(device_name) {
        println!("Device '{}' has been revoked.", device_name);
    } else {
        println!("Device '{}' not found.", device_name);
    }

    Ok(())
}

/// Format a unix timestamp as a human-readable string.
fn format_timestamp(ts: u64) -> String {
    if ts == 0 {
        return "never".to_string();
    }
    // Simple ISO-ish format using system time
    let duration = std::time::Duration::from_secs(ts);
    let datetime = std::time::UNIX_EPOCH + duration;
    match datetime.elapsed() {
        Ok(ago) => {
            let secs = ago.as_secs();
            if secs < 60 {
                format!("{}s ago", secs)
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else if secs < 86400 {
                format!("{}h ago", secs / 3600)
            } else {
                format!("{}d ago", secs / 86400)
            }
        }
        Err(_) => "just now".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp_zero() {
        assert_eq!(format_timestamp(0), "never");
    }

    #[test]
    fn test_format_timestamp_recent() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let result = format_timestamp(now);
        // Should be "just now" or "Xs ago"
        assert!(
            result.contains("ago") || result == "just now",
            "Unexpected: {}",
            result
        );
    }
}
