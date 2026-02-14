//! Shared CLI helpers used across multiple command handlers.

use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{info, warn};

use zeptoclaw::agent::{AgentLoop, ContextBuilder};
use zeptoclaw::bus::MessageBus;
use zeptoclaw::config::{Config, MemoryBackend, MemoryCitationsMode};
use zeptoclaw::cron::CronService;
use zeptoclaw::providers::{resolve_runtime_provider, ClaudeProvider, OpenAIProvider};
use zeptoclaw::runtime::{create_runtime, NativeRuntime};
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

/// Read a line from stdin, trimming whitespace.
pub(crate) fn read_line() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .with_context(|| "Failed to read input")?;
    Ok(input.trim().to_string())
}

/// Read a password/API key from stdin (hidden input).
pub(crate) fn read_secret() -> Result<String> {
    rpassword::read_password_from_bufread(&mut std::io::stdin().lock())
        .with_context(|| "Failed to read secret input")
}

/// Expand `~/` prefix to the user's home directory.
pub(crate) fn expand_tilde(path: &str) -> PathBuf {
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

pub(crate) fn memory_backend_label(backend: &MemoryBackend) -> &'static str {
    match backend {
        MemoryBackend::Disabled => "none",
        MemoryBackend::Builtin => "builtin",
        MemoryBackend::Qmd => "qmd",
    }
}

pub(crate) fn memory_citations_label(mode: &MemoryCitationsMode) -> &'static str {
    match mode {
        MemoryCitationsMode::Auto => "auto",
        MemoryCitationsMode::On => "on",
        MemoryCitationsMode::Off => "off",
    }
}

pub(crate) fn skills_loader_from_config(config: &Config) -> SkillsLoader {
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

/// Create and configure an agent with all tools registered.
pub(crate) async fn create_agent(config: Config, bus: Arc<MessageBus>) -> Result<Arc<AgentLoop>> {
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

    let unsupported = zeptoclaw::providers::configured_unsupported_provider_names(&config);
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
