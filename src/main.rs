//! ZeptoClaw CLI - Ultra-lightweight personal AI assistant
//!
//! This is the main entry point for the ZeptoClaw command-line interface.
//! It provides commands for running the AI agent in interactive mode,
//! starting the multi-channel gateway, and managing configuration.

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use zeptoclaw::agent::AgentLoop;
use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::channels::{register_configured_channels, ChannelManager};
use zeptoclaw::config::{Config, RuntimeType};
use zeptoclaw::cron::CronService;
use zeptoclaw::providers::{
    configured_provider_names, configured_unsupported_provider_names, resolve_runtime_provider,
    ClaudeProvider, OpenAIProvider, RUNTIME_SUPPORTED_PROVIDERS,
};
use zeptoclaw::runtime::{available_runtimes, create_runtime, NativeRuntime};
use zeptoclaw::session::SessionManager;
use zeptoclaw::tools::cron::CronTool;
use zeptoclaw::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
use zeptoclaw::tools::shell::ShellTool;
use zeptoclaw::tools::spawn::SpawnTool;
use zeptoclaw::tools::EchoTool;

#[derive(Parser)]
#[command(name = "zeptoclaw")]
#[command(about = "Ultra-lightweight personal AI assistant", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration and workspace
    Onboard,
    /// Start interactive agent mode
    Agent {
        /// Direct message to process (non-interactive mode)
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Start multi-channel gateway
    Gateway,
    /// Manage authentication
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Show version information
    Version,
    /// Show system status
    Status,
}

#[derive(Subcommand)]
enum AuthAction {
    /// Log in to AI provider
    Login,
    /// Log out from AI provider
    Logout,
    /// Show authentication status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Version) | None => {
            cmd_version();
        }
        Some(Commands::Onboard) => {
            cmd_onboard().await?;
        }
        Some(Commands::Agent { message }) => {
            cmd_agent(message).await?;
        }
        Some(Commands::Gateway) => {
            cmd_gateway().await?;
        }
        Some(Commands::Auth { action }) => {
            cmd_auth(action).await?;
        }
        Some(Commands::Status) => {
            cmd_status().await?;
        }
    }

    Ok(())
}

/// Display version information
fn cmd_version() {
    println!("zeptoclaw {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Ultra-lightweight personal AI assistant framework");
    println!("https://github.com/zeptoclaw/zeptoclaw");
}

/// Read a line from stdin, trimming whitespace
fn read_line() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .with_context(|| "Failed to read input")?;
    Ok(input.trim().to_string())
}

/// Read a password/API key from stdin (no echo if possible)
fn read_secret() -> Result<String> {
    // For now, just read normally. Could use rpassword crate for hidden input.
    read_line()
}

/// Initialize configuration directory and save default config
async fn cmd_onboard() -> Result<()> {
    println!("Initializing ZeptoClaw...");
    println!();

    // Create config directory
    let config_dir = Config::dir();
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("Failed to create config directory: {:?}", config_dir))?;
    println!("  Created config directory: {:?}", config_dir);

    // Create workspace directory
    let workspace_dir = config_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir)
        .with_context(|| format!("Failed to create workspace directory: {:?}", workspace_dir))?;
    println!("  Created workspace directory: {:?}", workspace_dir);

    // Create sessions directory
    let sessions_dir = config_dir.join("sessions");
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("Failed to create sessions directory: {:?}", sessions_dir))?;
    println!("  Created sessions directory: {:?}", sessions_dir);

    // Load existing config or create default
    let config_path = Config::path();
    let mut config = if config_path.exists() {
        println!("  Config already exists: {:?}", config_path);
        Config::load().unwrap_or_default()
    } else {
        println!("  Creating new config: {:?}", config_path);
        Config::default()
    };

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
            configure_anthropic(&mut config)?;
        }
        "2" | "2." => {
            configure_openai(&mut config)?;
        }
        "3" | "3." => {
            configure_anthropic(&mut config)?;
            println!();
            configure_openai(&mut config)?;
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

    // Configure Telegram channel
    configure_telegram(&mut config)?;

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

    Ok(())
}

