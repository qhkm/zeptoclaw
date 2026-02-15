//! Interactive onboarding wizard (zeptoclaw onboard).

use std::io::{self, Write};

use anyhow::{Context, Result};

use zeptoclaw::config::{Config, MemoryBackend, MemoryCitationsMode, RuntimeType};

use super::common::{memory_backend_label, memory_citations_label, read_line, read_secret};

/// Format the express-mode next-steps message.
fn express_next_steps() -> String {
    [
        "",
        "ZeptoClaw ready!",
        "",
        "Try: zeptoclaw agent -m \"What can you help me with?\"",
        "Or:  zeptoclaw agent -m \"Summarize https://news.ycombinator.com\"",
        "",
        "Run 'zeptoclaw onboard --full' for advanced setup (channels, heartbeat, runtime).",
        "Run 'zeptoclaw status' to see your configuration.",
    ]
    .join("\n")
}

/// Initialize configuration directory and save default config.
///
/// When `full` is false (default), runs express mode: creates directories
/// silently, configures the LLM provider, saves, and prints guided next
/// steps.  When `full` is true, runs the full 10-step interactive wizard.
pub(crate) async fn cmd_onboard(full: bool) -> Result<()> {
    // --- common: create directories ---
    let config_dir = Config::dir();
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("Failed to create config directory: {:?}", config_dir))?;

    let workspace_dir = config_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir)
        .with_context(|| format!("Failed to create workspace directory: {:?}", workspace_dir))?;

    let sessions_dir = config_dir.join("sessions");
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("Failed to create sessions directory: {:?}", sessions_dir))?;

    // --- common: load or create config ---
    let config_path = Config::path();
    let mut config = if config_path.exists() {
        Config::load()
            .with_context(|| format!("Failed to load existing config at {:?}", config_path))?
    } else {
        Config::default()
    };

    if full {
        // ---------- full 10-step wizard ----------
        println!("Initializing ZeptoClaw (full wizard)...");
        println!();
        println!("  Config directory: {:?}", config_dir);
        println!("  Workspace directory: {:?}", workspace_dir);
        println!("  Sessions directory: {:?}", sessions_dir);
        if config_path.exists() {
            println!("  Config already exists: {:?}", config_path);
        } else {
            println!("  Creating new config: {:?}", config_path);
        }

        println!();
        println!("API Key Setup");
        println!("=============");
        println!();
        println!("Which AI provider would you like to configure?");
        println!("  1. Anthropic (Claude) - Recommended");
        println!("  2. OpenAI (GPT-4, etc.)");
        println!("  3. Both");
        println!("  4. Skip (configure later)");
        println!();
        print!("Enter choice [1-4]: ");
        io::stdout().flush()?;

        let choice = read_line()?;

        match choice.as_str() {
            "1" | "1." => {
                configure_anthropic(&mut config).await?;
            }
            "2" | "2." => {
                configure_openai(&mut config).await?;
            }
            "3" | "3." => {
                configure_anthropic(&mut config).await?;
                println!();
                configure_openai(&mut config).await?;
            }
            "4" | "4." | "" => {
                println!("Skipping API key setup. You can configure later by:");
                println!("  - Editing {:?}", config_path);
                println!("  - Setting environment variables:");
                println!("    ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...");
                println!("    ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY=sk-...");
            }
            _ => {
                println!("Invalid choice. Skipping API key setup.");
            }
        }

        // Configure web search integration
        configure_web_search(&mut config)?;

        // Configure memory behavior.
        configure_memory(&mut config)?;

        // Configure WhatsApp + Google Sheets tools.
        configure_whatsapp_tool(&mut config)?;
        configure_google_sheets_tool(&mut config)?;

        // Configure heartbeat service.
        configure_heartbeat(&mut config)?;

        // Configure Telegram channel
        configure_telegram(&mut config)?;

        // Configure WhatsApp channel (via bridge)
        configure_whatsapp_channel(&mut config)?;

        // Configure runtime for shell command isolation
        configure_runtime(&mut config)?;

        // Save config
        config
            .save()
            .with_context(|| "Failed to save configuration")?;

        println!();
        println!("ZeptoClaw initialized successfully!");
        println!();
        println!("Next steps:");
        println!("  1. Run 'zeptoclaw agent' to start the interactive agent");
        println!("  2. Run 'zeptoclaw gateway' to start the multi-channel gateway");
        println!("  3. Run 'zeptoclaw status' to check your configuration");
    } else {
        // ---------- express mode (default) ----------
        println!("Initializing ZeptoClaw...");
        println!();

        println!("API Key Setup");
        println!("=============");
        println!();
        println!("Which AI provider would you like to configure?");
        println!("  1. Anthropic (Claude) - Recommended");
        println!("  2. OpenAI (GPT-4, etc.)");
        println!("  3. Both");
        println!("  4. Skip (configure later)");
        println!();
        print!("Enter choice [1-4]: ");
        io::stdout().flush()?;

        let choice = read_line()?;

        match choice.as_str() {
            "1" | "1." => {
                configure_anthropic(&mut config).await?;
            }
            "2" | "2." => {
                configure_openai(&mut config).await?;
            }
            "3" | "3." => {
                configure_anthropic(&mut config).await?;
                println!();
                configure_openai(&mut config).await?;
            }
            "4" | "4." | "" => {
                println!("Skipping API key setup. You can configure later by:");
                println!("  - Editing {:?}", config_path);
                println!("  - Setting environment variables:");
                println!("    ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...");
                println!("    ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY=sk-...");
            }
            _ => {
                println!("Invalid choice. Skipping API key setup.");
            }
        }

        // Save config
        config
            .save()
            .with_context(|| "Failed to save configuration")?;

        // Print guided next steps
        println!("{}", express_next_steps());
    }

    Ok(())
}

