//! CLI channel management commands (zeptoclaw channel list|setup|test).

use std::io::{self, Write};

use anyhow::{Context, Result};

use zeptoclaw::config::Config;

use super::common::{read_line, read_secret};
use super::ChannelAction;

fn canonical_channel_name(channel_name: &str) -> &str {
    match channel_name {
        "whatsapp" | "whatsapp_web" => "whatsapp_web",
        "whatsapp_cloud" | "whatsapp-cloud" => "whatsapp_cloud",
        _ => channel_name,
    }
}

fn whatsapp_web_available() -> bool {
    cfg!(feature = "whatsapp-web")
}

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
    println!("  {:<15} {:<10} {}", "telegram", tg_status, tg_detail);

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
    println!("  {:<15} {:<10} {}", "discord", dc_status, dc_detail);

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
    println!("  {:<15} {:<10} {}", "slack", sl_status, sl_detail);

    // WhatsApp Web
    let (wa_status, wa_detail) = match config.channels.whatsapp_web {
        Some(ref c) if c.enabled && whatsapp_web_available() => {
            ("enabled", format!("auth: {}", c.auth_dir))
        }
        Some(ref c) if c.enabled => (
            "configured",
            format!("feature not built (auth: {})", c.auth_dir),
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<15} {:<10} {}", "whatsapp_web", wa_status, wa_detail);

    // WhatsApp Cloud
    let (wc_status, wc_detail) = match config.channels.whatsapp_cloud {
        Some(ref c) if c.enabled => (
            "enabled",
            if c.phone_number_id.is_empty() {
                "phone_number_id missing".to_string()
            } else {
                format!("phone: {}", c.phone_number_id)
            },
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<15} {:<10} {}", "whatsapp_cloud", wc_status, wc_detail);

    // Webhook
    let (wh_status, wh_detail) = match config.channels.webhook {
        Some(ref c) if c.enabled => (
            "enabled",
            format!("{}:{}{}", c.bind_address, c.port, c.path),
        ),
        _ => ("disabled", "-".to_string()),
    };
    println!("  {:<15} {:<10} {}", "webhook", wh_status, wh_detail);

    Ok(())
}

// ---------------------------------------------------------------------------
// channel setup
// ---------------------------------------------------------------------------

/// Known channel names for validation.
const KNOWN_CHANNELS: &[&str] = &[
    "telegram",
    "discord",
    "slack",
    "whatsapp",
    "whatsapp_web",
    "whatsapp_cloud",
    "webhook",
];

/// Interactive setup for a named channel.
async fn cmd_channel_setup(channel_name: &str) -> Result<()> {
    let channel_name = canonical_channel_name(channel_name);

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
        "whatsapp_cloud" => setup_whatsapp_cloud(&mut config)?,
        "telegram" => setup_telegram(&mut config)?,
        "discord" => setup_discord(&mut config)?,
        "slack" => setup_slack(&mut config)?,
        "webhook" => setup_webhook(&mut config)?,
        _ => unreachable!(),
    }

    config
        .save()
        .with_context(|| "Failed to save configuration")?;

    Ok(())
}

/// Interactive WhatsApp Web channel setup.
fn setup_whatsapp_web(config: &mut Config) -> Result<()> {
    if !whatsapp_web_available() {
        anyhow::bail!(
            "WhatsApp Web support is not available in this build. Rebuild with --features whatsapp-web."
        );
    }

    println!();
    println!("WhatsApp Web Channel Setup");
    println!("--------------------------");

    let wa_config = config
        .channels
        .whatsapp_web
        .get_or_insert_with(Default::default);
    wa_config.enabled = true;

    print!("Phone number allowlist (comma-separated E.164, e.g. +60123456789, or Enter for all): ");
    io::stdout().flush()?;
    let allowlist = read_line()?;
    wa_config.allow_from = allowlist
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    println!("  WhatsApp Web channel enabled.");
    println!("  Run 'zeptoclaw gateway' to pair via QR code.");
    println!("  On first run, scan the QR code with your phone:");
    println!("    WhatsApp → Settings → Linked Devices → Link a Device");
    Ok(())
}

