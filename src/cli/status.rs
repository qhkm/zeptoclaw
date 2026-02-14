//! Status and auth command handlers.

use anyhow::{Context, Result};

use zeptoclaw::config::{Config, ContainerAgentBackend, MemoryBackend, ProviderConfig};
use zeptoclaw::providers::{
    configured_unsupported_provider_names, resolve_runtime_provider, RUNTIME_SUPPORTED_PROVIDERS,
};
use zeptoclaw::runtime::available_runtimes;

use super::common::{memory_backend_label, memory_citations_label, skills_loader_from_config};
use super::heartbeat::heartbeat_file_path;
use super::AuthAction;

/// Manage authentication.
pub(crate) async fn cmd_auth(action: AuthAction) -> Result<()> {
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

/// Show authentication status.
async fn cmd_auth_status() -> Result<()> {
    let config = Config::load().with_context(|| "Failed to load configuration")?;

    println!("Authentication Status");
    println!("=====================");
    println!();

    println!(
        "  Anthropic (Claude): {}",
        provider_status(&config.providers.anthropic)
    );
    println!(
        "  OpenAI:             {}",
        provider_status(&config.providers.openai)
    );
    println!(
        "  OpenRouter:         {}",
        provider_status(&config.providers.openrouter)
    );
    println!(
        "  Groq:               {}",
        provider_status(&config.providers.groq)
    );
    println!(
        "  Gemini:             {}",
        provider_status(&config.providers.gemini)
    );
    println!(
        "  Zhipu:              {}",
        provider_status(&config.providers.zhipu)
    );

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

fn provider_status(provider: &Option<ProviderConfig>) -> &'static str {
    provider
        .as_ref()
        .and_then(|p| p.api_key.as_ref())
        .map(|k| {
            if k.is_empty() {
                "not set"
            } else {
                "configured"
            }
        })
        .unwrap_or("not set")
}

/// Show system status.
pub(crate) async fn cmd_status() -> Result<()> {
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