/// Configure Brave Search API key for web_search tool.
fn configure_web_search(config: &mut Config) -> Result<()> {
    println!();
    println!("Web Search Setup (Brave)");
    println!("------------------------");
    println!("Get an API key from: https://brave.com/search/api/");
    println!();
    print!("Enter Brave Search API key (or press Enter to skip): ");
    io::stdout().flush()?;

    let api_key = read_secret()?;
    if !api_key.is_empty() {
        config.tools.web.search.api_key = Some(api_key);
        println!("  Brave Search API key configured.");
    } else {
        println!("  Skipped web search API key setup.");
    }

    print!(
        "Default web_search result count [1-10, current={}]: ",
        config.tools.web.search.max_results
    );
    io::stdout().flush()?;

    let count = read_line()?;
    if !count.is_empty() {
        if let Ok(parsed) = count.parse::<u32>() {
            config.tools.web.search.max_results = parsed.clamp(1, 10);
            println!(
                "  Default web_search max_results set to {}.",
                config.tools.web.search.max_results
            );
        } else {
            println!("  Invalid number. Keeping current value.");
        }
    }

    Ok(())
}

/// Configure memory backend and memory tool behavior.
fn configure_memory(config: &mut Config) -> Result<()> {
    println!();
    println!("Memory Setup");
    println!("------------");
    println!("Choose memory backend:");
    println!("  1. Built-in workspace markdown memory (recommended)");
    println!("  2. QMD (planned; currently falls back to built-in)");
    println!("  3. Disabled");
    println!();
    print!(
        "Memory backend [current={}]: ",
        memory_backend_label(&config.memory.backend)
    );
    io::stdout().flush()?;

    let backend_choice = read_line()?;
    if !backend_choice.is_empty() {
        config.memory.backend = match backend_choice.trim() {
            "1" | "builtin" => MemoryBackend::Builtin,
            "2" | "qmd" => MemoryBackend::Qmd,
            "3" | "none" | "disabled" => MemoryBackend::Disabled,
            _ => config.memory.backend.clone(),
        };
    }

    println!();
    println!("Memory citation mode:");
    println!("  1. Auto (CLI on, other channels off)");
    println!("  2. On");
    println!("  3. Off");
    print!(
        "Citation mode [current={}]: ",
        memory_citations_label(&config.memory.citations)
    );
    io::stdout().flush()?;

    let citations_choice = read_line()?;
    if !citations_choice.is_empty() {
        config.memory.citations = match citations_choice.trim() {
            "1" | "auto" => MemoryCitationsMode::Auto,
            "2" | "on" => MemoryCitationsMode::On,
            "3" | "off" => MemoryCitationsMode::Off,
            _ => config.memory.citations.clone(),
        };
    }

    print!(
        "Include default memory files (MEMORY.md + memory/**/*.md)? [{}]: ",
        if config.memory.include_default_memory {
            "Y/n"
        } else {
            "y/N"
        }
    );
    io::stdout().flush()?;

    let include_default = read_line()?.to_ascii_lowercase();
    if !include_default.is_empty() {
        config.memory.include_default_memory = match include_default.as_str() {
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => config.memory.include_default_memory,
        };
    }

    Ok(())
}