/// Interactive Telegram channel setup.
fn setup_telegram(config: &mut Config) -> Result<()> {
    println!();
    println!("Telegram Bot Setup");
    println!("------------------");
    println!("To create a bot: Open Telegram, message @BotFather, send /newbot");
    println!();
    print!("Enter Telegram bot token (or press Enter to skip): ");
    io::stdout().flush()?;

    let token = read_secret()?;
    if token.is_empty() {
        println!("  Skipped.");
        return Ok(());
    }

    let tg = config
        .channels
        .telegram
        .get_or_insert_with(Default::default);
    tg.token = token;
    tg.enabled = true;

    print!("Allowlist user IDs/usernames (comma-separated, or Enter for all): ");
    io::stdout().flush()?;
    let allowlist = read_line()?;
    tg.allow_from = allowlist
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    println!("  Telegram bot configured.");
    println!("  Run 'zeptoclaw gateway' to start the bot.");
    Ok(())
}

/// Interactive Discord channel setup.
fn setup_discord(config: &mut Config) -> Result<()> {
    println!();
    println!("Discord Bot Setup");
    println!("-----------------");
    println!("To create a bot:");
    println!("  1. Go to https://discord.com/developers/applications");
    println!("  2. Create New Application → Bot → Reset Token → copy it");
    println!("  3. Enable MESSAGE CONTENT intent under Bot → Privileged Intents");
    println!("  4. Invite bot to your server with OAuth2 URL Generator");
    println!();
    print!("Enter Discord bot token (or press Enter to skip): ");
    io::stdout().flush()?;

    let token = read_secret()?;
    if token.is_empty() {
        println!("  Skipped.");
        return Ok(());
    }

    let dc = config.channels.discord.get_or_insert_with(Default::default);
    dc.token = token;
    dc.enabled = true;

    print!("Allowlist user IDs (comma-separated, or Enter for all): ");
    io::stdout().flush()?;
    let allowlist = read_line()?;
    dc.allow_from = allowlist
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    println!("  Discord bot configured.");
    println!("  Run 'zeptoclaw gateway' to start the bot.");
    Ok(())
}

/// Interactive Slack channel setup.
fn setup_slack(config: &mut Config) -> Result<()> {
    println!();
    println!("Slack Bot Setup");
    println!("---------------");
    println!("To create a bot:");
    println!("  1. Go to https://api.slack.com/apps → Create New App");
    println!("  2. Add Bot Token Scopes: chat:write, app_mentions:read");
    println!("  3. Install to Workspace → copy Bot User OAuth Token (xoxb-...)");
    println!("  4. Generate App-Level Token with connections:write scope");
    println!();
    print!("Enter Slack bot token (xoxb-..., or press Enter to skip): ");
    io::stdout().flush()?;

    let bot_token = read_secret()?;
    if bot_token.is_empty() {
        println!("  Skipped.");
        return Ok(());
    }

    print!("Enter Slack app-level token (xapp-...): ");
    io::stdout().flush()?;
    let app_token = read_secret()?;

    let sl = config.channels.slack.get_or_insert_with(Default::default);
    sl.bot_token = bot_token;
    sl.app_token = app_token;
    sl.enabled = true;

    println!("  Slack bot configured.");
    println!("  Run 'zeptoclaw gateway' to start the bot.");
    Ok(())
}

