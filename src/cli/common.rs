//! Shared CLI helpers used across multiple command handlers.

use std::collections::HashSet;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{info, warn};

use zeptoclaw::agent::{AgentLoop, ContextBuilder, RuntimeContext};
use zeptoclaw::bus::MessageBus;
use zeptoclaw::config::templates::{AgentTemplate, TemplateRegistry};
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

pub(crate) fn load_template_registry() -> Result<TemplateRegistry> {
    let mut registry = TemplateRegistry::new();
    let template_dir = Config::dir().join("templates");
    registry
        .merge_from_dir(&template_dir)
        .with_context(|| format!("Failed to load templates from {}", template_dir.display()))?;
    Ok(registry)
}

pub(crate) fn resolve_template(name: &str) -> Result<AgentTemplate> {
    let registry = load_template_registry()?;
    if let Some(template) = registry.get(name) {
        return Ok(template.clone());
    }

    let mut available = registry
        .names()
        .into_iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>();
    available.sort();

    anyhow::bail!(
        "Template '{}' not found. Available templates: {}",
        name,
        available.join(", ")
    );
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
    create_agent_with_template(config, bus, None).await
}

/// Create and configure an agent with optional template overrides.
pub(crate) async fn create_agent_with_template(
    mut config: Config,
    bus: Arc<MessageBus>,
    template: Option<AgentTemplate>,
) -> Result<Arc<AgentLoop>> {
    if let Some(tpl) = &template {
        if let Some(model) = &tpl.model {
            config.agents.defaults.model = model.clone();
        }
        if let Some(max_tokens) = tpl.max_tokens {
            config.agents.defaults.max_tokens = max_tokens;
        }
        if let Some(temperature) = tpl.temperature {
            config.agents.defaults.temperature = temperature;
        }
        if let Some(max_tool_iterations) = tpl.max_tool_iterations {
            config.agents.defaults.max_tool_iterations = max_tool_iterations;
        }
    }

    let allowed_tools = template
        .as_ref()
        .and_then(|tpl| tpl.allowed_tools.as_ref())
        .map(|names| {
            names
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        });
    let blocked_tools = template
        .as_ref()
        .and_then(|tpl| tpl.blocked_tools.as_ref())
        .map(|names| {
            names
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    // Resolve tool profile: config default > template override > all tools
    let profile_tools: Option<HashSet<String>> =
        if let Some(ref profile_name) = config.agents.defaults.tool_profile {
            match config.tool_profiles.get(profile_name) {
                Some(tools) => tools
                    .as_ref()
                    .map(|names| names.iter().map(|n| n.to_ascii_lowercase()).collect()),
                None => {
                    warn!(
                        "Tool profile '{}' not found in tool_profiles config — all tools enabled",
                        profile_name
                    );
                    None
                }
            }
        } else {
            None
        };

    let tool_enabled = |name: &str| {
        let key = name.to_ascii_lowercase();
        // Profile filter (if active)
        if let Some(ref profile) = profile_tools {
            if !profile.contains(&key) {
                return false;
            }
        }
        // Template allowed filter
        if let Some(allowed) = &allowed_tools {
            if !allowed.contains(&key) {
                return false;
            }
        }
        !blocked_tools.contains(&key)
    };

    // Create session manager
    let session_manager = SessionManager::new().unwrap_or_else(|_| {
        warn!("Failed to create persistent session manager, using in-memory");
        SessionManager::new_memory()
    });

    let skills_prompt = build_skills_prompt(&config);
    let mut context_builder = ContextBuilder::new();

    // Load SOUL.md from workspace if present
    let soul_path = config.workspace_path().join("SOUL.md");
    if soul_path.is_file() {
        match std::fs::read_to_string(&soul_path) {
            Ok(content) => {
                let content = content.trim();
                if !content.is_empty() {
                    context_builder = context_builder.with_soul(content);
                    info!("Loaded SOUL.md from {}", soul_path.display());
                }
            }
            Err(e) => warn!("Failed to read SOUL.md at {}: {}", soul_path.display(), e),
        }
    }

    if let Some(tpl) = &template {
        context_builder = context_builder.with_system_prompt(&tpl.system_prompt);
    }
    if !skills_prompt.is_empty() {
        context_builder = context_builder.with_skills(&skills_prompt);
    }

    // Inject pinned memories into system prompt
    if !matches!(config.memory.backend, MemoryBackend::Disabled) {
        match zeptoclaw::memory::longterm::LongTermMemory::new() {
            Ok(ltm) => {
                let memory_ctx = zeptoclaw::memory::build_memory_injection(
                    &ltm,
                    "",
                    zeptoclaw::memory::MEMORY_INJECTION_BUDGET,
                );
                if !memory_ctx.is_empty() {
                    context_builder = context_builder.with_memory_context(memory_ctx);
                    info!("Injected pinned memories into system prompt");
                }
            }
            Err(e) => warn!("Failed to load long-term memory for injection: {}", e),
        }
    }

    // Build runtime context for environment awareness (time, platform, etc.)
    let runtime_ctx = RuntimeContext::new()
        .with_timezone(&config.agents.defaults.timezone)
        .with_os_info();
    context_builder = context_builder.with_runtime_context(runtime_ctx);

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
    if tool_enabled("echo") {
        agent.register_tool(Box::new(EchoTool)).await;
    }
    if tool_enabled("read_file") {
        agent.register_tool(Box::new(ReadFileTool)).await;
    }
    if tool_enabled("write_file") {
        agent.register_tool(Box::new(WriteFileTool)).await;
    }
    if tool_enabled("list_dir") {
        agent.register_tool(Box::new(ListDirTool)).await;
    }
    if tool_enabled("edit_file") {
        agent.register_tool(Box::new(EditFileTool)).await;
    }
    if tool_enabled("shell") {
        agent
            .register_tool(Box::new(ShellTool::with_runtime(runtime)))
            .await;
    }

    // Register web tools.
    if tool_enabled("web_search") {
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
    }
    if tool_enabled("web_fetch") {
        agent.register_tool(Box::new(WebFetchTool::new())).await;
        info!("Registered web_fetch tool");
    }

    // Register proactive messaging tool.
    if tool_enabled("message") {
        agent
            .register_tool(Box::new(MessageTool::new(agent.bus().clone())))
            .await;
        info!("Registered message tool");
    }

    // Register WhatsApp tool.
    if tool_enabled("whatsapp_send") {
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
    }

    // Register Google Sheets tool.
    if tool_enabled("google_sheets") {
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
    }

    match &config.memory.backend {
        MemoryBackend::Disabled => {
            info!("Memory tools are disabled");
        }
        MemoryBackend::Builtin => {
            if tool_enabled("memory_search") {
                agent
                    .register_tool(Box::new(MemorySearchTool::new(config.memory.clone())))
                    .await;
            }
            if tool_enabled("memory_get") {
                agent
                    .register_tool(Box::new(MemoryGetTool::new(config.memory.clone())))
                    .await;
            }
            if tool_enabled("longterm_memory") {
                match zeptoclaw::tools::longterm_memory::LongTermMemoryTool::new() {
                    Ok(tool) => {
                        agent.register_tool(Box::new(tool)).await;
                        info!("Registered longterm_memory tool");
                    }
                    Err(e) => warn!("Failed to initialize longterm_memory tool: {}", e),
                }
            }
            info!("Registered memory_search and memory_get tools");
        }
        MemoryBackend::Qmd => {
            warn!("Memory backend 'qmd' is not implemented yet; using built-in memory tools");
            if tool_enabled("memory_search") {
                agent
                    .register_tool(Box::new(MemorySearchTool::new(config.memory.clone())))
                    .await;
            }
            if tool_enabled("memory_get") {
                agent
                    .register_tool(Box::new(MemoryGetTool::new(config.memory.clone())))
                    .await;
            }
            if tool_enabled("longterm_memory") {
                match zeptoclaw::tools::longterm_memory::LongTermMemoryTool::new() {
                    Ok(tool) => {
                        agent.register_tool(Box::new(tool)).await;
                        info!("Registered longterm_memory tool");
                    }
                    Err(e) => warn!("Failed to initialize longterm_memory tool: {}", e),
                }
            }
            info!("Registered memory_search and memory_get tools");
        }
    }

    if tool_enabled("cron") {
        agent
            .register_tool(Box::new(CronTool::new(cron_service.clone())))
            .await;
    }
    if tool_enabled("spawn") {
        agent
            .register_tool(Box::new(SpawnTool::new(
                Arc::downgrade(&agent),
                agent.bus().clone(),
            )))
            .await;
    }
    if tool_enabled("r8r") {
        agent.register_tool(Box::new(R8rTool::default())).await;
    }
    if tool_enabled("reminder") {
        match zeptoclaw::tools::reminder::ReminderTool::new(Some(cron_service.clone())) {
            Ok(tool) => {
                agent.register_tool(Box::new(tool)).await;
                info!("Registered reminder tool");
            }
            Err(e) => warn!("Failed to initialize reminder tool: {}", e),
        }
    }

    // Register plugin tools (command-mode and binary-mode)
    if config.plugins.enabled {
        let plugin_dirs: Vec<PathBuf> = config
            .plugins
            .plugin_dirs
            .iter()
            .map(|d| expand_tilde(d))
            .collect();
        match zeptoclaw::plugins::discover_plugins(&plugin_dirs) {
            Ok(plugins) => {
                for plugin in plugins {
                    if !config.plugins.is_plugin_permitted(plugin.name()) {
                        info!(plugin = %plugin.name(), "Plugin blocked by config");
                        continue;
                    }
                    for tool_def in &plugin.manifest.tools {
                        if !tool_enabled(&tool_def.name) {
                            continue;
                        }
                        if plugin.manifest.is_binary() {
                            if let Some(ref bin_cfg) = plugin.manifest.binary {
                                match zeptoclaw::plugins::validate_binary_path(
                                    &plugin.path,
                                    bin_cfg,
                                ) {
                                    Ok(bin_path) => {
                                        let timeout = bin_cfg
                                            .timeout_secs
                                            .unwrap_or_else(|| tool_def.effective_timeout());
                                        agent
                                            .register_tool(Box::new(
                                                zeptoclaw::tools::binary_plugin::BinaryPluginTool::new(
                                                    tool_def.clone(),
                                                    plugin.name(),
                                                    bin_path,
                                                    timeout,
                                                ),
                                            ))
                                            .await;
                                        info!(
                                            plugin = %plugin.name(),
                                            tool = %tool_def.name,
                                            "Registered binary plugin tool"
                                        );
                                    }
                                    Err(e) => warn!(
                                        plugin = %plugin.name(),
                                        error = %e,
                                        "Binary validation failed"
                                    ),
                                }
                            }
                        } else {
                            agent
                                .register_tool(Box::new(zeptoclaw::tools::plugin::PluginTool::new(
                                    tool_def.clone(),
                                    plugin.name(),
                                )))
                                .await;
                            info!(
                                plugin = %plugin.name(),
                                tool = %tool_def.name,
                                "Registered command plugin tool"
                            );
                        }
                    }
                }
            }
            Err(e) => warn!(error = %e, "Plugin discovery failed"),
        }
    }

    // Validate and register custom CLI-defined tools
    let tool_warnings = zeptoclaw::config::validate::validate_custom_tools(&config);
    for w in &tool_warnings {
        warn!("Custom tool config: {}", w);
    }
    for tool_def in &config.custom_tools {
        if !tool_enabled(&tool_def.name) {
            continue;
        }
        // Skip tools with empty commands (caught by validate_custom_tools)
        if tool_def.command.trim().is_empty() {
            warn!(tool = %tool_def.name, "Skipping custom tool with empty command");
            continue;
        }
        let tool = zeptoclaw::tools::custom::CustomTool::new(tool_def.clone());
        agent.register_tool(Box::new(tool)).await;
        info!(tool = %tool_def.name, "Registered custom CLI tool");
    }

    info!("Registered {} tools", agent.tool_count().await);

    // Set up provider
    if let Some(runtime_provider) = resolve_runtime_provider(&config) {
        match runtime_provider.backend {
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
        info!(
            "Configured runtime provider: {} (backend: {})",
            runtime_provider.name, runtime_provider.backend
        );
    }

    let unsupported = zeptoclaw::providers::configured_unsupported_provider_names(&config);
    if !unsupported.is_empty() {
        warn!(
            "Configured provider(s) not yet supported by runtime: {}",
            unsupported.join(", ")
        );
    }

    // Register DelegateTool for agent swarm delegation (requires provider)
    if tool_enabled("delegate") && config.swarm.enabled {
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

/// Validate an API key by making a minimal API call.
/// Returns Ok(()) if key works, Err with user-friendly message if not.
pub(crate) async fn validate_api_key(
    provider: &str,
    api_key: &str,
    api_base: Option<&str>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    match provider {
        "anthropic" => {
            // Use read-only /v1/models endpoint to validate key without consuming tokens.
            let base = api_base.unwrap_or("https://api.anthropic.com");
            let resp = client
                .get(format!("{}/v1/models", base))
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await?;
            if resp.status().is_success() {
                Ok(())
            } else {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                Err(anyhow::anyhow!(friendly_api_error(
                    "anthropic",
                    status,
                    &body
                )))
            }
        }
        "openai" => {
            let base = api_base.unwrap_or("https://api.openai.com/v1");
            let resp = client
                .get(format!("{}/models", base))
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await?;
            if resp.status().is_success() {
                Ok(())
            } else {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                Err(anyhow::anyhow!(friendly_api_error("openai", status, &body)))
            }
        }
        _ => {
            warn!(
                "API key validation not supported for provider '{}', skipping",
                provider
            );
            Ok(())
        }
    }
}

/// Map HTTP status to user-friendly error message with actionable guidance.
pub(crate) fn friendly_api_error(provider: &str, status: u16, body: &str) -> String {
    // Try to extract a message from the provider's JSON error response.
    let api_msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message").or_else(|| e.as_str().map(|_| e)))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        });

    let base = match status {
        401 => format!(
            "Invalid API key. Check your {} key and try again.\n  {}",
            provider,
            if provider == "anthropic" {
                "Get key: https://console.anthropic.com/"
            } else {
                "Get key: https://platform.openai.com/api-keys"
            }
        ),
        402 => format!(
            "Billing issue on your {} account. Add a payment method.\n  {}",
            provider,
            if provider == "anthropic" {
                "Billing: https://console.anthropic.com/settings/billing"
            } else {
                "Billing: https://platform.openai.com/settings/organization/billing"
            }
        ),
        429 => "Rate limited. Wait a moment and try again.".to_string(),
        404 => {
            "Model not found. Your API key may not have access to the default model.".to_string()
        }
        _ => format!(
            "API returned HTTP {}. Check your API key and account status.",
            status
        ),
    };

    if let Some(msg) = api_msg {
        format!("{}\n  Detail: {}", base, msg)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_friendly_api_error_401_anthropic() {
        let msg = friendly_api_error("anthropic", 401, "");
        assert!(msg.contains("Invalid API key"));
        assert!(msg.contains("anthropic"));
        assert!(msg.contains("console.anthropic.com"));
    }

    #[test]
    fn test_friendly_api_error_401_openai() {
        let msg = friendly_api_error("openai", 401, "");
        assert!(msg.contains("Invalid API key"));
        assert!(msg.contains("openai"));
        assert!(msg.contains("platform.openai.com"));
    }

    #[test]
    fn test_friendly_api_error_402() {
        let msg = friendly_api_error("anthropic", 402, "");
        assert!(msg.contains("Billing issue"));
    }

    #[test]
    fn test_friendly_api_error_unknown_status() {
        let msg = friendly_api_error("openai", 500, "");
        assert!(msg.contains("HTTP 500"));
    }
}
