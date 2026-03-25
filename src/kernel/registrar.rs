//! Tool registration and filtering for ZeptoKernel.
//!
//! `ToolFilter` encapsulates the 5 filtering dimensions that gate tool registration.
//! `register_all_tools()` replaces the 590-line tool registration block in `cli/common.rs`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::bus::MessageBus;
use crate::config::templates::AgentTemplate;
use crate::config::{Config, MemoryBackend, ProjectBackend};
use crate::cron::CronService;
use crate::hands::HandManifest;
use crate::memory::longterm::LongTermMemory;
use crate::memory::traits::MemorySearcher;
use crate::runtime::ContainerRuntime;
use crate::security::{ShellAllowlistMode, ShellSecurityConfig};
use crate::tools::mcp::client::McpClient;
use crate::tools::mcp::discovery::{discover_mcp_servers, DiscoveredMcpServer, McpTransportType};
use crate::tools::mcp::wrapper::McpToolWrapper;
use crate::tools::ToolRegistry;

/// Build a [`ShellSecurityConfig`] from a template's `shell_allowlist` field.
///
/// When the template defines a non-`None` allowlist, the resulting config uses
/// [`ShellAllowlistMode::Strict`] so only the listed binaries may execute.
/// When `shell_allowlist` is `None` (or no template is provided), the default
/// blocklist-only config is returned.
pub fn build_shell_config(template: Option<&AgentTemplate>) -> ShellSecurityConfig {
    match template.and_then(|t| t.shell_allowlist.as_ref()) {
        None => ShellSecurityConfig::new(),
        Some(list) => ShellSecurityConfig::new().with_allowlist(
            list.iter().map(|s| s.as_str()).collect(),
            ShellAllowlistMode::Strict,
        ),
    }
}

/// Encapsulates the 5 filtering dimensions that gate tool registration.
///
/// Each dimension independently vetoes a tool name. A tool passes only if ALL
/// dimensions allow it:
///
/// 1. `allowed` — template + hand intersection (None = all allowed)
/// 2. `blocked` — template blocked_tools (explicit deny list)
/// 3. `profile` — tool_profiles config (None = all allowed)
/// 4. `denied` — tools.deny (startup guard degraded mode)
/// 5. `hand` — active hand required_tools (None = all allowed)
///
/// Replaces the inline closure at `cli/common.rs:576–595`.
pub struct ToolFilter {
    /// Intersection of template allowed_tools and hand required_tools.
    /// `None` means no restriction from these sources.
    allowed: Option<HashSet<String>>,
    /// Template blocked_tools — explicit deny list.
    blocked: HashSet<String>,
    /// Tool profile from config — `None` means no profile active.
    profile: Option<HashSet<String>>,
    /// Denied tools (startup guard degraded mode, etc.).
    denied: HashSet<String>,
}

