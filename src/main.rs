//! ZeptoClaw CLI - Ultra-lightweight personal AI assistant
//!
//! This is the main entry point for the ZeptoClaw command-line interface.
//! It provides commands for running the AI agent in interactive mode,
//! starting the multi-channel gateway, and managing configuration.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use zeptoclaw::agent::{AgentLoop, ContextBuilder};
use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::channels::{register_configured_channels, ChannelManager};
use zeptoclaw::config::{
    Config, ContainerAgentBackend, MemoryBackend, MemoryCitationsMode, RuntimeType,
};
use zeptoclaw::cron::CronService;
use zeptoclaw::health::{
    health_port, start_health_server, start_periodic_usage_flush, UsageMetrics,
};
use zeptoclaw::heartbeat::{ensure_heartbeat_file, HeartbeatService, HEARTBEAT_PROMPT};
use zeptoclaw::providers::{
    configured_provider_names, configured_unsupported_provider_names, resolve_runtime_provider,
    ClaudeProvider, OpenAIProvider, RUNTIME_SUPPORTED_PROVIDERS,
};
use zeptoclaw::runtime::{available_runtimes, create_runtime, NativeRuntime};
use zeptoclaw::session::SessionManager;
use zeptoclaw::skills::SkillsLoader;
use zeptoclaw::tools::cron::CronTool;
use zeptoclaw::tools::delegate::DelegateTool;
use zeptoclaw::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
use zeptoclaw::tools::shell::ShellTool;
use zeptoclaw::tools::spawn::SpawnTool;
use zeptoclaw::tools::{
    EchoTool, GoogleSheetsTool, MemoryGetTool, MemorySearchTool, MessageTool, R8rTool,
    WebFetchTool, WebSearchTool, WhatsAppTool,
};

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
        /// Stream the response token-by-token
        #[arg(long)]
        stream: bool,
    },
    /// Start multi-channel gateway
    Gateway {
        /// Run in container isolation [optional: docker, apple]
        #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "BACKEND")]
        containerized: Option<String>,
    },
    /// Run agent in stdin/stdout mode (for containerized execution)
    AgentStdin,
    /// Trigger or inspect heartbeat tasks
    Heartbeat {
        /// Show heartbeat file contents
        #[arg(short, long)]
        show: bool,
        /// Edit heartbeat file in $EDITOR
        #[arg(short, long)]
        edit: bool,
    },
    /// Manage skills
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },
    /// Manage authentication
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Show version information
    Version,
    /// Show system status
    Status,
    /// Validate configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum SkillsAction {
    /// List skills (ready-only by default)
    List {
        /// Include unavailable skills
        #[arg(short, long)]
        all: bool,
    },
    /// Show full skill content
    Show {
        /// Skill name
        name: String,
    },
    /// Create a new workspace skill template
    Create {
        /// Skill name
        name: String,
    },
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

#[derive(Subcommand)]
enum ConfigAction {
    /// Check configuration for errors and warnings
    Check,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging (JSON format when RUST_LOG_FORMAT=json)
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let use_json = std::env::var("RUST_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    if use_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .with_target(true)
            .with_thread_ids(false)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Version) | None => {
            cmd_version();
        }
        Some(Commands::Onboard) => {
            cmd_onboard().await?;
        }
        Some(Commands::Agent { message, stream }) => {
            cmd_agent(message, stream).await?;
        }
        Some(Commands::Gateway { containerized }) => {
            cmd_gateway(containerized).await?;
        }
        Some(Commands::AgentStdin) => {
            cmd_agent_stdin().await?;
        }
        Some(Commands::Heartbeat { show, edit }) => {
            cmd_heartbeat(show, edit).await?;
        }
        Some(Commands::Skills { action }) => {
            cmd_skills(action).await?;
        }
        Some(Commands::Auth { action }) => {
            cmd_auth(action).await?;
        }
        Some(Commands::Status) => {
            cmd_status().await?;
        }
        Some(Commands::Config { action }) => {
            cmd_config(action).await?;
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

async fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Check => {
            let config_path = zeptoclaw::config::Config::path();
            println!("Config file: {}", config_path.display());

            if !config_path.exists() {
                println!("[OK] No config file found (using defaults)");
                return Ok(());
            }

            let content =
                std::fs::read_to_string(&config_path).context("Failed to read config file")?;

            let raw: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    println!("[ERROR] Invalid JSON: {}", e);
                    return Ok(());
                }
            };

