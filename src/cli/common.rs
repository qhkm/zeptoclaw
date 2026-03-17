//! Shared CLI helpers used across multiple command handlers.

use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{info, warn};

use zeptoclaw::agent::{AgentLoop, ContextBuilder, RuntimeContext};
use zeptoclaw::bus::MessageBus;
use zeptoclaw::config::templates::{AgentTemplate, TemplateRegistry};
use zeptoclaw::config::{Config, MemoryBackend, MemoryCitationsMode};
use zeptoclaw::hands::resolve_hand;
use zeptoclaw::providers::{
    resolve_runtime_providers, FallbackProvider, LLMProvider, ProviderPlugin,
};
use zeptoclaw::session::SessionManager;
use zeptoclaw::skills::SkillsLoader;
use zeptoclaw::tools::approval::ApprovalPolicyConfig;
use zeptoclaw::tools::delegate::DelegateTool;
use zeptoclaw::tools::spawn::SpawnTool;

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
        MemoryBackend::Bm25 => "bm25",
        MemoryBackend::Embedding => "embedding",
        MemoryBackend::Hnsw => "hnsw",
        MemoryBackend::Tantivy => "tantivy",
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

// Provider functions extracted to zeptoclaw::kernel::provider.
// Re-import for use within this module.
use zeptoclaw::kernel::provider::{apply_retry_wrapper, provider_from_runtime_selection};

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
    let active_hand = if template.is_none() {
        if let Some(name) = config.agents.defaults.active_hand.as_deref() {
            let hands_dir = Config::dir().join("hands");
            match resolve_hand(name, &hands_dir)? {
                Some(hand) => Some(hand),
                None => {
                    warn!("Active hand '{}' not found, continuing without hand", name);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some(hand) = active_hand.as_ref() {
        if !hand.manifest.guardrails.require_approval_for.is_empty() {
            config.approval.enabled = true;
            config.approval.policy = ApprovalPolicyConfig::RequireForTools;
            for pattern in &hand.manifest.guardrails.require_approval_for {
                if !config.approval.require_for.contains(pattern) {
                    config.approval.require_for.push(pattern.clone());
                }
            }
        }
    }

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
        if let Some(budget) = tpl.max_token_budget {
            config.agents.defaults.token_budget = zeptoclaw::agent::budget::resolve_token_budget(
                config.agents.defaults.token_budget,
                Some(budget),
            );
        }
        if let Some(tpl_limit) = tpl.max_tool_calls {
            config.agents.defaults.max_tool_calls =
                Some(match config.agents.defaults.max_tool_calls {
                    Some(global) => global.min(tpl_limit),
                    None => tpl_limit,
                });
        }
    }

    // --- Kernel boot: assemble shared subsystems ---
    let kernel = zeptoclaw::kernel::ZeptoKernel::boot(
        config.clone(),
        bus.clone(),
        template.as_ref(),
        active_hand.as_ref().map(|h| &h.manifest),
    )
    .await?;

    // --- Per-session state: context builder, agent loop ---
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

    if let Some(sp) = &config.agents.defaults.system_prompt {
        context_builder = context_builder.with_system_prompt(sp);
    } else if let Some(tpl) = &template {
        context_builder = context_builder.with_system_prompt(&tpl.system_prompt);
    } else if let Some(hand) = active_hand.as_ref() {
        context_builder = context_builder.with_system_prompt(&hand.manifest.system_prompt);
    }
    if !skills_prompt.is_empty() {
        context_builder = context_builder.with_skills(&skills_prompt);
    }
    if let Some(hand) = active_hand.as_ref() {
        if !hand.skill_md.trim().is_empty() {
            context_builder = context_builder.with_skills(&hand.skill_md);
        }
    }

    // Build runtime context for environment awareness (time, platform, etc.)
    let runtime_ctx = RuntimeContext::new()
        .with_timezone(&config.agents.defaults.timezone)
        .with_os_info();
    context_builder = context_builder.with_runtime_context(runtime_ctx);

    // Create agent loop
    let mut agent_loop =
        AgentLoop::with_context_builder(config.clone(), session_manager, bus, context_builder);
    if let Some(ref ltm) = kernel.ltm {
        agent_loop.set_ltm(ltm.clone());
        info!("Wired shared LTM into agent for per-message memory injection");
    }
    if let Some(ref taint) = kernel.taint {
        agent_loop.set_taint(Arc::clone(taint));
        info!("Wired shared taint engine into agent loop");
    }
    let agent = Arc::new(agent_loop);

    // Transfer kernel tools + MCP clients into agent
    agent
        .merge_kernel_tools(kernel.tools, kernel.mcp_clients)
        .await;

    // Register per-session tools that need Weak<AgentLoop>
    let filter = zeptoclaw::kernel::ToolFilter::from_config(
        &config,
        template.as_ref(),
        active_hand.as_ref().map(|h| &h.manifest),
    );
    if filter.is_enabled("spawn") {
        agent
            .register_tool(Box::new(SpawnTool::new(
                Arc::downgrade(&agent),
                agent.bus().clone(),
            )))
            .await;
    }

    // Register Google Workspace tool (deferred from kernel registrar because it
    // needs async OAuth token resolution).
    #[cfg(feature = "google")]
    if filter.is_enabled("google") {
        let google_token = resolve_google_token(&config).await;
        if let Some(token) = google_token {
            agent
                .register_tool(Box::new(zeptoclaw::tools::GoogleTool::new(
                    &token,
                    &config.tools.google.default_calendar,
                    config.tools.google.max_search_results,
                )))
                .await;
            info!("Registered google tool");
        }
    }

    info!("Registered {} tools", agent.tool_count().await);

    // Set provider from kernel (already assembled: base → fallback → retry → quota)
    if let Some(provider) = kernel.provider {
        agent.set_provider_arc(provider).await;
    }

    // Build provider registry for runtime model switching (/model command).
    // Each configured provider is registered individually (without retry/fallback wrappers)
    // so /model can switch between them at runtime.
    for selection in resolve_runtime_providers(&config) {
        if let Some(provider) =
            provider_from_runtime_selection(&selection, &config.agents.defaults.model)
        {
            agent
                .set_provider_in_registry(selection.name, provider)
                .await;
            info!(
                provider = selection.name,
                "Registered provider in model-switch registry"
            );
        }
    }

    // Register provider plugins (JSON-RPC 2.0 over stdin/stdout).
    // Plugin providers are registered only when no runtime provider (Claude/OpenAI/etc.)
    // has been configured. The first plugin becomes primary; subsequent plugins are
    // chained as fallbacks when `providers.fallback.enabled` is true.
    if agent.provider().await.is_none() && !config.providers.plugins.is_empty() {
        let mut plugin_iter = config.providers.plugins.iter();

        // First plugin becomes the primary provider
        if let Some(first_cfg) = plugin_iter.next() {
            let first = ProviderPlugin::new(
                first_cfg.name.clone(),
                first_cfg.command.clone(),
                first_cfg.args.clone(),
            );
            let mut chain: Box<dyn LLMProvider> = Box::new(first);
            let mut chain_names = vec![first_cfg.name.clone()];

            // Additional plugins are appended as fallbacks when enabled
            if config.providers.fallback.enabled {
                for plugin_cfg in plugin_iter {
                    let fallback = ProviderPlugin::new(
                        plugin_cfg.name.clone(),
                        plugin_cfg.command.clone(),
                        plugin_cfg.args.clone(),
                    );
                    chain = Box::new(FallbackProvider::new(chain, Box::new(fallback)));
                    chain_names.push(plugin_cfg.name.clone());
                }
            }

            let chain_label = chain_names.join(" -> ");
            let plugin_count = chain_names.len();
            let chain = apply_retry_wrapper(chain, &config);
            agent.set_provider(chain).await;

            if plugin_count > 1 {
                info!(
                    plugin_count = plugin_count,
                    plugin_chain = %chain_label,
                    "Configured provider plugin fallback chain"
                );
            } else {
                info!("Configured provider plugin: {}", chain_label);
            }
        }
    }

    let unsupported = zeptoclaw::providers::configured_unsupported_provider_names(&config);
    if !unsupported.is_empty() {
        warn!(
            "Configured provider(s) not yet supported by runtime: {}",
            unsupported.join(", ")
        );
    }

    // Register DelegateTool for agent swarm delegation (requires provider)
    if filter.is_enabled("delegate") && config.swarm.enabled {
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

/// Result of API key validation.
#[derive(Debug, PartialEq)]
pub(crate) enum KeyValidation {
    /// Key confirmed valid (2xx response).
    Valid,
    /// Key recognized by server but the account is not currently usable (429).
    /// This typically indicates rate limiting and can also indicate quota
    /// exhaustion, depending on the provider.
    RateLimited,
}

/// Validate an API key by making a minimal API call.
///
/// HTTP 429 returns `Ok(RateLimited)` — the server recognized the key, so it is
/// not an authentication failure. Providers use 429 for rate limiting and, in
/// some cases, quota exhaustion, so onboarding should not report it as an
/// invalid key.
pub(crate) async fn validate_api_key(
    provider: &str,
    api_key: &str,
    api_base: Option<&str>,
) -> anyhow::Result<KeyValidation> {
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
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                Ok(KeyValidation::Valid)
            } else if status == 429 {
                Ok(KeyValidation::RateLimited)
            } else {
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
            validate_bearer_models_key(&client, "openai", api_key, base).await
        }
        "zhipu" => {
            let base = api_base.unwrap_or("https://open.bigmodel.cn/api/paas/v4");
            validate_bearer_models_key(&client, "zhipu", api_key, base).await
        }
        "openrouter" => {
            // OpenRouter has a dedicated key info endpoint.
            let base = api_base.unwrap_or("https://openrouter.ai/api/v1");
            let resp = client
                .get(format!("{}/key", base))
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await?;
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                Ok(KeyValidation::Valid)
            } else if status == 429 {
                Ok(KeyValidation::RateLimited)
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(anyhow::anyhow!(friendly_api_error(
                    "openrouter",
                    status,
                    &body
                )))
            }
        }
        _ => {
            warn!(
                "API key validation not supported for provider '{}', skipping",
                provider
            );
            Ok(KeyValidation::Valid)
        }
    }
}

async fn validate_bearer_models_key(
    client: &reqwest::Client,
    provider: &str,
    api_key: &str,
    base: &str,
) -> anyhow::Result<KeyValidation> {
    let resp = client
        .get(format!("{}/models", base))
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await?;
    let status = resp.status().as_u16();
    if resp.status().is_success() {
        Ok(KeyValidation::Valid)
    } else if status == 429 {
        Ok(KeyValidation::RateLimited)
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(anyhow::anyhow!(friendly_api_error(provider, status, &body)))
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
            match provider {
                "anthropic" => "Get key: https://console.anthropic.com/",
                "zhipu" => "Get key: https://open.bigmodel.cn/",
                "openrouter" => "Get key: https://openrouter.ai/settings/keys",
                _ => "Get key: https://platform.openai.com/api-keys",
            }
        ),
        402 => match provider {
            "openrouter" => {
                "Insufficient OpenRouter credits. Add credits and try again.\n  Credits: https://openrouter.ai/settings/credits"
                    .to_string()
            }
            _ => format!(
                "Billing issue on your {} account. Add a payment method.\n  {}",
                provider,
                match provider {
                    "anthropic" => "Billing: https://console.anthropic.com/settings/billing",
                    "zhipu" => "Billing: https://open.bigmodel.cn/",
                    _ => "Billing: https://platform.openai.com/settings/organization/billing",
                }
            ),
        },
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

/// Resolve Google access token: stored OAuth -> config fallback.
#[cfg(feature = "google")]
async fn resolve_google_token(config: &Config) -> Option<String> {
    // 1. Try stored OAuth token
    let token_path = Config::dir().join("tokens").join("google.json");
    if let Ok(data) = tokio::fs::read_to_string(&token_path).await {
        if let Ok(token_set) = serde_json::from_str::<zeptoclaw::auth::OAuthTokenSet>(&data) {
            if !token_set.is_expired() {
                return Some(token_set.access_token.clone());
            }
            tracing::warn!("Stored Google OAuth token expired, falling back to config");
        }
    }

    // 2. Fall back to static access_token from config
    config
        .tools
        .google
        .access_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_validation_server(
        expected_path: &'static str,
        expected_headers: &'static [(&'static str, &'static str)],
        status: u16,
        body: &'static str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener
            .local_addr()
            .expect("test server should expose address");

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("test server should accept");
            let mut req = Vec::new();
            loop {
                let mut buf = [0_u8; 1024];
                let n = stream
                    .read(&mut buf)
                    .await
                    .expect("test server should read request");
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
                if req.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&req).to_lowercase();

            assert!(
                req.starts_with(&format!("get {} http/1.1", expected_path)),
                "unexpected request line: {req}"
            );
            for (name, value) in expected_headers {
                let header = format!("{}: {}", name.to_lowercase(), value.to_lowercase());
                assert!(
                    req.contains(&header),
                    "missing header: {header}\nrequest: {req}"
                );
            }

            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                429 => "Too Many Requests",
                _ => "Test Status",
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                status,
                reason,
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("test server should write response");
        });

        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn test_validate_api_key_anthropic_429_returns_rate_limited() {
        let base = spawn_validation_server(
            "/v1/models",
            &[
                ("x-api-key", "test-key"),
                ("anthropic-version", "2023-06-01"),
            ],
            429,
            "{}",
        )
        .await;

        let result = validate_api_key("anthropic", "test-key", Some(&base))
            .await
            .expect("429 should be treated as a valid key");

        assert_eq!(result, KeyValidation::RateLimited);
    }

    #[tokio::test]
    async fn test_validate_api_key_openai_429_returns_rate_limited() {
        let base = spawn_validation_server(
            "/models",
            &[("authorization", "Bearer test-key")],
            429,
            "{}",
        )
        .await;

        let result = validate_api_key("openai", "test-key", Some(&base))
            .await
            .expect("429 should be treated as a valid key");

        assert_eq!(result, KeyValidation::RateLimited);
    }

    #[tokio::test]
    async fn test_validate_api_key_zhipu_429_returns_rate_limited() {
        let base = spawn_validation_server(
            "/models",
            &[("authorization", "Bearer test-key")],
            429,
            "{}",
        )
        .await;

        let result = validate_api_key("zhipu", "test-key", Some(&base))
            .await
            .expect("429 should be treated as a valid key");

        assert_eq!(result, KeyValidation::RateLimited);
    }

    #[tokio::test]
    async fn test_validate_api_key_openrouter_429_returns_rate_limited() {
        let base =
            spawn_validation_server("/key", &[("authorization", "Bearer test-key")], 429, "{}")
                .await;

        let result = validate_api_key("openrouter", "test-key", Some(&base))
            .await
            .expect("429 should be treated as a valid key");

        assert_eq!(result, KeyValidation::RateLimited);
    }

    #[tokio::test]
    async fn test_validate_api_key_anthropic_401_returns_error() {
        let base = spawn_validation_server(
            "/v1/models",
            &[
                ("x-api-key", "bad-key"),
                ("anthropic-version", "2023-06-01"),
            ],
            401,
            r#"{"error":{"message":"bad key"}}"#,
        )
        .await;

        let err = validate_api_key("anthropic", "bad-key", Some(&base))
            .await
            .expect_err("401 should remain an error");

        assert!(err.to_string().contains("Invalid API key"));
    }

    #[tokio::test]
    async fn test_validate_api_key_openai_401_returns_error() {
        let base = spawn_validation_server(
            "/models",
            &[("authorization", "Bearer bad-key")],
            401,
            r#"{"error":{"message":"bad key"}}"#,
        )
        .await;

        let err = validate_api_key("openai", "bad-key", Some(&base))
            .await
            .expect_err("401 should remain an error");

        assert!(err.to_string().contains("Invalid API key"));
    }

    #[tokio::test]
    async fn test_validate_api_key_zhipu_401_returns_error() {
        let base = spawn_validation_server(
            "/models",
            &[("authorization", "Bearer bad-key")],
            401,
            r#"{"error":{"message":"bad key"}}"#,
        )
        .await;

        let err = validate_api_key("zhipu", "bad-key", Some(&base))
            .await
            .expect_err("401 should remain an error");

        assert!(err.to_string().contains("Invalid API key"));
        assert!(err.to_string().contains("open.bigmodel.cn"));
    }

    #[tokio::test]
    async fn test_validate_api_key_openrouter_401_returns_error() {
        let base = spawn_validation_server(
            "/key",
            &[("authorization", "Bearer bad-key")],
            401,
            r#"{"error":{"message":"bad key"}}"#,
        )
        .await;

        let err = validate_api_key("openrouter", "bad-key", Some(&base))
            .await
            .expect_err("401 should remain an error");

        assert!(err.to_string().contains("Invalid API key"));
    }

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
    fn test_friendly_api_error_401_zhipu() {
        let msg = friendly_api_error("zhipu", 401, "");
        assert!(msg.contains("Invalid API key"));
        assert!(msg.contains("zhipu"));
        assert!(msg.contains("open.bigmodel.cn"));
    }

    #[test]
    fn test_friendly_api_error_401_openrouter() {
        let msg = friendly_api_error("openrouter", 401, "");
        assert!(msg.contains("Invalid API key"));
        assert!(msg.contains("openrouter"));
        assert!(msg.contains("openrouter.ai/settings/keys"));
    }

    #[test]
    fn test_friendly_api_error_402() {
        let msg = friendly_api_error("anthropic", 402, "");
        assert!(msg.contains("Billing issue"));
    }

    #[test]
    fn test_friendly_api_error_402_openrouter() {
        let msg = friendly_api_error("openrouter", 402, "");
        assert!(msg.contains("Insufficient OpenRouter credits"));
        assert!(msg.contains("openrouter.ai/settings/credits"));
    }

    #[test]
    fn test_friendly_api_error_429() {
        let msg = friendly_api_error("anthropic", 429, "");
        assert!(msg.contains("Rate limited"));
    }

    #[test]
    fn test_friendly_api_error_unknown_status() {
        let msg = friendly_api_error("openai", 500, "");
        assert!(msg.contains("HTTP 500"));
    }

    // Provider chain tests moved to zeptoclaw::kernel::provider::tests
}
