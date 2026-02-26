//! CLI module — command parsing and dispatch
//!
//! All CLI logic lives here. `main.rs` calls `cli::run()`.

pub mod agent;
pub mod batch;
pub mod channel;
pub mod common;
pub mod config;
pub mod daemon;
pub mod doctor;
pub mod gateway;
pub mod heartbeat;
pub mod history;
pub mod memory;
pub mod migrate;
pub mod onboard;
pub mod pair;
pub mod secrets;
pub mod skills;
pub mod status;
pub mod template;
pub mod tools;
pub mod update;
pub mod watch;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "zeptoclaw")]
#[command(version)]
#[command(about = "Ultra-lightweight personal AI assistant", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration and workspace
    Onboard {
        /// Run full 10-step wizard (express mode by default)
        #[arg(long)]
        full: bool,
    },
    /// Start interactive agent mode
    Agent {
        /// Direct message to process (non-interactive mode)
        #[arg(short, long)]
        message: Option<String>,
        /// Apply an agent template (built-in or ~/.zeptoclaw/templates/*.json)
        #[arg(long)]
        template: Option<String>,
        /// Stream the response token-by-token
        #[arg(long)]
        stream: bool,
        /// Show what tools would be called without executing them
        #[arg(long)]
        dry_run: bool,
        /// Agent mode: observer (read-only), assistant (read/write + approval), autonomous (full access)
        #[arg(long)]
        mode: Option<String>,
    },
    /// Process prompts from a file
    Batch {
        /// Input file (.txt, .json, or .jsonl)
        #[arg(long)]
        input: std::path::PathBuf,
        /// Optional output file (prints to stdout if omitted)
        #[arg(long)]
        output: Option<std::path::PathBuf>,
        /// Output format for results
        #[arg(long, value_enum, default_value_t = BatchFormat::Text)]
        format: BatchFormat,
        /// Stop processing after the first failed prompt
        #[arg(long)]
        stop_on_error: bool,
        /// Stream LLM output internally while collecting final result text
        #[arg(long)]
        stream: bool,
        /// Apply an agent template to all prompts
        #[arg(long)]
        template: Option<String>,
    },
    /// Start multi-channel gateway
    Gateway {
        /// Run in container isolation [optional: docker, apple]
        #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "BACKEND")]
        containerized: Option<String>,
        /// Start a tunnel to expose gateway publicly [cloudflare, ngrok, tailscale, auto]
        #[arg(long, value_name = "PROVIDER")]
        tunnel: Option<String>,
    },
    /// Run agent in stdin/stdout mode (for containerized execution)
    AgentStdin,
    /// Trigger or inspect heartbeat tasks
    Heartbeat {
        /// Show heartbeat file contents
        #[arg(short, long, conflicts_with = "edit")]
        show: bool,
        /// Edit heartbeat file in $EDITOR
        #[arg(short, long, conflicts_with = "show")]
        edit: bool,
    },
    /// Manage conversation history
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
    /// Manage long-term memory
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// Manage agent templates
    Template {
        #[command(subcommand)]
        action: TemplateAction,
    },
    /// Manage skills
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },
    /// Manage and discover tools
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
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
    /// Manage communication channels
    Channel {
        #[command(subcommand)]
        action: ChannelAction,
    },
    /// Validate configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage secret encryption
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },
    /// Watch a URL for changes and notify
    Watch {
        /// URL to monitor
        url: String,
        /// Check interval (e.g., "1h", "30m", "15m")
        #[arg(long, default_value = "1h")]
        interval: String,
        /// Channel to notify on changes (telegram, slack, discord). Omit for stdout only.
        #[arg(long)]
        notify: Option<String>,
    },
    /// Manage device pairing (bearer token auth)
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// Run system diagnostics
    Doctor {
        /// Include online provider connectivity checks
        #[arg(long)]
        online: bool,
    },
    /// Start supervised daemon (auto-restarts gateway on failure)
    Daemon,
    /// Migrate config and skills from an OpenClaw installation
    Migrate {
        /// Path to OpenClaw directory (auto-detected if omitted)
        #[arg(long)]
        from: Option<String>,
        /// Accept all defaults without prompting
        #[arg(long, short)]
        yes: bool,
        /// Preview what would be migrated without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Check for updates or update to latest version
    Update {
        /// Only check, don't download
        #[arg(long)]
        check: bool,
        /// Install specific version (e.g., "v0.5.2")
        #[arg(long)]
        version: Option<String>,
        /// Force re-download even if already on latest
        #[arg(long)]
        force: bool,
    },
    /// Hardware device management (USB discovery, peripherals)
    Hardware {
        #[command(subcommand)]
        action: HardwareAction,
    },
}