/// Configure WhatsApp Cloud API tool credentials.
fn configure_whatsapp_tool(config: &mut Config) -> Result<()> {
    println!();
    println!("WhatsApp Cloud API Tool Setup");
    println!("-----------------------------");
    println!("Get credentials from: https://developers.facebook.com/apps/");
    print!("Enter WhatsApp Phone Number ID (or press Enter to skip): ");
    io::stdout().flush()?;
    let phone_number_id = read_line()?;

    if phone_number_id.is_empty() {
        println!("  Skipped WhatsApp tool setup.");
        return Ok(());
    }

    print!("Enter WhatsApp Access Token: ");
    io::stdout().flush()?;
    let access_token = read_secret()?;
    if access_token.is_empty() {
        println!("  Missing access token, WhatsApp tool not enabled.");
        return Ok(());
    }

    config.tools.whatsapp.phone_number_id = Some(phone_number_id);
    config.tools.whatsapp.access_token = Some(access_token);

    print!(
        "Default WhatsApp template language [current={}]: ",
        config.tools.whatsapp.default_language
    );
    io::stdout().flush()?;
    let lang = read_line()?;
    if !lang.is_empty() {
        config.tools.whatsapp.default_language = lang;
    }

    println!("  WhatsApp tool configured.");
    Ok(())
}

/// Configure Google Sheets tool credentials.
fn configure_google_sheets_tool(config: &mut Config) -> Result<()> {
    println!();
    println!("Google Sheets Tool Setup");
    println!("------------------------");
    println!("Use either an OAuth access token or a base64 payload containing access_token.");
    print!("Enter Google Sheets access token (or press Enter to skip): ");
    io::stdout().flush()?;
    let access_token = read_secret()?;

    if !access_token.is_empty() {
        config.tools.google_sheets.access_token = Some(access_token);
        println!("  Google Sheets access token configured.");
        return Ok(());
    }

    print!("Enter base64 credentials payload (optional): ");
    io::stdout().flush()?;
    let payload = read_line()?;
    if !payload.is_empty() {
        config.tools.google_sheets.service_account_base64 = Some(payload);
        println!("  Google Sheets base64 payload configured.");
    } else {
        println!("  Skipped Google Sheets tool setup.");
    }

    Ok(())
}

/// Configure heartbeat settings.
fn configure_heartbeat(config: &mut Config) -> Result<()> {
    println!();
    println!("Heartbeat Service Setup");
    println!("-----------------------");
    println!("Heartbeat periodically asks the agent to check HEARTBEAT.md.");
    print!(
        "Enable heartbeat service? [{}]: ",
        if config.heartbeat.enabled {
            "Y/n"
        } else {
            "y/N"
        }
    );
    io::stdout().flush()?;
    let enabled = read_line()?.to_ascii_lowercase();

    if !enabled.is_empty() {
        config.heartbeat.enabled = matches!(enabled.as_str(), "y" | "yes");
    }

    if config.heartbeat.enabled {
        print!(
            "Heartbeat interval in minutes [current={}]: ",
            config.heartbeat.interval_secs / 60
        );
        io::stdout().flush()?;
        let minutes = read_line()?;
        if !minutes.is_empty() {
            if let Ok(parsed) = minutes.parse::<u64>() {
                config.heartbeat.interval_secs = (parsed.max(1)) * 60;
            }
        }
        println!("  Heartbeat enabled.");
    } else {
        println!("  Heartbeat disabled.");
    }

    Ok(())
}