            let diagnostics = zeptoclaw::config::validate::validate_config(&raw);
            for diag in &diagnostics {
                println!("{}", diag);
            }

            let errors = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Error)
                .count();
            let warnings = diagnostics
                .iter()
                .filter(|d| d.level == zeptoclaw::config::validate::DiagnosticLevel::Warn)
                .count();

            if errors == 0 && warnings == 0 {
                println!("\nConfiguration looks good!");
            } else {
                println!("\nFound {} error(s), {} warning(s)", errors, warnings);
            }
        }
    }
    Ok(())
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

/// Read a password/API key from stdin (hidden input)
fn read_secret() -> Result<String> {
    rpassword::read_password_from_bufread(&mut std::io::stdin().lock())
        .with_context(|| "Failed to read secret input")
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

fn memory_backend_label(backend: &MemoryBackend) -> &'static str {
    match backend {
        MemoryBackend::Disabled => "none",
        MemoryBackend::Builtin => "builtin",
        MemoryBackend::Qmd => "qmd",
    }
}

fn memory_citations_label(mode: &MemoryCitationsMode) -> &'static str {
    match mode {
        MemoryCitationsMode::Auto => "auto",
        MemoryCitationsMode::On => "on",
        MemoryCitationsMode::Off => "off",
    }
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

    let skills_prompt = build_skills_prompt(&config);
    let context_builder = if skills_prompt.is_empty() {
        ContextBuilder::new()
    } else {
        ContextBuilder::new().with_skills(&skills_prompt)
    };

    // Create agent loop
    let agent = Arc::new(AgentLoop::with_context_builder(
        config.clone(),
        session_manager,
        bus,
        context_builder,
    ));

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

    // Register web tools.
    if let Some(web_search_key) = config.tools.web.search.api_key.as_deref() {
        let web_search_key = web_search_key.trim();
        if !web_search_key.is_empty() {
            agent
                .register_tool(Box::new(WebSearchTool::with_max_results(
                    web_search_key,
                    config.tools.web.search.max_results as usize,
                )))
                .await;
            info!("Registered web_search tool");
        }
    }
    agent.register_tool(Box::new(WebFetchTool::new())).await;
    info!("Registered web_fetch tool");

    // Register proactive messaging tool.
    agent
        .register_tool(Box::new(MessageTool::new(agent.bus().clone())))
        .await;
    info!("Registered message tool");

    // Register WhatsApp tool.
    if let (Some(phone_number_id), Some(access_token)) = (
        config.tools.whatsapp.phone_number_id.as_deref(),
        config.tools.whatsapp.access_token.as_deref(),
    ) {
        if !phone_number_id.trim().is_empty() && !access_token.trim().is_empty() {
            agent
                .register_tool(Box::new(WhatsAppTool::with_default_language(
                    phone_number_id.trim(),
                    access_token.trim(),
                    config.tools.whatsapp.default_language.trim(),
                )))
                .await;
            info!("Registered whatsapp_send tool");
        }
    }

    // Register Google Sheets tool.
    if let Some(access_token) = config.tools.google_sheets.access_token.as_deref() {
        let token = access_token.trim();
        if !token.is_empty() {
            agent
                .register_tool(Box::new(GoogleSheetsTool::new(token)))
                .await;
            info!("Registered google_sheets tool");
        }
    } else if let Some(encoded) = config.tools.google_sheets.service_account_base64.as_deref() {
        match GoogleSheetsTool::from_service_account(encoded.trim()) {
            Ok(tool) => {
                agent.register_tool(Box::new(tool)).await;
                info!("Registered google_sheets tool from base64 payload");
            }
            Err(e) => warn!("Failed to initialize google_sheets tool: {}", e),
        }
    }

    match &config.memory.backend {
        MemoryBackend::Disabled => {
            info!("Memory tools are disabled");
        }
        MemoryBackend::Builtin => {
            agent
                .register_tool(Box::new(MemorySearchTool::new(config.memory.clone())))
                .await;
            agent
                .register_tool(Box::new(MemoryGetTool::new(config.memory.clone())))
                .await;
            info!("Registered memory_search and memory_get tools");
        }
        MemoryBackend::Qmd => {
            warn!("Memory backend 'qmd' is not implemented yet; using built-in memory tools");
            agent
                .register_tool(Box::new(MemorySearchTool::new(config.memory.clone())))
                .await;
            agent
                .register_tool(Box::new(MemoryGetTool::new(config.memory.clone())))
                .await;
            info!("Registered memory_search and memory_get tools");
        }
    }

    agent
        .register_tool(Box::new(CronTool::new(cron_service.clone())))
        .await;
    agent
        .register_tool(Box::new(SpawnTool::new(
            Arc::downgrade(&agent),
            agent.bus().clone(),
        )))
        .await;
    agent.register_tool(Box::new(R8rTool::default())).await;

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

    // Register DelegateTool for agent swarm delegation (requires provider)
    if config.swarm.enabled {
        if let Some(provider) = agent.provider().await {
            agent
                .register_tool(Box::new(DelegateTool::new(
                    config.clone(),
                    provider,
                    agent.bus().clone(),
                )))
                .await;
            info!("Registered delegate tool (swarm)");
        } else {
            warn!("Swarm enabled but no provider configured — delegate tool not registered");
        }
    }

    Ok(agent)
}