/// Configure Anthropic provider
fn configure_anthropic(config: &mut Config) -> Result<()> {
    println!();
    println!("Anthropic (Claude) Setup");
    println!("------------------------");
    println!("Get your API key from: https://console.anthropic.com/");
    println!();
    print!("Enter Anthropic API key (or press Enter to skip): ");
    io::stdout().flush()?;

    let api_key = read_secret()?;

    if !api_key.is_empty() {
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

/// Configure OpenAI provider
fn configure_openai(config: &mut Config) -> Result<()> {
    println!();
    println!("OpenAI Setup");
    println!("------------");
    println!("Get your API key from: https://platform.openai.com/api-keys");
    println!();
    print!("Enter OpenAI API key (or press Enter to skip): ");
    io::stdout().flush()?;

    let api_key = read_secret()?;

    if !api_key.is_empty() {
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
            config.agents.defaults.model = "gpt-4o".to_string();
            println!("  Default model set to: gpt-4o");
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

/// Configure Telegram channel
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

/// Configure runtime for shell command isolation
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

/// Create and configure an agent with all tools registered
async fn create_agent(config: Config, bus: Arc<MessageBus>) -> Result<Arc<AgentLoop>> {
    // Create session manager
    let session_manager = SessionManager::new().unwrap_or_else(|_| {
        warn!("Failed to create persistent session manager, using in-memory");
        SessionManager::new_memory()
    });

    // Create agent loop
    let agent = Arc::new(AgentLoop::new(config.clone(), session_manager, bus));

    // Create and start cron service for scheduled tasks.
    let cron_store_path = Config::dir().join("cron").join("jobs.json");
    let cron_service = Arc::new(CronService::new(cron_store_path, agent.bus().clone()));
    cron_service.start().await?;

    // Create runtime from config
    let runtime = match create_runtime(&config.runtime).await {
        Ok(r) => {
            info!("Using {} runtime for shell commands", r.name());
            r
        }
        Err(e) => {
            if config.runtime.allow_fallback_to_native {
                warn!(
                    "Failed to create configured runtime: {}. Falling back to native.",
                    e
                );
                Arc::new(NativeRuntime::new())
            } else {
                return Err(anyhow::anyhow!(
                    "Configured runtime '{:?}' unavailable: {}. \
Enable runtime.allow_fallback_to_native to opt in to native fallback.",
                    config.runtime.runtime_type,
                    e
                ));
            }
        }
    };

    // Register all tools
    agent.register_tool(Box::new(EchoTool)).await;
    agent.register_tool(Box::new(ReadFileTool)).await;
    agent.register_tool(Box::new(WriteFileTool)).await;
    agent.register_tool(Box::new(ListDirTool)).await;
    agent.register_tool(Box::new(EditFileTool)).await;
    agent
        .register_tool(Box::new(ShellTool::with_runtime(runtime)))
        .await;
    agent
        .register_tool(Box::new(CronTool::new(cron_service.clone())))
        .await;
    agent
        .register_tool(Box::new(SpawnTool::new(
            Arc::downgrade(&agent),
            agent.bus().clone(),
        )))
        .await;

    info!("Registered {} tools", agent.tool_count().await);

    // Set up provider
    if let Some(runtime_provider) = resolve_runtime_provider(&config) {
        match runtime_provider.name {
            "anthropic" => {
                let provider = ClaudeProvider::new(&runtime_provider.api_key);
                agent.set_provider(Box::new(provider)).await;
            }
            "openai" => {
                let provider = if let Some(base_url) = runtime_provider.api_base.as_deref() {
                    OpenAIProvider::with_base_url(&runtime_provider.api_key, base_url)
                } else {
                    OpenAIProvider::new(&runtime_provider.api_key)
                };
                agent.set_provider(Box::new(provider)).await;
            }
            _ => {}
        }
        info!("Configured runtime provider: {}", runtime_provider.name);
    }

    let unsupported = configured_unsupported_provider_names(&config);
    if !unsupported.is_empty() {
        warn!(
            "Configured provider(s) not yet supported by runtime: {}",
            unsupported.join(", ")
        );
    }

    Ok(agent)
}

/// Interactive or single-message agent mode
async fn cmd_agent(message: Option<String>) -> Result<()> {
    // Load configuration
    let config = Config::load().with_context(|| "Failed to load configuration")?;

    // Create message bus
    let bus = Arc::new(MessageBus::new());

    // Create agent
    let agent = create_agent(config.clone(), bus.clone()).await?;

    // Check whether the runtime can use at least one configured provider.
    if resolve_runtime_provider(&config).is_none() {
        let configured = configured_provider_names(&config);
        if configured.is_empty() {
            eprintln!(
                "Warning: No AI provider configured. Set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY"
            );
            eprintln!("or add your API key to {:?}", Config::path());
        } else {
            eprintln!(
                "Warning: Configured provider(s) are not supported by this runtime: {}",
                configured.join(", ")
            );
            eprintln!(
                "Currently supported runtime providers: {}",
                RUNTIME_SUPPORTED_PROVIDERS.join(", ")
            );
        }
        eprintln!();
    }

    if let Some(msg) = message {
        // Single message mode
        let inbound = InboundMessage::new("cli", "user", "cli", &msg);
        match agent.process_message(&inbound).await {
            Ok(response) => {
                println!("{}", response);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Interactive mode
        println!("ZeptoClaw Interactive Agent");
        println!("Type your message and press Enter. Type 'quit' or 'exit' to stop.");
        println!();

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        loop {
            print!("> ");
            stdout.flush()?;

            let mut input = String::new();
            match stdin.lock().read_line(&mut input) {
                Ok(0) => {
                    // EOF
                    println!();
                    break;
                }
                Ok(_) => {
                    let input = input.trim();
                    if input.is_empty() {
                        continue;
                    }
                    if input == "quit" || input == "exit" {
                        println!("Goodbye!");
                        break;
                    }

                    // Process message
                    let inbound = InboundMessage::new("cli", "user", "cli", input);
                    match agent.process_message(&inbound).await {
                        Ok(response) => {
                            println!();
                            println!("{}", response);
                            println!();
                        }
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            eprintln!();
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error reading input: {}", e);
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Start multi-channel gateway
async fn cmd_gateway() -> Result<()> {
    println!("Starting ZeptoClaw Gateway...");

    // Load configuration
    let config = Config::load().with_context(|| "Failed to load configuration")?;

    // Validate provider before starting services.
    let runtime_provider_name = resolve_runtime_provider(&config).map(|provider| provider.name);
    if runtime_provider_name.is_none() {
        let configured = configured_provider_names(&config);
        if configured.is_empty() {
            error!("No AI provider configured. Set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY");
            error!("or add your API key to {:?}", Config::path());
        } else {
            error!(
                "Configured provider(s) are not supported by this runtime: {}",
                configured.join(", ")
            );
            error!(
                "Currently supported runtime providers: {}",
                RUNTIME_SUPPORTED_PROVIDERS.join(", ")
            );
        }
        std::process::exit(1);
    }

    // Create message bus
    let bus = Arc::new(MessageBus::new());

    // Create agent
    let agent = create_agent(config.clone(), bus.clone()).await?;

    // Create channel manager
    let channel_manager = ChannelManager::new(bus.clone(), config.clone());

    // Register channels via factory.
    let channel_count = register_configured_channels(&channel_manager, bus.clone(), &config).await;
    if channel_count == 0 {
        warn!(
            "No channels configured. Enable channels in {:?}",
            Config::path()
        );
        warn!("The agent loop will still run but won't receive messages from external sources.");
    } else {
        info!("Registered {} channel(s)", channel_count);
    }

    // Start all channels
    channel_manager
        .start_all()
        .await
        .with_context(|| "Failed to start channels")?;

    // Start agent loop in background
    let agent_clone = Arc::clone(&agent);
    let agent_handle = tokio::spawn(async move {
        if let Err(e) = agent_clone.start().await {
            error!("Agent loop error: {}", e);
        }
    });

    println!();
    println!("Gateway is running. Press Ctrl+C to stop.");
    println!();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .with_context(|| "Failed to listen for Ctrl+C")?;

    println!();
    println!("Shutting down...");

    // Stop agent
    agent.stop();

    // Stop all channels
    channel_manager
        .stop_all()
        .await
        .with_context(|| "Failed to stop channels")?;

    // Wait for agent to stop
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), agent_handle).await;

    println!("Gateway stopped.");
    Ok(())
}

/// Manage authentication
async fn cmd_auth(action: AuthAction) -> Result<()> {
    match action {
        AuthAction::Login => {
            println!("Authentication login is not yet implemented.");
            println!();
            println!("To configure API keys, either:");
            println!("  1. Set environment variables:");
            println!("     export ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...");
            println!();
            println!("  2. Edit your config file:");
            println!("     {:?}", Config::path());
        }
        AuthAction::Logout => {
            println!("Authentication logout is not yet implemented.");
        }
        AuthAction::Status => {
            cmd_auth_status().await?;
        }
    }
    Ok(())
}

/// Show authentication status
async fn cmd_auth_status() -> Result<()> {
    let config = Config::load().with_context(|| "Failed to load configuration")?;

    println!("Authentication Status");
    println!("=====================");
    println!();

    // Anthropic/Claude
    let anthropic_status = config
        .providers
        .anthropic
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set");
    println!("  Anthropic (Claude): {}", anthropic_status);

    // OpenAI
    let openai_status = config
        .providers
        .openai
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set");
    println!("  OpenAI:             {}", openai_status);

    // OpenRouter
    let openrouter_status = config
        .providers
        .openrouter
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set");
    println!("  OpenRouter:         {}", openrouter_status);

    // Groq
    let groq_status = config
        .providers
        .groq
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set");
    println!("  Groq:               {}", groq_status);

    // Gemini
    let gemini_status = config
        .providers
        .gemini
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set");
    println!("  Gemini:             {}", gemini_status);

    // Zhipu
    let zhipu_status = config
        .providers
        .zhipu
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set");
    println!("  Zhipu:              {}", zhipu_status);

    println!();
    println!("Runtime Provider Support");
    println!("------------------------");
    println!("  Supported: {}", RUNTIME_SUPPORTED_PROVIDERS.join(", "));
    let unsupported = configured_unsupported_provider_names(&config);
    if !unsupported.is_empty() {
        println!("  Configured but unsupported: {}", unsupported.join(", "));
    }

    println!();

    // Channel tokens
    println!("Channel Tokens");
    println!("--------------");

    // Telegram
    let telegram_status = config
        .channels
        .telegram
        .as_ref()
        .map(|c| {
            if c.token.is_empty() {
                "not set"
            } else if c.enabled {
                "configured (enabled)"
            } else {
                "configured (disabled)"
            }
        })
        .unwrap_or("not set");
    println!("  Telegram: {}", telegram_status);

    // Discord
    let discord_status = config
        .channels
        .discord
        .as_ref()
        .map(|c| {
            if c.token.is_empty() {
                "not set"
            } else if c.enabled {
                "configured (enabled)"
            } else {
                "configured (disabled)"
            }
        })
        .unwrap_or("not set");
    println!("  Discord:  {}", discord_status);

    // Slack
    let slack_status = config
        .channels
        .slack
        .as_ref()
        .map(|c| {
            if c.bot_token.is_empty() {
                "not set"
            } else if c.enabled {
                "configured (enabled)"
            } else {
                "configured (disabled)"
            }
        })
        .unwrap_or("not set");
    println!("  Slack:    {}", slack_status);

    println!();

    Ok(())
}

/// Show system status
async fn cmd_status() -> Result<()> {
    let config = Config::load().unwrap_or_default();

    println!("ZeptoClaw Status");
    println!("================");
    println!();

    // Version
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!();

    // Configuration
    println!("Configuration");
    println!("-------------");
    println!("  Config directory: {:?}", Config::dir());
    println!("  Config file:      {:?}", Config::path());
    println!("  Config exists:    {}", Config::path().exists());
    println!();

    // Workspace
    println!("Workspace");
    println!("---------");
    let workspace_path = config.workspace_path();
    println!("  Path:   {:?}", workspace_path);
    println!("  Exists: {}", workspace_path.exists());
    println!();

    // Sessions
    println!("Sessions");
    println!("--------");
    let sessions_path = Config::dir().join("sessions");
    println!("  Path:   {:?}", sessions_path);
    println!("  Exists: {}", sessions_path.exists());
    if sessions_path.exists() {
        let session_count = std::fs::read_dir(&sessions_path)
            .map(|entries| entries.filter_map(|e| e.ok()).count())
            .unwrap_or(0);
        println!("  Count:  {}", session_count);
    }
    println!();

    // Agent defaults
    println!("Agent Defaults");
    println!("--------------");
    println!("  Model:              {}", config.agents.defaults.model);
    println!(
        "  Max tokens:         {}",
        config.agents.defaults.max_tokens
    );
    println!(
        "  Temperature:        {}",
        config.agents.defaults.temperature
    );
    println!(
        "  Max tool iterations: {}",
        config.agents.defaults.max_tool_iterations
    );
    println!();

    // Gateway
    println!("Gateway");
    println!("-------");
    println!("  Host: {}", config.gateway.host);
    println!("  Port: {}", config.gateway.port);
    println!();

    // Runtime info
    println!("Runtime");
    println!("-------");
    println!("  Type: {:?}", config.runtime.runtime_type);
    println!(
        "  Native fallback: {}",
        if config.runtime.allow_fallback_to_native {
            "enabled (opt-in)"
        } else {
            "disabled (fail-closed)"
        }
    );
    let available = available_runtimes().await;
    println!("  Available: {}", available.join(", "));
    println!();

    // Provider status
    let runtime_provider_name = resolve_runtime_provider(&config).map(|provider| provider.name);
    println!(
        "Runtime provider: {}",
        runtime_provider_name.unwrap_or("not configured")
    );
    let unsupported = configured_unsupported_provider_names(&config);
    if !unsupported.is_empty() {
        println!("Configured but unsupported: {}", unsupported.join(", "));
    }
    println!(
        "Runtime supports: {}",
        RUNTIME_SUPPORTED_PROVIDERS.join(", ")
    );
    println!();

    // Registered tools (static list)
    println!("Available Tools");
    println!("---------------");
    println!("  - echo");
    println!("  - read_file");
    println!("  - write_file");
    println!("  - list_dir");
    println!("  - edit_file");
    println!("  - shell");
    println!("  - cron");
    println!("  - spawn");
    println!();

    Ok(())
}
