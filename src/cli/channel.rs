//! CLI channel management commands (zeptoclaw channel list|setup|test).

use std::io::{self, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio_tungstenite::connect_async;

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

    // WhatsApp
    let (wa_status, wa_detail) = match config.channels.whatsapp {
        Some(ref c) if c.enabled => ("enabled", format!("bridge: {}", c.bridge_url)),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<12} {:<10} {}", "whatsapp", wa_status, wa_detail);

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
const KNOWN_CHANNELS: &[&str] = &["telegram", "discord", "slack", "whatsapp", "webhook"];

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
        "whatsapp" => setup_whatsapp(&mut config)?,
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

/// Interactive WhatsApp channel setup.
fn setup_whatsapp(config: &mut Config) -> Result<()> {
    println!();
    println!("WhatsApp Channel Setup (via Bridge)");
    println!("-----------------------------------");
    println!("Requires whatsmeow-rs bridge: https://github.com/qhkm/whatsmeow-rs");
    println!();

    let whatsapp_config = config
        .channels
        .whatsapp
        .get_or_insert_with(Default::default);

    print!("Enable WhatsApp channel? [y/N]: ");
    io::stdout().flush()?;
    let enabled = read_line()?.to_ascii_lowercase();
    if !matches!(enabled.as_str(), "y" | "yes") {
        whatsapp_config.enabled = false;
        println!("  WhatsApp channel disabled.");
        return Ok(());
    }
    whatsapp_config.enabled = true;

    print!("Bridge WebSocket URL [{}]: ", whatsapp_config.bridge_url);
    io::stdout().flush()?;
    let bridge_url = read_line()?;
    if !bridge_url.is_empty() {
        whatsapp_config.bridge_url = bridge_url;
    }

    print!("Phone number allowlist (comma-separated, or Enter for all): ");
    io::stdout().flush()?;
    let allowlist = read_line()?;
    if !allowlist.is_empty() {
        whatsapp_config.allow_from = allowlist
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    println!(
        "  WhatsApp channel configured (bridge: {}).",
        whatsapp_config.bridge_url
    );
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
        "whatsapp" => test_whatsapp(&config).await,
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

/// Test WhatsApp bridge connectivity via WebSocket.
async fn test_whatsapp(config: &Config) -> Result<()> {
    let bridge_url = match config.channels.whatsapp {
        Some(ref c) if c.enabled => {
            if c.bridge_url.is_empty() {
                anyhow::bail!("WhatsApp channel enabled but bridge_url is empty");
            }
            c.bridge_url.clone()
        }
        Some(_) => {
            anyhow::bail!(
                "WhatsApp channel is not enabled. Run 'zeptoclaw channel setup whatsapp' first."
            );
        }
        None => {
            anyhow::bail!(
                "WhatsApp channel not configured. Run 'zeptoclaw channel setup whatsapp' first."
            );
        }
    };

    println!("Testing WhatsApp bridge connection to {}...", bridge_url);

    match tokio::time::timeout(Duration::from_secs(5), connect_async(&bridge_url)).await {
        Ok(Ok((_ws_stream, _))) => {
            println!("WhatsApp bridge reachable at {}", bridge_url);
        }
        Ok(Err(e)) => {
            println!("Failed to connect to WhatsApp bridge: {}", e);
        }
        Err(_) => {
            println!(
                "Connection timed out after 5 seconds. Is the bridge running at {}?",
                bridge_url
            );
        }
    }

    Ok(())
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
        assert!(KNOWN_CHANNELS.contains(&"whatsapp"));
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
    async fn test_channel_test_whatsapp_not_configured() {
        // Default config has no WhatsApp configured
        let config = Config::default();
        let result = test_whatsapp(&config).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not configured"));
    }

    #[tokio::test]
    async fn test_channel_test_whatsapp_disabled() {
        let mut config = Config::default();
        config.channels.whatsapp = Some(zeptoclaw::config::WhatsAppConfig {
            enabled: false,
            bridge_url: "ws://localhost:3001".to_string(),
            bridge_token: None,
            allow_from: vec![],
            bridge_managed: true,
            deny_by_default: true,
        });
        let result = test_whatsapp(&config).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not enabled"));
    }

    #[tokio::test]
    async fn test_channel_test_whatsapp_empty_url() {
        let mut config = Config::default();
        config.channels.whatsapp = Some(zeptoclaw::config::WhatsAppConfig {
            enabled: true,
            bridge_url: String::new(),
            bridge_token: None,
            allow_from: vec![],
            bridge_managed: true,
            deny_by_default: true,
        });
        let result = test_whatsapp(&config).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("bridge_url is empty"));
    }
}