/// Interactive Webhook channel setup.
fn setup_webhook(config: &mut Config) -> Result<()> {
    println!();
    println!("Webhook Channel Setup");
    println!("---------------------");
    println!("Receives messages via HTTP POST to a local endpoint.");
    println!();

    let wh = config.channels.webhook.get_or_insert_with(Default::default);

    print!("Bind address [{}]: ", wh.bind_address);
    io::stdout().flush()?;
    let bind = read_line()?;
    if !bind.is_empty() {
        wh.bind_address = bind;
    }

    print!("Port [{}]: ", wh.port);
    io::stdout().flush()?;
    let port_str = read_line()?;
    if !port_str.is_empty() {
        if let Ok(p) = port_str.parse::<u16>() {
            wh.port = p;
        } else {
            println!("  Invalid port, keeping default {}.", wh.port);
        }
    }

    print!("Bearer auth token (or Enter for none): ");
    io::stdout().flush()?;
    let auth = read_secret()?;
    if !auth.is_empty() {
        wh.auth_token = Some(auth);
    }

    wh.enabled = true;
    println!(
        "  Webhook configured at {}:{}{}",
        wh.bind_address, wh.port, wh.path
    );
    println!("  Run 'zeptoclaw gateway' to start listening.");
    Ok(())
}

/// Interactive WhatsApp Cloud API channel setup.
fn setup_whatsapp_cloud(config: &mut Config) -> Result<()> {
    println!();
    println!("WhatsApp Cloud API Setup (Official)");
    println!("-----------------------------------");
    println!("Uses Meta's official Cloud API. Requires a Meta Business account.");
    println!("  1. Go to https://developers.facebook.com → Create App → Business");
    println!("  2. Add WhatsApp product → API Setup");
    println!("  3. Copy Phone Number ID and generate a permanent access token");
    println!("  4. Set up a webhook URL (use 'zeptoclaw gateway --tunnel auto')");
    println!();
    print!("Enter Phone Number ID (or press Enter to skip): ");
    io::stdout().flush()?;

    let phone_id = read_line()?;
    if phone_id.is_empty() {
        println!("  Skipped.");
        return Ok(());
    }

    print!("Enter permanent access token: ");
    io::stdout().flush()?;
    let token = read_secret()?;

    print!("Choose a webhook verify token (any secret string): ");
    io::stdout().flush()?;
    let verify_token = read_secret()?;

    let wc = config
        .channels
        .whatsapp_cloud
        .get_or_insert_with(Default::default);
    wc.phone_number_id = phone_id;
    wc.access_token = token;
    wc.webhook_verify_token = verify_token;
    wc.enabled = true;

    println!("  WhatsApp Cloud API configured.");
    println!(
        "  Webhook endpoint: {}:{}{}",
        wc.bind_address, wc.port, wc.path
    );
    println!(
        "  Run 'zeptoclaw gateway' to start, then configure the webhook URL in Meta dashboard."
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// channel test
// ---------------------------------------------------------------------------

/// Test connectivity for a named channel.
async fn cmd_channel_test(channel_name: &str) -> Result<()> {
    let channel_name = canonical_channel_name(channel_name);

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
        "whatsapp_cloud" => match config.channels.whatsapp_cloud {
            Some(ref c) if c.enabled => {
                println!("WhatsApp Cloud API channel is configured and enabled.");
                println!("  Phone Number ID: {}", c.phone_number_id);
                println!("  Webhook: {}:{}{}", c.bind_address, c.port, c.path);
                Ok(())
            }
            _ => {
                anyhow::bail!("WhatsApp Cloud channel not configured. Run 'zeptoclaw channel setup whatsapp_cloud' first.");
            }
        },
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
    if !whatsapp_web_available() {
        anyhow::bail!(
            "WhatsApp Web support is not available in this build. Rebuild with --features whatsapp-web."
        );
    }

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
        assert!(KNOWN_CHANNELS.contains(&"whatsapp"));
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
        // Without whatsapp-web feature: "not available in this build"
        // With whatsapp-web feature but no config: "not configured"
        assert!(
            err_msg.contains("not configured") || err_msg.contains("not available"),
            "unexpected error: {}",
            err_msg
        );
    }
}