fn skills_loader_from_config(config: &Config) -> SkillsLoader {
    let workspace_dir = config
        .skills
        .workspace_dir
        .as_deref()
        .map(expand_tilde)
        .unwrap_or_else(|| Config::dir().join("skills"));
    SkillsLoader::new(workspace_dir, None)
}

fn build_skills_prompt(config: &Config) -> String {
    if !config.skills.enabled {
        return String::new();
    }

    let loader = skills_loader_from_config(config);
    let disabled: std::collections::HashSet<String> = config
        .skills
        .disabled
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();

    let visible_skills = loader
        .list_skills(false)
        .into_iter()
        .filter(|info| !disabled.contains(&info.name.to_ascii_lowercase()))
        .collect::<Vec<_>>();

    if visible_skills.is_empty() {
        return String::new();
    }

    let mut summary_lines = vec!["<skills>".to_string()];
    for info in &visible_skills {
        if let Some(skill) = loader.load_skill(&info.name) {
            let available = loader.check_requirements(&skill);
            summary_lines.push(format!("  <skill available=\"{}\">", available));
            summary_lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
            summary_lines.push(format!(
                "    <description>{}</description>",
                escape_xml(&skill.description)
            ));
            summary_lines.push(format!(
                "    <location>{}</location>",
                escape_xml(&skill.path)
            ));
            summary_lines.push("  </skill>".to_string());
        }
    }
    summary_lines.push("</skills>".to_string());

    let mut always_names = loader.get_always_skills();
    always_names.extend(config.skills.always_load.iter().cloned());
    always_names.sort();
    always_names.dedup();
    always_names.retain(|name| !disabled.contains(&name.to_ascii_lowercase()));
    always_names.retain(|name| loader.load_skill(name).is_some());

    let always_content = if always_names.is_empty() {
        String::new()
    } else {
        loader.load_skills_for_context(&always_names)
    };

    if always_content.is_empty() {
        summary_lines.join("\n")
    } else {
        format!(
            "{}\n\n## Active Skills\n\n{}",
            summary_lines.join("\n"),
            always_content
        )
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Interactive or single-message agent mode
async fn cmd_agent(message: Option<String>, stream: bool) -> Result<()> {
    // Load configuration
    let config = Config::load().with_context(|| "Failed to load configuration")?;

    // Create message bus
    let bus = Arc::new(MessageBus::new());

    // Create agent
    let agent = create_agent(config.clone(), bus.clone()).await?;

    // Streaming flag placeholder (used in Task 5 for actual streaming)
    let _streaming = stream || config.agents.defaults.streaming;

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

/// Run agent in stdin/stdout mode for containerized execution.
///
/// Reads a JSON `AgentRequest` from stdin, processes it through the agent,
/// and writes a marked `AgentResponse` to stdout.
async fn cmd_agent_stdin() -> Result<()> {
    let mut config = Config::load().with_context(|| "Failed to load configuration")?;

    // Read JSON request from stdin
    let stdin = io::stdin();
    let mut input = String::new();
    stdin
        .lock()
        .read_line(&mut input)
        .with_context(|| "Failed to read from stdin")?;

    let request: zeptoclaw::gateway::AgentRequest =
        serde_json::from_str(&input).map_err(|e| anyhow::anyhow!("Invalid request JSON: {}", e))?;

    if let Err(e) = request.validate() {
        let response = zeptoclaw::gateway::AgentResponse::error(
            &request.request_id,
            &e.to_string(),
            "INVALID_REQUEST",
        );
        println!("{}", response.to_marked_json());
        io::stdout().flush()?;
        return Ok(());
    }

    let zeptoclaw::gateway::AgentRequest {
        request_id,
        message,
        agent_config,
        session,
    } = request;

    // Apply request-scoped agent defaults.
    config.agents.defaults = agent_config;

    // Create agent with merged config
    let bus = Arc::new(MessageBus::new());
    let agent = create_agent(config, bus.clone()).await?;

    // Seed provided session state before processing.
    if let Some(ref seed_session) = session {
        agent.session_manager().save(seed_session).await?;
    }

    // Process the message
    let response = match agent.process_message(&message).await {
        Ok(content) => {
            let updated_session = agent.session_manager().get(&message.session_key).await?;
            zeptoclaw::gateway::AgentResponse::success(&request_id, &content, updated_session)
        }
        Err(e) => {
            zeptoclaw::gateway::AgentResponse::error(&request_id, &e.to_string(), "PROCESS_ERROR")
        }
    };

    // Write response with markers to stdout
    println!("{}", response.to_marked_json());
    io::stdout().flush()?;

    Ok(())
}

/// Start multi-channel gateway
async fn cmd_gateway(containerized_flag: Option<String>) -> Result<()> {
    println!("Starting ZeptoClaw Gateway...");

    // Load configuration
    let mut config = Config::load().with_context(|| "Failed to load configuration")?;

    // --containerized [docker|apple] overrides config backend
    let containerized = containerized_flag.is_some();
    if let Some(ref b) = containerized_flag {
        if b != "auto" {
            config.container_agent.backend = match b.to_lowercase().as_str() {
                "docker" => ContainerAgentBackend::Docker,
                #[cfg(target_os = "macos")]
                "apple" => ContainerAgentBackend::Apple,
                "auto" => ContainerAgentBackend::Auto,
                other => {
                    #[cfg(target_os = "macos")]
                    return Err(anyhow::anyhow!(
                        "Unknown backend '{}'. Use: docker or apple",
                        other
                    ));
                    #[cfg(not(target_os = "macos"))]
                    return Err(anyhow::anyhow!("Unknown backend '{}'. Use: docker", other));
                }
            };
        }
    }

    // Create message bus
    let bus = Arc::new(MessageBus::new());

    // Create usage metrics tracker
    let metrics = Arc::new(UsageMetrics::new());

    // Start health check server (liveness + readiness)
    let hp = health_port();
    let health_handle = match start_health_server(hp, Arc::clone(&metrics)).await {
        Ok(handle) => {
            info!(
                port = hp,
                "Health endpoints available at /healthz and /readyz"
            );
            Some(handle)
        }
        Err(e) => {
            warn!(error = %e, "Failed to start health server (non-fatal)");
            None
        }
    };

    // Create shutdown watch channel for periodic usage flush
    let (usage_shutdown_tx, usage_shutdown_rx) = tokio::sync::watch::channel(false);
    let usage_flush_handle = start_periodic_usage_flush(Arc::clone(&metrics), usage_shutdown_rx);

    // Determine agent backend: containerized or in-process
    let mut proxy = None;
    let proxy_handle = if containerized {
        info!("Starting gateway with containerized agent mode");

        // Resolve backend (auto-detect or explicit from config)
        let backend = zeptoclaw::gateway::resolve_backend(&config.container_agent)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        info!("Resolved container backend: {}", backend);

        // Validate the resolved backend
        match backend {
            zeptoclaw::gateway::ResolvedBackend::Docker => {
                validate_docker_available(configured_docker_binary(&config.container_agent))
                    .await?;
            }
            #[cfg(target_os = "macos")]
            zeptoclaw::gateway::ResolvedBackend::Apple => {
                validate_apple_available().await?;
            }
        }

        // Check image exists (Docker-specific)
        let image = &config.container_agent.image;
        if backend == zeptoclaw::gateway::ResolvedBackend::Docker {
            let docker_binary = configured_docker_binary(&config.container_agent);
            let image_check = tokio::process::Command::new(docker_binary)
                .args(["image", "inspect", image])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            if !image_check.map(|s| s.success()).unwrap_or(false) {
                eprintln!(
                    "Warning: Docker image '{}' not found (checked via '{}').",
                    image, docker_binary
                );
                eprintln!("Build it with: {} build -t {} .", docker_binary, image);
                return Err(anyhow::anyhow!(
                    "Docker image '{}' not found (checked via '{}')",
                    image,
                    docker_binary
                ));
            }
        }

        info!("Using container image: {} (backend={})", image, backend);

        let proxy_instance = Arc::new(zeptoclaw::gateway::ContainerAgentProxy::new(
            config.clone(),
            bus.clone(),
            backend,
        ));
        proxy_instance.set_usage_metrics(Arc::clone(&metrics));
        let proxy_for_task = Arc::clone(&proxy_instance);
        let proxy_metrics = Arc::clone(&metrics);
        proxy = Some(proxy_instance);

        Some(tokio::spawn(async move {
            if let Err(e) = proxy_for_task.start().await {
                error!("Container agent proxy error: {}", e);
            }
            proxy_metrics.set_ready(false);
            warn!("Container agent proxy stopped; readiness set to false");
        }))
    } else {
        // Validate provider for in-process mode
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
        None
    };

    // Create in-process agent (only needed when not containerized)
    let agent = if !containerized {
        let agent = create_agent(config.clone(), bus.clone()).await?;
        agent.set_usage_metrics(Arc::clone(&metrics)).await;
        Some(agent)
    } else {
        None
    };

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

    let heartbeat_service = if config.heartbeat.enabled {
        let heartbeat_path = heartbeat_file_path(&config);
        match ensure_heartbeat_file(&heartbeat_path).await {
            Ok(true) => info!("Created heartbeat file template at {:?}", heartbeat_path),
            Ok(false) => {}
            Err(e) => warn!(
                "Failed to initialize heartbeat file {:?}: {}",
                heartbeat_path, e
            ),
        }

        let service = Arc::new(HeartbeatService::new(
            heartbeat_path,
            config.heartbeat.interval_secs,
            bus.clone(),
            "heartbeat:system",
        ));
        service.start().await?;
        Some(service)
    } else {
        None
    };

    // Start agent loop in background (only for in-process mode)
    let agent_handle = if let Some(ref agent) = agent {
        let agent_clone = Arc::clone(agent);
        let agent_metrics = Arc::clone(&metrics);
        Some(tokio::spawn(async move {
            if let Err(e) = agent_clone.start().await {
                error!("Agent loop error: {}", e);
            }
            agent_metrics.set_ready(false);
            warn!("Agent loop stopped; readiness set to false");
        }))
    } else {
        None
    };

    // Mark gateway as ready for /readyz
    metrics.set_ready(true);

    println!();
    if containerized {
        println!("Gateway is running (containerized mode). Press Ctrl+C to stop.");
    } else {
        println!("Gateway is running. Press Ctrl+C to stop.");
    }
    println!();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .with_context(|| "Failed to listen for Ctrl+C")?;

    println!();
    println!("Shutting down...");

    // Mark not ready immediately
    metrics.set_ready(false);

    // Signal usage flush to emit final summary
    let _ = usage_shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), usage_flush_handle).await;

    if let Some(service) = &heartbeat_service {
        service.stop().await;
    }

    // Stop agent or proxy
    if let Some(ref agent) = agent {
        agent.stop();
    }
    if let Some(ref proxy) = proxy {
        proxy.stop();
    }

    // Stop all channels
    channel_manager
        .stop_all()
        .await
        .with_context(|| "Failed to stop channels")?;

    // Wait for agent/proxy to stop
    if let Some(handle) = agent_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }
    if let Some(handle) = proxy_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    // Stop health server
    if let Some(handle) = health_handle {
        handle.abort();
    }

    println!("Gateway stopped.");
    Ok(())
}

