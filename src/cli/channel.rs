//! CLI channel management commands (zeptoclaw channel list|setup|test).

use std::io::{self, Write};

use anyhow::{Context, Result};

use zeptoclaw::config::Config;

use super::common::read_line;
use super::ChannelAction;

/// Dispatch channel subcommands.
pub(crate) async fn cmd_channel(action: ChannelAction) -> Result<()> {
    match action {
        ChannelAction::List => cmd_channel_list().await,
        ChannelAction::Setup { channel_name } => cmd_channel_setup(&channel_name).await,
        ChannelAction::Test { channel_name } => cmd_channel_test(&channel_name).await,
    }
}

// ---------------------------------------------------------------------------
// channel list
// ---------------------------------------------------------------------------

/// Display a table of all configured channels with their status.
async fn cmd_channel_list() -> Result<()> {
    let config = Config::load().unwrap_or_default();

    println!("Channels:");

    // Telegram
    let (tg_status, tg_detail) = match config.channels.telegram {
        Some(ref c) if c.enabled => (
            "enabled",
            if c.token.is_empty() {
                "token missing".to_string()
            } else {
                "token configured".to_string()
            },
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<12} {:<10} {}", "telegram", tg_status, tg_detail);

    // Discord
    let (dc_status, dc_detail) = match config.channels.discord {
        Some(ref c) if c.enabled => (
            "enabled",
            if c.token.is_empty() {
                "token missing".to_string()
            } else {
                "token configured".to_string()
            },
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<12} {:<10} {}", "discord", dc_status, dc_detail);

    // Slack
    let (sl_status, sl_detail) = match config.channels.slack {
        Some(ref c) if c.enabled => (
            "enabled",
            if c.bot_token.is_empty() {
                "token missing".to_string()
            } else {
                "token configured".to_string()
            },
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<12} {:<10} {}", "slack", sl_status, sl_detail);

    // WhatsApp Web
    let (wa_status, wa_detail) = match config.channels.whatsapp_web {
        Some(ref c) if c.enabled => ("enabled", format!("auth: {}", c.auth_dir)),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<12} {:<10} {}", "whatsapp_web", wa_status, wa_detail);

    // Webhook
    let (wh_status, wh_detail) = match config.channels.webhook {
        Some(ref c) if c.enabled => (
            "enabled",
            format!("{}:{}{}", c.bind_address, c.port, c.path),
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<12} {:<10} {}", "webhook", wh_status, wh_detail);

    Ok(())
}

// ---------------------------------------------------------------------------
// channel setup
// ---------------------------------------------------------------------------

/// Known channel names for validation.
const KNOWN_CHANNELS: &[&str] = &["telegram", "discord", "slack", "whatsapp_web", "webhook"];

/// Interactive setup for a named channel.
async fn cmd_channel_setup(channel_name: &str) -> Result<()> {
    if !KNOWN_CHANNELS.contains(&channel_name) {
        anyhow::bail!(
            "Unknown channel '{}'. Known channels: {}",
            channel_name,
            KNOWN_CHANNELS.join(", ")
        );
    }

    let mut config = Config::load().unwrap_or_default();

    match channel_name {
        "whatsapp_web" => setup_whatsapp_web(&mut config)?,
        "telegram" => {
            println!("Use 'zeptoclaw onboard' to configure Telegram.");
            return Ok(());
        }
        "discord" => {
            println!("Use 'zeptoclaw onboard' to configure Discord.");
            return Ok(());
        }
        "slack" => {
            println!("Use 'zeptoclaw onboard' to configure Slack.");
            return Ok(());
        }
        "webhook" => {
            println!("Use 'zeptoclaw onboard' to configure Webhook.");
            return Ok(());
        }
        _ => unreachable!(),
    }

    config
        .save()
        .with_context(|| "Failed to save configuration")?;

    Ok(())
}

/// Interactive WhatsApp Web channel setup.
fn setup_whatsapp_web(config: &mut Config) -> Result<()> {
    println!();
    println!("WhatsApp Web Channel Setup (Native)");
    println!("-----------------------------------");
    println!("Uses wa-rs for direct WhatsApp Web protocol support.");
    println!("Requires: cargo build --features whatsapp-web");
    println!();

    let wa_config = config
        .channels
        .whatsapp_web
        .get_or_insert_with(Default::default);

    print!("Enable WhatsApp Web channel? [y/N]: ");
    io::stdout().flush()?;
    let enabled = read_line()?.to_ascii_lowercase();
    if !matches!(enabled.as_str(), "y" | "yes") {
        wa_config.enabled = false;
        println!("  WhatsApp Web channel disabled.");
        return Ok(());
    }
    wa_config.enabled = true;

    print!("Phone number allowlist (comma-separated E.164, or Enter for all): ");
    io::stdout().flush()?;
    let allowlist = read_line()?;
    if !allowlist.is_empty() {
        wa_config.allow_from = allowlist
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    println!("  WhatsApp Web channel configured.");
    println!("  Run 'zeptoclaw gateway' to pair via QR code.");
    Ok(())
}

// ---------------------------------------------------------------------------
// channel test
// ---------------------------------------------------------------------------

/// Test connectivity for a named channel.
async fn cmd_channel_test(channel_name: &str) -> Result<()> {
    if !KNOWN_CHANNELS.contains(&channel_name) {
        anyhow::bail!(
            "Unknown channel '{}'. Known channels: {}",
            channel_name,
            KNOWN_CHANNELS.join(", ")
        );
    }

    let config = Config::load().unwrap_or_default();

    match channel_name {
        "whatsapp_web" => test_whatsapp_web(&config).await,
        "telegram" => {
            println!("Telegram test: not yet implemented (use BotFather /getMe).");
            Ok(())
        }
        "discord" => {
            println!("Discord test: not yet implemented (use Discord API /gateway).");
            Ok(())
        }
        "slack" => {
            println!("Slack test: not yet implemented (use Slack auth.test).");
            Ok(())
        }
        "webhook" => {
            println!("Webhook test: not yet implemented (start server and POST to it).");
            Ok(())
        }
        _ => unreachable!(),
    }
}

/// Test WhatsApp Web channel configuration.
async fn test_whatsapp_web(config: &Config) -> Result<()> {
    match config.channels.whatsapp_web {
        Some(ref c) if c.enabled => {
            println!("WhatsApp Web channel is configured and enabled.");
            println!("  Auth dir: {}", c.auth_dir);
            println!("  Allowlist: {:?}", c.allow_from);
            println!("  Run 'zeptoclaw gateway' to connect and pair.");
            Ok(())
        }
        Some(_) => {
            anyhow::bail!(
                "WhatsApp Web channel is not enabled. Run 'zeptoclaw channel setup whatsapp_web' first."
            );
        }
        None => {
            anyhow::bail!(
                "WhatsApp Web channel not configured. Run 'zeptoclaw channel setup whatsapp_web' first."
            );
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_channels_contains_expected() {
        assert!(KNOWN_CHANNELS.contains(&"telegram"));
        assert!(KNOWN_CHANNELS.contains(&"discord"));
        assert!(KNOWN_CHANNELS.contains(&"slack"));
        assert!(KNOWN_CHANNELS.contains(&"whatsapp_web"));
        assert!(KNOWN_CHANNELS.contains(&"webhook"));
    }

    #[test]
    fn test_known_channels_rejects_unknown() {
        assert!(!KNOWN_CHANNELS.contains(&"irc"));
        assert!(!KNOWN_CHANNELS.contains(&"sms"));
    }

    #[tokio::test]
    async fn test_channel_list_does_not_panic() {
        // This test just verifies cmd_channel_list runs without panicking.
        // It uses the default Config (no channels enabled).
        let result = cmd_channel_list().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_channel_setup_unknown_channel() {
        let result = cmd_channel_setup("irc").await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown channel"));
        assert!(err_msg.contains("irc"));
    }

    #[tokio::test]
    async fn test_channel_test_unknown_channel() {
        let result = cmd_channel_test("sms").await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown channel"));
    }

    #[tokio::test]
    async fn test_channel_test_whatsapp_web_not_configured() {
        let config = Config::default();
        let result = test_whatsapp_web(&config).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not configured"));
    }
}