impl ToolFilter {
    /// Build a filter from current config, optional template, and optional hand manifest.
    ///
    /// This mirrors the logic currently inline in `create_agent_with_template()`:
    /// - Template `allowed_tools` and hand `required_tools` are intersected
    /// - Template `blocked_tools` become the blocked set
    /// - Config `tool_profiles` resolved by profile name
    /// - Config `tools.deny` becomes the denied set
    pub fn from_config(
        config: &Config,
        template: Option<&AgentTemplate>,
        hand: Option<&HandManifest>,
    ) -> Self {
        let template_allowed = template
            .and_then(|tpl| tpl.allowed_tools.as_ref())
            .map(|names| {
                names
                    .iter()
                    .map(|n| n.to_ascii_lowercase())
                    .collect::<HashSet<_>>()
            });

        let hand_allowed = hand.map(|h| {
            h.required_tools
                .iter()
                .map(|n| n.to_ascii_lowercase())
                .collect::<HashSet<_>>()
        });

        let allowed = match (template_allowed, hand_allowed) {
            (Some(t), Some(h)) => Some(t.intersection(&h).cloned().collect()),
            (Some(t), None) => Some(t),
            (None, Some(h)) => Some(h),
            (None, None) => None,
        };

        let blocked = template
            .and_then(|tpl| tpl.blocked_tools.as_ref())
            .map(|names| {
                names
                    .iter()
                    .map(|n| n.to_ascii_lowercase())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();

        let profile = if let Some(ref profile_name) = config.agents.defaults.tool_profile {
            config.tool_profiles.get(profile_name).and_then(|tools| {
                tools
                    .as_ref()
                    .map(|names| names.iter().map(|n| n.to_ascii_lowercase()).collect())
            })
        } else {
            None
        };

        let denied = config
            .tools
            .deny
            .iter()
            .map(|n| n.to_ascii_lowercase())
            .collect();

        Self {
            allowed,
            blocked,
            profile,
            denied,
        }
    }

    /// Check if a tool name passes all 5 filter dimensions.
    ///
    /// All checks are case-insensitive (tool names are lowercased).
    pub fn is_enabled(&self, name: &str) -> bool {
        let key = name.to_ascii_lowercase();

        // Deny list (startup guard degraded mode, etc.)
        if self.denied.contains(&key) {
            return false;
        }

        // Profile filter (if active)
        if let Some(ref profile) = self.profile {
            if !profile.contains(&key) {
                return false;
            }
        }

        // Allowed filter (template + hand intersection)
        if let Some(ref allowed) = self.allowed {
            if !allowed.contains(&key) {
                return false;
            }
        }

        // Blocked filter (template blocked_tools)
        !self.blocked.contains(&key)
    }

    /// Returns `true` when the user has set an explicit tool profile or
    /// template `allowed_tools` list. Coding-only tools use this to honour
    /// explicit opt-in even when no "coding" template tag is active.
    pub fn has_explicit_profile(&self) -> bool {
        self.profile.is_some() || self.allowed.is_some()
    }

    /// Create a permissive filter that allows all tools.
    pub fn allow_all() -> Self {
        Self {
            allowed: None,
            blocked: HashSet::new(),
            profile: None,
            denied: HashSet::new(),
        }
    }
}

/// Shared dependencies needed by tool constructors during registration.
///
/// Bundles the subsystems that individual tools require, avoiding a long
/// parameter list on `register_all_tools()`.
pub struct ToolDeps {
    /// Container runtime for shell commands.
    pub runtime: Arc<dyn ContainerRuntime>,
    /// Message bus for proactive messaging.
    pub bus: Arc<MessageBus>,
    /// Cron service for scheduled tasks.
    pub cron_service: Arc<CronService>,
    /// Pluggable memory search backend.
    pub memory_searcher: Arc<dyn MemorySearcher>,
    /// Shared long-term memory (None when memory is disabled).
    pub shared_ltm: Option<Arc<tokio::sync::Mutex<LongTermMemory>>>,
    /// Active template (used to derive shell security config).
    pub template: Option<AgentTemplate>,
}

/// Register all kernel-owned tools into `registry`, gated by `filter`.
///
/// Returns MCP clients for graceful shutdown tracking.
///
/// Tools NOT registered here (require `AgentLoop`):
/// - `SpawnTool` — needs `Weak<AgentLoop>`
/// - `DelegateTool` — needs `Weak<AgentLoop>` + provider
pub async fn register_all_tools(
    registry: &mut ToolRegistry,
    config: &Config,
    filter: &ToolFilter,
    deps: &ToolDeps,
) -> anyhow::Result<(Vec<Arc<McpClient>>, HashSet<String>)> {
    use crate::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
    use crate::tools::shell::ShellTool;

    // Build shared shell security config from template (once, then cloned per tool)
    let shell_config = build_shell_config(deps.template.as_ref());

    // Track external (plugin/MCP/custom/composed) tool names for taint engine
    let mut external_tool_names: HashSet<String> = HashSet::new();

    // --- Group 1: Simple tools (no dependencies beyond config) ---
    if filter.is_enabled("echo") {
        registry.register(Box::new(crate::tools::EchoTool));
    }
    if filter.is_enabled("read_file") {
        registry.register(Box::new(ReadFileTool));
    }
    if filter.is_enabled("write_file") {
        registry.register(Box::new(WriteFileTool));
    }
    if filter.is_enabled("list_dir") {
        registry.register(Box::new(ListDirTool));
    }
    if filter.is_enabled("edit_file") {
        registry.register(Box::new(EditFileTool));
    }

    // --- Group 1b: Coding tools (default-off, enabled by "coding" template tag) ---
    // These are laptop/server workload tools that assume bash/filesystem access.
    // They ship as opt-in via the coder template, explicit tool_profiles, or
    // template allowed_tools. Without any of these, they stay off to keep the
    // core runtime portable for IoT/embedded use cases.
    let coding_profile_active = deps
        .template
        .as_ref()
        .map(|t| t.tags.iter().any(|tag| tag == "coding"))
        .unwrap_or(false);
    let has_explicit_profile = filter.has_explicit_profile();
    let coding_tools_on =
        coding_profile_active || has_explicit_profile || config.tools.coding_tools;
    if coding_tools_on && filter.is_enabled("grep") {
        registry.register(Box::new(crate::tools::grep::GrepTool));
        info!("Registered grep tool (coding profile)");
    }
    if coding_tools_on && filter.is_enabled("find") {
        registry.register(Box::new(crate::tools::find::FindTool));
        info!("Registered find tool (coding profile)");
    }

    // --- Group 2: Runtime-dependent ---
    if filter.is_enabled("shell") {
        registry.register(Box::new(ShellTool::with_security_and_runtime(
            shell_config.clone(),
            Arc::clone(&deps.runtime),
        )));
    }

    // --- Group 3: Git ---
    if filter.is_enabled("git") {
        if crate::tools::GitTool::is_available() {
            registry.register(Box::new(crate::tools::GitTool::with_security(
                shell_config.clone(),
            )));
            info!("Registered git tool");
        } else {
            tracing::debug!("git binary not found, skipping git tool");
        }
    }

    // --- Group 4: Web tools ---
    if filter.is_enabled("web_search") {
        let max = config.tools.web.search.max_results as usize;
        let search_cfg = &config.tools.web.search;

        let provider = search_cfg
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| {
                if search_cfg
                    .api_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .is_some()
                {
                    "searxng".to_string()
                } else if search_cfg
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .is_some()
                {
                    "brave".to_string()
                } else {
                    "ddg".to_string()
                }
            });

        match provider.as_str() {
            "searxng" => {
                let url = search_cfg
                    .api_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("SearXNG provider requires tools.web.search.api_url")
                    })?;
                registry.register(Box::new(crate::tools::SearxngSearchTool::with_max_results(
                    url, max,
                )?));
                info!("Registered web_search tool (SearXNG)");
            }
            "brave" => {
                let key = search_cfg
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Brave provider requires tools.web.search.api_key")
                    })?;
                registry.register(Box::new(crate::tools::WebSearchTool::with_max_results(
                    key, max,
                )));
                info!("Registered web_search tool (Brave)");
            }
            "ddg" => {
                registry.register(Box::new(crate::tools::DdgSearchTool::with_max_results(max)));
                info!("Registered web_search tool (DuckDuckGo fallback)");
            }
            other => {
                return Err(anyhow::anyhow!(
                    "Invalid tools.web.search.provider '{}'. Expected one of: brave, searxng, ddg",
                    other
                ));
            }
        }
    }
    // Browser tool (opt-in): when enabled, replaces web_fetch
    if filter.is_enabled("browser") && config.tools.browser.enabled {
        registry.register(Box::new(crate::tools::BrowserTool::new(
            &config.tools.browser,
        )));
        info!(
            "Registered browser tool (engine: {})",
            config.tools.browser.engine
        );
    }
    if filter.is_enabled("web_fetch")
        && !(config.tools.browser.enabled && filter.is_enabled("browser"))
    {
        registry.register(Box::new(crate::tools::WebFetchTool::new()));
        info!("Registered web_fetch tool");
    }

    // --- Group 5: HTTP request ---
    if filter.is_enabled("http_request") {
        if let Some(http_cfg) = &config.tools.http_request {
            if !http_cfg.allowed_domains.is_empty() {
                registry.register(Box::new(crate::tools::HttpRequestTool::new(
                    http_cfg.allowed_domains.clone(),
                    http_cfg.timeout_secs,
                    http_cfg.max_response_bytes,
                )));
                info!("Registered http_request tool");
            }
        }
    }

    // --- Group 6: Document tools ---
    if filter.is_enabled("pdf_read") {
        let workspace_str = config.workspace_path().to_string_lossy().into_owned();
        registry.register(Box::new(crate::tools::PdfReadTool::new(workspace_str)));
        info!("Registered pdf_read tool");
    }
    if filter.is_enabled("docx_read") {
        let workspace_str = config.workspace_path().to_string_lossy().into_owned();
        registry.register(Box::new(crate::tools::DocxReadTool::new(workspace_str)));
        info!("Registered docx_read tool");
    }

    // --- Group 7: Channel/messaging tools ---
    if filter.is_enabled("message") {
        registry.register(Box::new(crate::tools::MessageTool::new(Arc::clone(
            &deps.bus,
        ))));
        info!("Registered message tool");
    }
    if filter.is_enabled("whatsapp_send") {
        if let (Some(phone_number_id), Some(access_token)) = (
            config.tools.whatsapp.phone_number_id.as_deref(),
            config.tools.whatsapp.access_token.as_deref(),
        ) {
            if !phone_number_id.trim().is_empty() && !access_token.trim().is_empty() {
                registry.register(Box::new(crate::tools::WhatsAppTool::with_default_language(
                    phone_number_id.trim(),
                    access_token.trim(),
                    config.tools.whatsapp.default_language.trim(),
                )));
                info!("Registered whatsapp_send tool");
            }
        }
    }

    // --- Group 8: Google tools ---
    if filter.is_enabled("google_sheets") {
        if let Some(access_token) = config.tools.google_sheets.access_token.as_deref() {
            let token = access_token.trim();
            if !token.is_empty() {
                registry.register(Box::new(crate::tools::GoogleSheetsTool::new(token)));
                info!("Registered google_sheets tool");
            }
        } else if let Some(encoded) = config.tools.google_sheets.service_account_base64.as_deref() {
            match crate::tools::GoogleSheetsTool::from_service_account(encoded.trim()) {
                Ok(tool) => {
                    registry.register(Box::new(tool));
                    info!("Registered google_sheets tool from base64 payload");
                }
                Err(e) => warn!("Failed to initialize google_sheets tool: {}", e),
            }
        }
    }

    // NOTE: Google Workspace tool (feature = "google") is NOT registered here.
    // It requires async OAuth token resolution that depends on stored credentials,
    // which is handled in `cli/common.rs` after kernel boot. See the
    // `resolve_google_token()` + `GoogleTool::new()` block in `create_agent_with_template()`.
    #[cfg(feature = "google")]
    if filter.is_enabled("google") {
        info!("Google Workspace tool deferred — registered in create_agent_with_template() after OAuth resolution");
    }

    // --- Group 9: Memory tools ---
    if !matches!(config.memory.backend, MemoryBackend::Disabled) {
        if filter.is_enabled("memory_search") {
            registry.register(Box::new(crate::tools::MemorySearchTool::with_searcher(
                config.memory.clone(),
                Arc::clone(&deps.memory_searcher),
            )));
        }
        if filter.is_enabled("memory_get") {
            registry.register(Box::new(crate::tools::MemoryGetTool::new(
                config.memory.clone(),
            )));
        }
        if filter.is_enabled("longterm_memory") {
            if let Some(ref ltm) = deps.shared_ltm {
                let tool =
                    crate::tools::longterm_memory::LongTermMemoryTool::with_memory(ltm.clone());
                registry.register(Box::new(tool));
                info!(
                    "Registered longterm_memory tool (searcher: {})",
                    deps.memory_searcher.name()
                );
            } else {
                warn!("longterm_memory tool enabled but LTM failed to initialize");
            }
        }
        info!(
            "Registered memory tools (backend: {})",
            deps.memory_searcher.name()
        );
    } else {
        info!("Memory tools are disabled");
    }

    // --- Group 10: Interaction tools ---
    if filter.is_enabled("ask_clarification") {
        registry.register(Box::new(crate::tools::clarification::AskClarificationTool));
    }

    // --- Group 11: Scheduling/cron ---
    if filter.is_enabled("cron") {
        registry.register(Box::new(crate::tools::cron::CronTool::new(Arc::clone(
            &deps.cron_service,
        ))));
    }
    if filter.is_enabled("r8r") {
        registry.register(Box::new(crate::tools::R8rTool::default()));
    }

    // --- Group 11: Project management ---
    if filter.is_enabled("project") {
        let project_config = config.project.clone();
        let has_token = match project_config.backend {
            ProjectBackend::Github => project_config
                .github_token
                .as_deref()
                .filter(|t| !t.is_empty())
                .is_some(),
            ProjectBackend::Jira => project_config
                .jira_token
                .as_deref()
                .filter(|t| !t.is_empty())
                .is_some(),
            ProjectBackend::Linear => project_config
                .linear_api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .is_some(),
        };
        if has_token {
            registry.register(Box::new(crate::tools::ProjectTool::new(project_config)));
            info!(
                "Registered project tool ({:?} backend)",
                config.project.backend
            );
        }
    }

    // --- Group 12: Transcription ---
    if config.tools.transcribe.enabled {
        if let Some(api_key) = &config.tools.transcribe.groq_api_key {
            if filter.is_enabled("transcribe") {
                match crate::tools::TranscribeTool::new(api_key, &config.tools.transcribe.model) {
                    Ok(tool) => {
                        registry.register(Box::new(tool));
                        info!(
                            "Registered transcribe tool (model: {})",
                            config.tools.transcribe.model
                        );
                    }
                    Err(e) => warn!("Failed to initialize transcribe tool: {}", e),
                }
            }
        }
    }

    // --- Group 13: Reminders ---
    if filter.is_enabled("reminder") {
        match crate::tools::reminder::ReminderTool::new(Some(Arc::clone(&deps.cron_service))) {
            Ok(tool) => {
                registry.register(Box::new(tool));
                info!("Registered reminder tool");
            }
            Err(e) => warn!("Failed to initialize reminder tool: {}", e),
        }
    }

    // --- Group 14: ClawHub skills marketplace ---
    if config.tools.skills.enabled && config.tools.skills.clawhub.enabled {
        use crate::skills::registry::{ClawHubRegistry, SearchCache};
        use std::time::Duration;

        let cache = Arc::new(SearchCache::new(
            config.tools.skills.search_cache.max_size,
            Duration::from_secs(config.tools.skills.search_cache.ttl_seconds),
        ));
        let clawhub = Arc::new(ClawHubRegistry::with_allowed_hosts(
            &config.tools.skills.clawhub.base_url,
            config.tools.skills.clawhub.auth_token.clone(),
            cache,
            config.tools.skills.clawhub.allowed_hosts.clone(),
        ));
        if filter.is_enabled("find_skills") {
            registry.register(Box::new(crate::tools::FindSkillsTool::new(Arc::clone(
                &clawhub,
            ))));
            info!("Registered find_skills tool");
        }
        if filter.is_enabled("install_skill") {
            let skills_dir = config
                .skills
                .workspace_dir
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| Config::dir().join("skills").to_string_lossy().into_owned());
            registry.register(Box::new(crate::tools::InstallSkillTool::new(
                Arc::clone(&clawhub),
                skills_dir,
            )));
            info!("Registered install_skill tool");
        }
    }

    // --- Group 15: Android (feature-gated) ---
    #[cfg(feature = "android")]
    if filter.is_enabled("android") {
        registry.register(Box::new(crate::tools::android::AndroidTool::new()));
        info!("Registered android tool");
    }

    // --- Group 16: Plugin tools ---
    if config.plugins.enabled {
        let plugin_dirs: Vec<PathBuf> = config
            .plugins
            .plugin_dirs
            .iter()
            .map(|d| crate::config::expand_home(d))
            .collect();
        match crate::plugins::discover_plugins(&plugin_dirs) {
            Ok(plugins) => {
                for plugin in plugins {
                    if !config.plugins.is_plugin_permitted(plugin.name()) {
                        info!(plugin = %plugin.name(), "Plugin blocked by config");
                        continue;
                    }
                    for tool_def in &plugin.manifest.tools {
                        if !filter.is_enabled(&tool_def.name) {
                            continue;
                        }
                        if plugin.manifest.is_binary() {
                            if let Some(ref bin_cfg) = plugin.manifest.binary {
                                match crate::plugins::validate_binary_path(&plugin.path, bin_cfg) {
                                    Ok(bin_path) => {
                                        let timeout = bin_cfg
                                            .timeout_secs
                                            .unwrap_or_else(|| tool_def.effective_timeout());
                                        registry.register(Box::new(
                                            crate::tools::binary_plugin::BinaryPluginTool::new(
                                                tool_def.clone(),
                                                plugin.name(),
                                                bin_path,
                                                timeout,
                                            ),
                                        ));
                                        external_tool_names.insert(tool_def.name.clone());
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
                            registry.register(Box::new(
                                crate::tools::plugin::PluginTool::with_security(
                                    tool_def.clone(),
                                    plugin.name(),
                                    shell_config.clone(),
                                ),
                            ));
                            external_tool_names.insert(tool_def.name.clone());
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

    // --- Group 17: Composed tools ---
    if filter.is_enabled("create_tool") {
        registry.register(Box::new(crate::tools::composed::CreateToolTool::new()));
    }
    for tool in crate::tools::composed::load_composed_tools() {
        let name = tool.name().to_string();
        if !filter.is_enabled(&name) {
            continue;
        }
        registry.register(tool);
        external_tool_names.insert(name.clone());
        info!(tool = %name, "Registered composed tool");
    }

    // --- Group 18: Custom CLI-defined tools ---
    let tool_warnings = crate::config::validate::validate_custom_tools(config);
    for w in &tool_warnings {
        warn!("Custom tool config: {}", w);
    }
    for tool_def in &config.custom_tools {
        if !filter.is_enabled(&tool_def.name) {
            continue;
        }
        if tool_def.command.trim().is_empty() {
            warn!(tool = %tool_def.name, "Skipping custom tool with empty command");
            continue;
        }
        let tool =
            crate::tools::custom::CustomTool::with_security(tool_def.clone(), shell_config.clone());
        registry.register(Box::new(tool));
        external_tool_names.insert(tool_def.name.clone());
        info!(tool = %tool_def.name, "Registered custom CLI tool");
    }

    // --- Group 19: MCP server tools (async) ---
    let mut mcp_clients: Vec<Arc<McpClient>> = Vec::new();
    {
        let workspace = config.workspace_path();
        let mut all_servers = discover_mcp_servers(Some(&workspace));

        for server_cfg in &config.mcp.servers {
            let transport = if let Some(url) = server_cfg.url.clone() {
                McpTransportType::Http { url }
            } else if let Some(command) = server_cfg.command.clone() {
                McpTransportType::Stdio {
                    command,
                    args: server_cfg.args.clone().unwrap_or_default(),
                    env: server_cfg.env.clone().unwrap_or_default(),
                }
            } else {
                warn!(
                    server = %server_cfg.name,
                    "MCP server config has neither url nor command, skipping"
                );
                continue;
            };

            all_servers.push(DiscoveredMcpServer {
                name: server_cfg.name.clone(),
                transport,
                source: "config".to_string(),
            });
        }

        for server in &all_servers {
            let timeout = config
                .mcp
                .servers
                .iter()
                .find(|cfg| cfg.name == server.name)
                .map_or(30, |cfg| cfg.timeout_secs);

            let client_result: Result<McpClient, String> = match &server.transport {
                McpTransportType::Http { url } => {
                    Ok(McpClient::new_http(&server.name, url, timeout))
                }
                McpTransportType::Stdio { command, args, env } => {
                    match McpClient::new_stdio(&server.name, command, args, env, timeout).await {
                        Ok(c) => Ok(c),
                        Err(e) => {
                            warn!(
                                server = %server.name,
                                error = %e,
                                "Failed to spawn stdio MCP server, skipping"
                            );
                            continue;
                        }
                    }
                }
            };

            let client = match client_result {
                Ok(c) => Arc::new(c),
                Err(e) => {
                    warn!(server = %server.name, error = %e, "Failed to create MCP client");
                    continue;
                }
            };

            if let Err(e) = client.initialize().await {
                warn!(
                    server = %server.name,
                    error = %e,
                    "MCP server initialize failed, skipping"
                );
                continue;
            }

            mcp_clients.push(Arc::clone(&client));

            match client.list_tools().await {
                Ok(tools) => {
                    let mut registered_count = 0usize;
                    for tool in tools {
                        let prefixed_name = format!("{}_{}", server.name, tool.name);
                        if !filter.is_enabled(&prefixed_name) {
                            continue;
                        }
                        let prefixed = format!("{}_{}", server.name, tool.name);
                        registry.register(Box::new(McpToolWrapper::new(
                            &server.name,
                            &tool.name,
                            tool.description.as_deref().unwrap_or(""),
                            tool.input_schema.clone(),
                            Arc::clone(&client),
                        )));
                        external_tool_names.insert(prefixed);
                        registered_count += 1;
                    }
                    info!(
                        server = %server.name,
                        transport = client.transport_type(),
                        tools = registered_count,
                        source = %server.source,
                        "Registered MCP server tools"
                    );
                }
                Err(e) => {
                    warn!(
                        server = %server.name,
                        error = %e,
                        "Failed to list MCP tools, skipping"
                    );
                }
            }
        }
    }

    info!(
        "Registered {} tools ({} external)",
        registry.len(),
        external_tool_names.len()
    );

    Ok((mcp_clients, external_tool_names))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::templates::AgentTemplate;
    use crate::config::Config;
    use crate::hands::{HandGuardrails, HandManifest};

    // -----------------------------------------------------------
    // ToolFilter::is_enabled — individual dimension tests
    // -----------------------------------------------------------

    #[test]
    fn test_allow_all_passes_everything() {
        let filter = ToolFilter::allow_all();
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("shell"));
        assert!(filter.is_enabled("web_search"));
        assert!(filter.is_enabled("ANYTHING_AT_ALL"));
    }

    #[test]
    fn test_denied_blocks_tool() {
        let filter = ToolFilter {
            allowed: None,
            blocked: HashSet::new(),
            profile: None,
            denied: ["shell".to_string()].into_iter().collect(),
        };
        assert!(!filter.is_enabled("shell"));
        assert!(!filter.is_enabled("Shell")); // case-insensitive
        assert!(filter.is_enabled("echo")); // other tools pass
    }

    #[test]
    fn test_blocked_blocks_tool() {
        let filter = ToolFilter {
            allowed: None,
            blocked: ["web_search".to_string()].into_iter().collect(),
            profile: None,
            denied: HashSet::new(),
        };
        assert!(!filter.is_enabled("web_search"));
        assert!(!filter.is_enabled("Web_Search")); // case-insensitive
        assert!(filter.is_enabled("echo"));
    }

    #[test]
    fn test_allowed_restricts_to_set() {
        let filter = ToolFilter {
            allowed: Some(
                ["echo".to_string(), "shell".to_string()]
                    .into_iter()
                    .collect(),
            ),
            blocked: HashSet::new(),
            profile: None,
            denied: HashSet::new(),
        };
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("shell"));
        assert!(!filter.is_enabled("web_search")); // not in allowed set
    }

    #[test]
    fn test_profile_restricts_to_set() {
        let filter = ToolFilter {
            allowed: None,
            blocked: HashSet::new(),
            profile: Some(
                ["echo".to_string(), "read_file".to_string()]
                    .into_iter()
                    .collect(),
            ),
            denied: HashSet::new(),
        };
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("read_file"));
        assert!(!filter.is_enabled("shell")); // not in profile
    }

    #[test]
    fn test_case_insensitivity_query_side() {
        // from_config always lowercases stored values, so the filter
        // stores lowercase. Case insensitivity is on the QUERY side —
        // is_enabled("Echo") should match stored "echo".
        let filter = ToolFilter {
            allowed: Some(["echo".to_string()].into_iter().collect()),
            blocked: ["shell".to_string()].into_iter().collect(),
            profile: None,
            denied: ["git".to_string()].into_iter().collect(),
        };
        // Queried with mixed case → lowercased before lookup
        assert!(filter.is_enabled("Echo"));
        assert!(filter.is_enabled("ECHO"));
        assert!(!filter.is_enabled("Shell"));
        assert!(!filter.is_enabled("SHELL"));
        assert!(!filter.is_enabled("Git"));
        assert!(!filter.is_enabled("GIT"));
    }

    // -----------------------------------------------------------
    // ToolFilter::is_enabled — combined dimension tests
    // -----------------------------------------------------------

    #[test]
    fn test_denied_takes_priority_over_allowed() {
        let filter = ToolFilter {
            allowed: Some(["shell".to_string()].into_iter().collect()),
            blocked: HashSet::new(),
            profile: None,
            denied: ["shell".to_string()].into_iter().collect(),
        };
        // shell is in allowed set BUT also in denied — denied wins
        assert!(!filter.is_enabled("shell"));
    }

    #[test]
    fn test_blocked_takes_priority_over_allowed() {
        let filter = ToolFilter {
            allowed: Some(["shell".to_string()].into_iter().collect()),
            blocked: ["shell".to_string()].into_iter().collect(),
            profile: None,
            denied: HashSet::new(),
        };
        // shell is in allowed set BUT also in blocked — blocked wins
        assert!(!filter.is_enabled("shell"));
    }

    #[test]
    fn test_profile_and_allowed_intersection() {
        let filter = ToolFilter {
            allowed: Some(
                ["echo".to_string(), "shell".to_string()]
                    .into_iter()
                    .collect(),
            ),
            blocked: HashSet::new(),
            profile: Some(
                ["echo".to_string(), "read_file".to_string()]
                    .into_iter()
                    .collect(),
            ),
            denied: HashSet::new(),
        };
        // echo is in both → passes
        assert!(filter.is_enabled("echo"));
        // shell is in allowed but not profile → fails
        assert!(!filter.is_enabled("shell"));
        // read_file is in profile but not allowed → fails
        assert!(!filter.is_enabled("read_file"));
    }

    #[test]
    fn test_all_five_dimensions_combined() {
        let filter = ToolFilter {
            allowed: Some(
                [
                    "echo".to_string(),
                    "shell".to_string(),
                    "read_file".to_string(),
                    "web_search".to_string(),
                ]
                .into_iter()
                .collect(),
            ),
            blocked: ["web_search".to_string()].into_iter().collect(),
            profile: Some(
                [
                    "echo".to_string(),
                    "shell".to_string(),
                    "read_file".to_string(),
                    "web_search".to_string(),
                ]
                .into_iter()
                .collect(),
            ),
            denied: ["shell".to_string()].into_iter().collect(),
        };
        // echo: in allowed ✓, not blocked ✓, in profile ✓, not denied ✓ → passes
        assert!(filter.is_enabled("echo"));
        // shell: in allowed ✓, not blocked ✓, in profile ✓, BUT denied → fails
        assert!(!filter.is_enabled("shell"));
        // web_search: in allowed ✓, BUT blocked → fails
        assert!(!filter.is_enabled("web_search"));
        // read_file: in allowed ✓, not blocked ✓, in profile ✓, not denied ✓ → passes
        assert!(filter.is_enabled("read_file"));
        // git: not in allowed → fails
        assert!(!filter.is_enabled("git"));
    }

    // -----------------------------------------------------------
    // ToolFilter::from_config — construction tests
    // -----------------------------------------------------------

    #[test]
    fn test_from_config_default_allows_all() {
        let config = Config::default();
        let filter = ToolFilter::from_config(&config, None, None);
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("shell"));
        assert!(filter.is_enabled("web_search"));
        assert!(filter.is_enabled("any_tool_name"));
    }

    #[test]
    fn test_from_config_with_deny_list() {
        let mut config = Config::default();
        config.tools.deny = vec!["shell".to_string(), "git".to_string()];
        let filter = ToolFilter::from_config(&config, None, None);
        assert!(!filter.is_enabled("shell"));
        assert!(!filter.is_enabled("git"));
        assert!(filter.is_enabled("echo"));
    }

    #[test]
    fn test_from_config_with_template_allowed() {
        let config = Config::default();
        let template = AgentTemplate {
            name: "test".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Some(vec!["echo".to_string(), "read_file".to_string()]),
            blocked_tools: None,
            max_tool_iterations: None,
            tags: vec![],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };
        let filter = ToolFilter::from_config(&config, Some(&template), None);
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("read_file"));
        assert!(!filter.is_enabled("shell"));
    }

    #[test]
    fn test_from_config_with_template_blocked() {
        let config = Config::default();
        let template = AgentTemplate {
            name: "test".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: None,
            blocked_tools: Some(vec!["shell".to_string()]),
            max_tool_iterations: None,
            tags: vec![],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };
        let filter = ToolFilter::from_config(&config, Some(&template), None);
        assert!(!filter.is_enabled("shell"));
        assert!(filter.is_enabled("echo"));
    }

    #[test]
    fn test_from_config_with_hand_required_tools() {
        let config = Config::default();
        let hand = HandManifest {
            name: "test".to_string(),
            description: "test hand".to_string(),
            required_tools: vec!["echo".to_string(), "git".to_string()],
            system_prompt: String::new(),
            guardrails: HandGuardrails::default(),
            settings: HashMap::new(),
        };
        let filter = ToolFilter::from_config(&config, None, Some(&hand));
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("git"));
        assert!(!filter.is_enabled("shell")); // not in hand required_tools
    }

    #[test]
    fn test_from_config_template_and_hand_intersect() {
        let config = Config::default();
        let template = AgentTemplate {
            name: "test".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Some(vec![
                "echo".to_string(),
                "shell".to_string(),
                "read_file".to_string(),
            ]),
            blocked_tools: None,
            max_tool_iterations: None,
            tags: vec![],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };
        let hand = HandManifest {
            name: "test".to_string(),
            description: "test hand".to_string(),
            required_tools: vec!["echo".to_string(), "git".to_string()],
            system_prompt: String::new(),
            guardrails: HandGuardrails::default(),
            settings: HashMap::new(),
        };
        let filter = ToolFilter::from_config(&config, Some(&template), Some(&hand));
        // Only "echo" is in both template allowed AND hand required
        assert!(filter.is_enabled("echo"));
        assert!(!filter.is_enabled("shell")); // template only
        assert!(!filter.is_enabled("git")); // hand only
        assert!(!filter.is_enabled("read_file")); // template only
    }

    #[test]
    fn test_from_config_with_tool_profile() {
        let mut config = Config::default();
        config.agents.defaults.tool_profile = Some("minimal".to_string());
        config.tool_profiles.insert(
            "minimal".to_string(),
            Some(vec!["echo".to_string(), "read_file".to_string()]),
        );
        let filter = ToolFilter::from_config(&config, None, None);
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("read_file"));
        assert!(!filter.is_enabled("shell"));
    }

    #[test]
    fn test_from_config_unknown_profile_allows_all() {
        let mut config = Config::default();
        config.agents.defaults.tool_profile = Some("nonexistent".to_string());
        // Profile name set but not in tool_profiles map
        let filter = ToolFilter::from_config(&config, None, None);
        // Unknown profile → profile stays None → all tools allowed
        assert!(filter.is_enabled("echo"));
        assert!(filter.is_enabled("shell"));
    }

    // -----------------------------------------------------------
    // Regression: exact parity with inline closure
    // -----------------------------------------------------------

    // -----------------------------------------------------------
    // build_shell_config — template shell_allowlist tests
    // -----------------------------------------------------------

    #[test]
    fn test_build_shell_config_with_allowlist() {
        let tpl = AgentTemplate {
            name: "restricted".to_string(),
            description: "test".to_string(),
            system_prompt: "test".to_string(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: None,
            blocked_tools: None,
            max_tool_iterations: None,
            shell_allowlist: Some(vec!["git".to_string(), "cargo".to_string()]),
            max_token_budget: None,
            max_tool_calls: None,
            tags: vec![],
        };
        let config = build_shell_config(Some(&tpl));
        assert!(config.validate_command("git status").is_ok());
        assert!(config.validate_command("cargo build").is_ok());
        assert!(config.validate_command("curl https://evil.com").is_err());
    }

    #[test]
    fn test_build_shell_config_none_uses_default() {
        let tpl = AgentTemplate {
            name: "open".to_string(),
            description: "test".to_string(),
            system_prompt: "test".to_string(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: None,
            blocked_tools: None,
            max_tool_iterations: None,
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
            tags: vec![],
        };
        let config = build_shell_config(Some(&tpl));
        assert!(config.validate_command("curl https://example.com").is_ok());
    }

    #[test]
    fn test_build_shell_config_empty_denies_all() {
        let tpl = AgentTemplate {
            name: "locked".to_string(),
            description: "test".to_string(),
            system_prompt: "test".to_string(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: None,
            blocked_tools: None,
            max_tool_iterations: None,
            shell_allowlist: Some(vec![]),
            max_token_budget: None,
            max_tool_calls: None,
            tags: vec![],
        };
        let config = build_shell_config(Some(&tpl));
        assert!(config.validate_command("ls").is_err());
        assert!(config.validate_command("git status").is_err());
    }

    #[test]
    fn test_build_shell_config_no_template() {
        let config = build_shell_config(None);
        assert!(config.validate_command("curl https://example.com").is_ok());
    }

    // -----------------------------------------------------------
    // Regression: exact parity with inline closure
    // -----------------------------------------------------------

    #[test]
    fn test_filter_parity_with_inline_closure() {
        // Reproduce the exact scenario from common.rs:576-595
        // Template allows: shell, read_file
        // Template blocks: web_search
        // Config deny: git
        // No profile, no hand
        let mut config = Config::default();
        config.tools.deny = vec!["git".to_string()];

        let template = AgentTemplate {
            name: "test".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Some(vec!["shell".to_string(), "read_file".to_string()]),
            blocked_tools: Some(vec!["web_search".to_string()]),
            max_tool_iterations: None,
            tags: vec![],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };

        let filter = ToolFilter::from_config(&config, Some(&template), None);

        // shell: allowed ✓, not blocked ✓, not denied ✓ → true
        assert!(filter.is_enabled("shell"));
        // read_file: allowed ✓, not blocked ✓, not denied ✓ → true
        assert!(filter.is_enabled("read_file"));
        // web_search: blocked → false
        assert!(!filter.is_enabled("web_search"));
        // git: denied → false
        assert!(!filter.is_enabled("git"));
        // echo: not in allowed → false
        assert!(!filter.is_enabled("echo"));
    }
}