/// Configure Anthropic provider.
async fn configure_anthropic(config: &mut Config) -> Result<()> {
    println!();
    println!("Anthropic (Claude) Setup");
    println!("------------------------");
    println!("Get your API key from: https://console.anthropic.com/");
    println!();
    print!("Enter Anthropic API key (or press Enter to skip): ");
    io::stdout().flush()?;

    let api_key = read_secret()?;

    if !api_key.is_empty() {
        print!("  Validating API key...");
        io::stdout().flush()?;
        match super::common::validate_api_key("anthropic", &api_key, None).await {
            Ok(()) => println!(" valid!"),
            Err(e) => {
                println!(" failed.");
                println!("  Warning: {}", e);
                println!("  Saving anyway -- you can fix this later.");
            }
        }
        let provider_config = config
            .providers
            .anthropic
            .get_or_insert_with(Default::default);
        provider_config.api_key = Some(api_key);
        // Set Claude model as default when Anthropic is configured
        config.agents.defaults.model = "claude-sonnet-4-5-20250929".to_string();
        println!("  Anthropic API key configured.");
        println!("  Default model set to: claude-sonnet-4-5-20250929");
    } else {
        println!("  Skipped Anthropic configuration.");
    }

    Ok(())
}

/// Configure OpenAI provider.
async fn configure_openai(config: &mut Config) -> Result<()> {
    println!();
    println!("OpenAI Setup");
    println!("------------");
    println!("Get your API key from: https://platform.openai.com/api-keys");
    println!();
    print!("Enter OpenAI API key (or press Enter to skip): ");
    io::stdout().flush()?;

    let api_key = read_secret()?;

    if !api_key.is_empty() {
        print!("  Validating API key...");
        io::stdout().flush()?;
        // Use custom base URL for validation if one was previously configured
        let existing_base = config
            .providers
            .openai
            .as_ref()
            .and_then(|p| p.api_base.as_deref());
        match super::common::validate_api_key("openai", &api_key, existing_base).await {
            Ok(()) => println!(" valid!"),
            Err(e) => {
                println!(" failed.");
                println!("  Warning: {}", e);
                println!("  Saving anyway -- you can fix this later.");
            }
        }
        let provider_config = config.providers.openai.get_or_insert_with(Default::default);
        provider_config.api_key = Some(api_key);
        // Set OpenAI model as default when OpenAI is configured (and Anthropic isn't)
        if config
            .providers
            .anthropic
            .as_ref()
            .and_then(|p| p.api_key.as_ref())
            .map(|k| k.is_empty())
            .unwrap_or(true)
        {
            config.agents.defaults.model = "gpt-5.1".to_string();
            println!("  Default model set to: gpt-5.1");
        }
        println!("  OpenAI API key configured.");

        // Ask about custom base URL
        println!();
        println!("Do you want to use a custom API base URL?");
        println!("(For Azure OpenAI, local models, or OpenAI-compatible APIs)");
        print!("Enter custom base URL (or press Enter for default): ");
        io::stdout().flush()?;

        let base_url = read_line()?;
        if !base_url.is_empty() {
            provider_config.api_base = Some(base_url);
            println!("  Custom base URL configured.");
        }
    } else {
        println!("  Skipped OpenAI configuration.");
    }

    Ok(())
}

/// Configure Telegram channel.
fn configure_telegram(config: &mut Config) -> Result<()> {
    println!();
    println!("Telegram Bot Setup");
    println!("------------------");
    println!("To create a bot: Open Telegram, message @BotFather, send /newbot");
    println!();
    print!("Enter Telegram bot token (or press Enter to skip): ");
    io::stdout().flush()?;

    let token = read_line()?;

    if !token.is_empty() {
        let telegram_config = config
            .channels
            .telegram
            .get_or_insert_with(Default::default);
        telegram_config.token = token;
        telegram_config.enabled = true;
        println!("  Telegram bot configured.");
        println!("  Run 'zeptoclaw gateway' to start the bot.");
    } else {
        println!("  Skipped Telegram configuration.");
    }

    Ok(())
}

