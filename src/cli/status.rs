//! Status and auth command handlers.

use anyhow::{Context, Result};

use zeptoclaw::auth;
use zeptoclaw::config::{Config, ContainerAgentBackend, ProviderConfig};
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
        AuthAction::Login { provider } => {
            cmd_auth_login(provider).await?;
        }
        AuthAction::Logout { provider } => {
            cmd_auth_logout(provider)?;
        }
        AuthAction::Status => {
            cmd_auth_status().await?;
        }
        AuthAction::Refresh { provider } => {
            cmd_auth_refresh(&provider).await?;
        }
    }
    Ok(())
}

/// OAuth login flow.
async fn cmd_auth_login(provider: Option<String>) -> Result<()> {
    let provider = provider.unwrap_or_else(|| {
        println!(
            "OAuth-supported providers: {}",
            auth::oauth_supported_providers().join(", ")
        );
        println!();
        "anthropic".to_string()
    });

    let oauth_config = auth::provider_oauth_config(&provider).ok_or_else(|| {
        anyhow::anyhow!(
            "Provider '{}' does not support OAuth authentication.\n\
             Supported providers: {}\n\n\
             To configure API keys instead:\n  \
             export ZEPTOCLAW_PROVIDERS_{}_API_KEY=your-key-here",
            provider,
            auth::oauth_supported_providers().join(", "),
            provider.to_uppercase()
        )
    })?;

    println!("WARNING: Using OAuth subscription tokens for API access may violate");
    println!("the provider's Terms of Service. The provider may block these tokens");
    println!("at any time. If blocked, ZeptoClaw will fall back to your API key.");
    println!();

    // Use a simple client ID (dynamic registration would be better but more complex)
    let client_id = "zeptoclaw";

    let tokens = auth::oauth::run_oauth_flow(&oauth_config, client_id).await?;

    // Store the tokens
    let encryption = zeptoclaw::security::encryption::resolve_master_key(true)
        .map_err(|e| anyhow::anyhow!("Cannot store tokens without encryption key: {}", e))?;

    let store = auth::store::TokenStore::new(encryption);
    store
        .save(&tokens)
        .map_err(|e| anyhow::anyhow!("Failed to save tokens: {}", e))?;

    println!();
    println!("Authenticated with {} successfully!", provider);
    if tokens.expires_at.is_some() {
        println!("Token expires in: {}", tokens.expires_in_human());
    }
    if tokens.refresh_token.is_some() {
        println!("Refresh token: stored (will auto-refresh before expiry)");
    }

    // Suggest config update
    println!();
    println!("To use OAuth tokens automatically, update your config:");
    println!(
        r#"  "providers": {{ "{}": {{ "auth_method": "auto" }} }}"#,
        provider
    );

    Ok(())
}

/// OAuth logout.
fn cmd_auth_logout(provider: Option<String>) -> Result<()> {
    let encryption = match zeptoclaw::security::encryption::resolve_master_key(true) {
        Ok(enc) => enc,
        Err(_) => {
            println!("No encryption key available. If you have stored tokens,");
            println!("delete them manually: rm ~/.zeptoclaw/auth/tokens.json.enc");
            return Ok(());
        }
    };

    let store = auth::store::TokenStore::new(encryption);

    if let Some(provider) = provider {
        match store.delete(&provider) {
            Ok(true) => println!("Logged out from {} (OAuth tokens removed).", provider),
            Ok(false) => println!("No OAuth tokens stored for '{}'.", provider),
            Err(e) => println!("Failed to remove tokens: {}", e),
        }
    } else {
        // Show what's stored and ask which to remove
        match store.list() {
            Ok(entries) if entries.is_empty() => {
                println!("No OAuth tokens stored.");
            }
            Ok(entries) => {
                println!("Stored OAuth tokens:");
                for (name, summary) in &entries {
                    println!(
                        "  {}: {} (refresh: {})",
                        name,
                        if summary.is_expired {
                            "expired"
                        } else {
                            &summary.expires_in
                        },
                        if summary.has_refresh_token {
                            "yes"
                        } else {
                            "no"
                        },
                    );
                }
                println!();
                println!("Specify a provider to log out: zeptoclaw auth logout <provider>");
            }
            Err(e) => println!("Failed to read token store: {}", e),
        }
    }

    Ok(())
}

/// Force refresh OAuth tokens.
async fn cmd_auth_refresh(provider: &str) -> Result<()> {
    let encryption = zeptoclaw::security::encryption::resolve_master_key(true)
        .map_err(|e| anyhow::anyhow!("Cannot access tokens without encryption key: {}", e))?;

    let store = auth::store::TokenStore::new(encryption);

    match auth::refresh::ensure_fresh_token(&store, provider).await {
        Ok(_) => {
            println!("Token refreshed for {}.", provider);
            println!("New token stored securely.");
        }
        Err(e) => {
            println!("Failed to refresh token for {}: {}", provider, e);
            println!();
            println!("Try logging in again: zeptoclaw auth login {}", provider);
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

    // Load OAuth token store (best-effort)
    let token_store = zeptoclaw::security::encryption::resolve_master_key(false)
        .ok()
        .map(auth::store::TokenStore::new);

    let oauth_status = |name: &str| -> String {
        if let Some(ref store) = token_store {
            if let Ok(Some(token)) = store.load(name) {
                if token.is_expired() {
                    return format!(
                        "OAuth (expired{})",
                        if token.refresh_token.is_some() {
                            ", has refresh token"
                        } else {
                            ""
                        }
                    );
                }
                return format!("OAuth (expires in {})", token.expires_in_human());
            }
        }
        String::new()
    };

    let provider_display = |name: &str, label: &str, provider: &Option<ProviderConfig>| {
        let api = provider_status(provider);
        let oauth = oauth_status(name);
        let auth_method = provider
            .as_ref()
            .and_then(|p| p.auth_method.as_deref())
            .unwrap_or("api_key");

        if !oauth.is_empty() {
            println!("  {}: {} | {} [method: {}]", label, api, oauth, auth_method);
        } else {
            println!("  {}: {} [method: {}]", label, api, auth_method);
        }
    };

    provider_display(
        "anthropic",
        "Anthropic (Claude)",
        &config.providers.anthropic,
    );
    provider_display("openai", "OpenAI            ", &config.providers.openai);
    provider_display(
        "openrouter",
        "OpenRouter        ",
        &config.providers.openrouter,
    );
    provider_display("groq", "Groq              ", &config.providers.groq);
    provider_display("gemini", "Gemini            ", &config.providers.gemini);
    provider_display("zhipu", "Zhipu             ", &config.providers.zhipu);

    println!();
    println!(
        "OAuth-supported providers: {}",
        auth::oauth_supported_providers().join(", ")
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

    // Long-term memory stats
    let ltm_path = Config::dir().join("memory").join("longterm.json");
    if ltm_path.exists() {
        match zeptoclaw::memory::longterm::LongTermMemory::new() {
            Ok(mem) => {
                let count = mem.count();
                let categories = mem.categories();
                println!("  Long-term entries: {}", count);
                if !categories.is_empty() {
                    println!("  Categories: {}", categories.join(", "));
                }
            }
            Err(_) => {
                println!("  Long-term memory: error reading");
            }
        }
    } else {
        println!("  Long-term entries: 0 (no data file)");
    }
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

    // Available tools (dynamic)
    println!("Available Tools");
    println!("---------------");
    super::tools::print_tools_summary(&config);
    println!();

    Ok(())
}