#[derive(Subcommand)]
pub enum MemoryAction {
    /// List all stored memories
    List {
        /// Filter by category
        #[arg(long)]
        category: Option<String>,
    },
    /// Search memories by query
    Search {
        /// Search query (matches key, value, category, tags)
        query: String,
    },
    /// Set a memory value
    Set {
        /// Memory key (e.g. "user:name", "preference:language")
        key: String,
        /// Memory value
        value: String,
        /// Category for grouping
        #[arg(long, default_value = "general")]
        category: String,
        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
    },
    /// Delete a memory by key
    Delete {
        /// Memory key to delete
        key: String,
    },
    /// Show memory statistics
    Stats,
    /// Remove expired memories below decay threshold
    Cleanup {
        /// Decay score threshold (0.0-1.0). Entries below this are removed.
        #[arg(long, default_value_t = 0.1)]
        threshold: f32,
    },
    /// Export longterm memory to a JSON snapshot file
    Export {
        /// Output file path (default: ~/.zeptoclaw/memory/snapshot.json)
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Import longterm memory from a JSON snapshot file
    Import {
        /// Path to snapshot file
        path: std::path::PathBuf,
        /// Overwrite existing keys (default: skip existing)
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Subcommand)]
pub enum SkillsAction {
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
    /// Search for skills on ClawHub and GitHub
    Search {
        /// Search query
        query: String,
        /// Source filter: clawhub, github, or all (default)
        #[arg(long, default_value = "all")]
        source: String,
    },
    /// Install a skill by name (from community repo) or --github
    Install {
        /// Skill name (installs from community repo by default)
        name: String,
        /// Install from explicit GitHub repo (owner/repo or owner/repo/skill)
        #[arg(long)]
        github: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ToolsAction {
    /// List all available tools with status
    List,
    /// Show details for a specific tool
    Info {
        /// Tool name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum AuthAction {
    /// Log in to AI provider via OAuth browser sign-in
    Login {
        /// Provider to authenticate with (e.g., "anthropic")
        provider: Option<String>,
    },
    /// Log out from AI provider (delete stored OAuth tokens)
    Logout {
        /// Provider to log out from (e.g., "anthropic")
        provider: Option<String>,
    },
    /// Show authentication status for all providers
    Status,
    /// Force refresh OAuth tokens
    Refresh {
        /// Provider to refresh tokens for
        provider: String,
    },
    /// Set up a Claude Code subscription token for API access
    SetupToken,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Check configuration for errors and warnings
    Check,
}

#[derive(Subcommand)]
pub enum ChannelAction {
    /// List all channels and their status
    List,
    /// Interactive setup for a channel
    Setup {
        /// Channel name (telegram, discord, slack, whatsapp, webhook)
        channel_name: String,
    },
    /// Test channel connectivity
    Test {
        /// Channel name (telegram, discord, slack, whatsapp, webhook)
        channel_name: String,
    },
}

#[derive(Subcommand)]
pub enum HistoryAction {
    /// List recent CLI conversations
    List {
        /// Maximum number of conversations to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show a conversation by session key or title query
    Show {
        /// Session key (exact) or title substring (case-insensitive)
        query: String,
    },
    /// Remove old CLI conversations
    Cleanup {
        /// Keep this many most-recent conversations
        #[arg(long, default_value_t = 50)]
        keep: usize,
    },
}

#[derive(Subcommand)]
pub enum TemplateAction {
    /// List available templates (built-in + user-defined)
    List,
    /// Show full template details
    Show {
        /// Template name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    /// Encrypt all plaintext secrets in config
    Encrypt,
    /// Decrypt all secrets for editing
    Decrypt,
    /// Re-encrypt with a new key
    Rotate,
}

#[derive(Subcommand)]
pub enum PairAction {
    /// Generate a new 6-digit pairing code
    New,
    /// List all paired devices
    List,
    /// Revoke a paired device
    Revoke {
        /// Device name to revoke
        device: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum HardwareAction {
    /// List discovered USB devices
    List,
    /// Show info about a specific device
    Info {
        /// Device name or VID:PID (e.g., "nucleo-f401re" or "0483:374b")
        device: String,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum BatchFormat {
    Text,
    Jsonl,
}

/// Entry point for the CLI — called from main().
pub async fn run() -> Result<()> {
    // Initialize logging from config (format, level, optional file output).
    // Load config early so we can respect the logging settings; fall back to
    // defaults if the config file is missing or unreadable.
    let logging_cfg = zeptoclaw::config::Config::load()
        .map(|c| c.logging)
        .unwrap_or_default();
    zeptoclaw::utils::logging::init_logging(&logging_cfg);

    let cli = Cli::parse();

    match cli.command {
        None => {
            let mut cmd = Cli::command();
            cmd.print_help()?;
            println!();
        }
        Some(Commands::Version) => {
            cmd_version();
        }
        Some(Commands::Onboard { full }) => {
            onboard::cmd_onboard(full).await?;
        }
        Some(Commands::Agent {
            message,
            template,
            stream,
            dry_run,
            mode,
        }) => {
            agent::cmd_agent(message, template, stream, dry_run, mode).await?;
        }
        Some(Commands::Batch {
            input,
            output,
            format,
            stop_on_error,
            stream,
            template,
        }) => {
            batch::cmd_batch(input, output, format, stop_on_error, stream, template).await?;
        }
        Some(Commands::Gateway {
            containerized,
            tunnel,
        }) => {
            gateway::cmd_gateway(containerized, tunnel).await?;
        }
        Some(Commands::AgentStdin) => {
            agent::cmd_agent_stdin().await?;
        }
        Some(Commands::Heartbeat { show, edit }) => {
            heartbeat::cmd_heartbeat(show, edit).await?;
        }
        Some(Commands::History { action }) => {
            history::cmd_history(action).await?;
        }
        Some(Commands::Memory { action }) => {
            memory::cmd_memory(action).await?;
        }
        Some(Commands::Template { action }) => {
            template::cmd_template(action).await?;
        }
        Some(Commands::Skills { action }) => {
            skills::cmd_skills(action).await?;
        }
        Some(Commands::Tools { action }) => {
            tools::cmd_tools(action).await?;
        }
        Some(Commands::Auth { action }) => {
            status::cmd_auth(action).await?;
        }
        Some(Commands::Status) => {
            status::cmd_status().await?;
        }
        Some(Commands::Channel { action }) => {
            channel::cmd_channel(action).await?;
        }
        Some(Commands::Config { action }) => {
            config::cmd_config(action).await?;
        }
        Some(Commands::Secrets { action }) => {
            secrets::cmd_secrets(action).await?;
        }
        Some(Commands::Watch {
            url,
            interval,
            notify,
        }) => {
            watch::cmd_watch(url, interval, notify).await?;
        }
        Some(Commands::Pair { action }) => {
            pair::cmd_pair(action).await?;
        }
        Some(Commands::Doctor { online }) => {
            doctor::cmd_doctor(online).await?;
        }
        Some(Commands::Daemon) => {
            daemon::cmd_daemon().await?;
        }
        Some(Commands::Migrate { from, yes, dry_run }) => {
            migrate::cmd_migrate(from, yes, dry_run).await?;
        }
        Some(Commands::Update {
            check,
            version,
            force,
        }) => {
            update::cmd_update(check, version, force).await?;
        }
        Some(Commands::Hardware { action }) => {
            cmd_hardware(action);
        }
    }

    Ok(())
}

/// Display version information
fn cmd_version() {
    println!("zeptoclaw {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Ultra-lightweight personal AI assistant");
    println!("https://github.com/qhkm/zeptoclaw");
}

/// Handle hardware subcommands (list, info).
fn cmd_hardware(action: HardwareAction) {
    use zeptoclaw::hardware::HardwareManager;

    let mgr = HardwareManager::new();

    match action {
        HardwareAction::List => {
            let devices = mgr.discover_devices();
            if devices.is_empty() {
                println!("No hardware devices found.");
                println!();
                #[cfg(not(feature = "hardware"))]
                {
                    println!("Hardware discovery requires the 'hardware' feature.");
                    println!("Build with: cargo build --features hardware");
                }
                #[cfg(feature = "hardware")]
                {
                    println!(
                        "Connect a board (e.g., Nucleo-F401RE, Arduino) via USB and try again."
                    );
                }
            } else {
                println!("Discovered devices:");
                println!();
                for d in &devices {
                    let arch = d.architecture.as_deref().unwrap_or("--");
                    let detail = d.detail.as_deref().unwrap_or("--");
                    println!(
                        "  {:04x}:{:04x}  {:<20} {:<20} {}",
                        d.vid, d.pid, d.name, arch, detail
                    );
                }
                println!();
                println!("{} device(s) found.", devices.len());
            }
        }
        HardwareAction::Info { device } => match mgr.device_info(&device) {
            Some(info) => {
                println!("Device: {}", info.name);
                println!("  VID:PID       {:04x}:{:04x}", info.vid, info.pid);
                if let Some(arch) = &info.architecture {
                    println!("  Architecture  {}", arch);
                }
                if let Some(detail) = &info.detail {
                    println!("  Description   {}", detail);
                }
                if let Some(path) = &info.device_path {
                    println!("  Serial path   {}", path);
                }
            }
            None => {
                println!("Device '{}' not found.", device);
                println!();
                println!("Try: zeptoclaw hardware list");
                println!("Or use a VID:PID format (e.g., 0483:374b)");
            }
        },
    }
}