/// Configure WhatsApp channel (via whatsmeow-rs bridge).
fn configure_whatsapp_channel(config: &mut Config) -> Result<()> {
    println!();
    println!("WhatsApp Channel Setup (via Bridge)");
    println!("-----------------------------------");
    println!("Requires whatsmeow-rs bridge: https://github.com/qhkm/whatsmeow-rs");
    print!("Enable WhatsApp channel? [y/N]: ");
    io::stdout().flush()?;

    let enabled = read_line()?.to_ascii_lowercase();
    if !matches!(enabled.as_str(), "y" | "yes") {
        println!("  Skipped WhatsApp channel configuration.");
        return Ok(());
    }

    let whatsapp_config = config
        .channels
        .whatsapp
        .get_or_insert_with(Default::default);
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
    println!("  Run 'zeptoclaw gateway' to start the WhatsApp channel.");
    Ok(())
}

/// Configure runtime for shell command isolation.
fn configure_runtime(config: &mut Config) -> Result<()> {
    println!();
    println!("=== Runtime Configuration ===");
    println!("Choose container runtime for shell command isolation:");
    println!("  1. Native (no container, uses application-level security)");
    println!("  2. Docker (requires Docker installed)");
    #[cfg(target_os = "macos")]
    println!("  3. Apple Container (macOS 15+ only)");
    println!();

    loop {
        print!("Enter choice [1]: ");
        io::stdout().flush()?;

        let choice = read_line()?.trim().to_string();
        let choice = if choice.is_empty() { "1" } else { &choice };

        match choice {
            "1" => {
                config.runtime.runtime_type = RuntimeType::Native;
                config.runtime.allow_fallback_to_native = false;
                println!("Configured: Native runtime (no container isolation)");
                break;
            }
            "2" => {
                config.runtime.runtime_type = RuntimeType::Docker;
                print!("Docker image [alpine:latest]: ");
                io::stdout().flush()?;
                let image = read_line()?.trim().to_string();
                if !image.is_empty() {
                    config.runtime.docker.image = image;
                }
                println!(
                    "Configured: Docker runtime with image {}",
                    config.runtime.docker.image
                );

                print!("Allow fallback to native if Docker is unavailable? [y/N]: ");
                io::stdout().flush()?;
                let fallback = read_line()?.trim().to_lowercase();
                config.runtime.allow_fallback_to_native = matches!(fallback.as_str(), "y" | "yes");
                if config.runtime.allow_fallback_to_native {
                    println!(
                        "Fallback enabled: native runtime will be used if Docker is unavailable."
                    );
                } else {
                    println!("Fallback disabled: startup will fail if Docker is unavailable.");
                }
                break;
            }
            #[cfg(target_os = "macos")]
            "3" => {
                config.runtime.runtime_type = RuntimeType::AppleContainer;
                println!("Configured: Apple Container runtime");

                print!("Allow fallback to native if Apple Container is unavailable? [y/N]: ");
                io::stdout().flush()?;
                let fallback = read_line()?.trim().to_lowercase();
                config.runtime.allow_fallback_to_native = matches!(fallback.as_str(), "y" | "yes");
                if config.runtime.allow_fallback_to_native {
                    println!(
                        "Fallback enabled: native runtime will be used if Apple Container is unavailable."
                    );
                } else {
                    println!(
                        "Fallback disabled: startup will fail if Apple Container is unavailable."
                    );
                }
                break;
            }
            _ => {
                println!("Invalid choice. Please try again.");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_express_next_steps_message() {
        let msg = express_next_steps();
        assert!(msg.contains("ZeptoClaw ready!"));
        assert!(msg.contains("zeptoclaw agent -m"));
        assert!(msg.contains("zeptoclaw onboard --full"));
        assert!(msg.contains("zeptoclaw status"));
        assert!(msg.contains("Summarize https://news.ycombinator.com"));
    }
}