fn heartbeat_file_path(config: &Config) -> PathBuf {
    config
        .heartbeat
        .file_path
        .as_deref()
        .map(expand_tilde)
        .unwrap_or_else(|| Config::dir().join("HEARTBEAT.md"))
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

/// Heartbeat utility command.
async fn cmd_heartbeat(show: bool, edit: bool) -> Result<()> {
    let config = Config::load().with_context(|| "Failed to load configuration")?;
    let heartbeat_path = heartbeat_file_path(&config);

    if ensure_heartbeat_file(&heartbeat_path).await? {
        println!("Created heartbeat file at {:?}", heartbeat_path);
    }

    if show {
        let content = tokio::fs::read_to_string(&heartbeat_path).await?;
        println!("{}", content);
        return Ok(());
    }

    if edit {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
        let status = std::process::Command::new(editor)
            .arg(&heartbeat_path)
            .status()
            .with_context(|| "Failed to launch editor")?;
        if !status.success() {
            eprintln!("Editor exited with status: {}", status);
        }
        return Ok(());
    }

    let content = tokio::fs::read_to_string(&heartbeat_path)
        .await
        .unwrap_or_default();
    if HeartbeatService::is_empty(&content) {
        println!("Heartbeat file has no actionable tasks.");
        return Ok(());
    }

    let bus = Arc::new(MessageBus::new());
    let agent = create_agent(config, bus).await?;
    let inbound = InboundMessage::new("cli", "heartbeat", "heartbeat:cli", HEARTBEAT_PROMPT);
    let response = agent.process_message(&inbound).await?;
    println!("{}", response);
    Ok(())
}

/// Skills management command.
async fn cmd_skills(action: SkillsAction) -> Result<()> {
    let config = Config::load().with_context(|| "Failed to load configuration")?;
    let loader = skills_loader_from_config(&config);

    match action {
        SkillsAction::List { all } => {
            let disabled: std::collections::HashSet<String> = config
                .skills
                .disabled
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect();
            let mut listed = loader.list_skills(!all);
            listed.retain(|info| !disabled.contains(&info.name.to_ascii_lowercase()));

            if listed.is_empty() {
                println!("No skills found.");
                return Ok(());
            }

            println!("Skills:");
            for info in listed {
                let ready = loader
                    .load_skill(&info.name)
                    .map(|skill| loader.check_requirements(&skill))
                    .unwrap_or(false);
                let marker = if ready {
                    "ready"
                } else {
                    "missing requirements"
                };
                println!("  - {} ({}, {})", info.name, info.source, marker);
            }
        }
        SkillsAction::Show { name } => {
            if let Some(skill) = loader.load_skill(&name) {
                println!("Name: {}", skill.name);
                println!("Description: {}", skill.description);
                println!("Source: {}", skill.source);
                println!("Path: {}", skill.path);
                println!();
                println!("{}", skill.content);
            } else {
                eprintln!("Skill '{}' not found", name);
            }
        }
        SkillsAction::Create { name } => {
            let dir = loader.workspace_dir().join(&name);
            let skill_file = dir.join("SKILL.md");
            if skill_file.exists() {
                eprintln!("Skill '{}' already exists at {:?}", name, skill_file);
                return Ok(());
            }

            std::fs::create_dir_all(&dir)?;
            let template = format!(
                r#"---
name: {name}
description: Describe what this skill does.
metadata: {{"zeptoclaw":{{"emoji":"📚","requires":{{}}}}}}
---

# {name} Skill

Describe usage and concrete command examples.
"#
            );
            std::fs::write(&skill_file, template)?;
            println!("Created skill at {:?}", skill_file);
        }
    }

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

/// Validate that Docker is available, returning a user-friendly error if not.
async fn validate_docker_available(docker_binary: &str) -> Result<()> {
    if !zeptoclaw::gateway::is_docker_available_with_binary(docker_binary).await {
        return Err(anyhow::anyhow!(
            "Docker is not available via '{}'. Install Docker or run without --containerized.",
            docker_binary
        ));
    }
    Ok(())
}

fn configured_docker_binary(config: &zeptoclaw::config::ContainerAgentConfig) -> &str {
    config
        .docker_binary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("docker")
}

/// Validate that Apple Container is available (macOS only).
#[cfg(target_os = "macos")]
async fn validate_apple_available() -> Result<()> {
    if !zeptoclaw::gateway::is_apple_container_available().await {
        return Err(anyhow::anyhow!(
            "Apple Container is not available. Requires macOS 15+ with `container` CLI installed."
        ));
    }
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

    // Container Agent
    println!("Container Agent");
    println!("---------------");
    let backend_label = match config.container_agent.backend {
        ContainerAgentBackend::Auto => "auto",
        ContainerAgentBackend::Docker => "docker",
        #[cfg(target_os = "macos")]
        ContainerAgentBackend::Apple => "apple",
    };
    println!("  Backend: {}", backend_label);
    println!("  Image: {}", config.container_agent.image);
    if let Some(binary) = config.container_agent.docker_binary.as_deref() {
        if !binary.trim().is_empty() {
            println!("  Docker binary override: {}", binary);
        }
    }
    println!("  Timeout: {}s", config.container_agent.timeout_secs);
    println!(
        "  Max concurrent: {}",
        config.container_agent.max_concurrent
    );
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

    // Memory
    println!("Memory");
    println!("------");
    println!(
        "  Backend: {}",
        memory_backend_label(&config.memory.backend)
    );
    println!(
        "  Citations: {}",
        memory_citations_label(&config.memory.citations)
    );
    println!(
        "  Include default files: {}",
        if config.memory.include_default_memory {
            "yes"
        } else {
            "no"
        }
    );
    println!("  Max results: {}", config.memory.max_results);
    println!("  Min score: {}", config.memory.min_score);
    println!();

    // Heartbeat
    println!("Heartbeat");
    println!("---------");
    println!(
        "  Enabled: {}",
        if config.heartbeat.enabled {
            "yes"
        } else {
            "no"
        }
    );
    println!("  Interval: {}s", config.heartbeat.interval_secs);
    println!("  File: {:?}", heartbeat_file_path(&config));
    println!();

    // Skills
    println!("Skills");
    println!("------");
    println!(
        "  Enabled: {}",
        if config.skills.enabled { "yes" } else { "no" }
    );
    println!(
        "  Workspace dir: {:?}",
        skills_loader_from_config(&config).workspace_dir()
    );
    if !config.skills.always_load.is_empty() {
        println!("  Always load: {}", config.skills.always_load.join(", "));
    }
    if !config.skills.disabled.is_empty() {
        println!("  Disabled: {}", config.skills.disabled.join(", "));
    }
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
    if config
        .tools
        .web
        .search
        .api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
    {
        println!("  - web_search");
    } else {
        println!("  - web_search (disabled: set tools.web.search.api_key or BRAVE_API_KEY)");
    }
    println!("  - web_fetch");
    println!("  - message");
    println!("  - r8r");
    if config
        .tools
        .whatsapp
        .phone_number_id
        .as_deref()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
        && config
            .tools
            .whatsapp
            .access_token
            .as_deref()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    {
        println!("  - whatsapp_send");
    } else {
        println!("  - whatsapp_send (disabled: set tools.whatsapp.phone_number_id/access_token)");
    }

    let has_gsheets = config
        .tools
        .google_sheets
        .access_token
        .as_deref()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
        || config
            .tools
            .google_sheets
            .service_account_base64
            .as_deref()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
    if has_gsheets {
        println!("  - google_sheets");
    } else {
        println!("  - google_sheets (disabled: set tools.google_sheets token config)");
    }

    match &config.memory.backend {
        MemoryBackend::Disabled => {
            println!("  - memory_search (disabled: memory.backend=none)");
            println!("  - memory_get (disabled: memory.backend=none)");
        }
        MemoryBackend::Builtin => {
            println!("  - memory_search");
            println!("  - memory_get");
        }
        MemoryBackend::Qmd => {
            println!("  - memory_search (qmd fallback -> builtin)");
            println!("  - memory_get (qmd fallback -> builtin)");
        }
    }
    println!("  - cron");
    println!("  - spawn");
    println!();

    Ok(())
}
