//! Configuration type definitions for ZeptoClaw
//!
//! This module defines all configuration structs used throughout the framework.
//! All types implement serde traits for JSON serialization and have sensible defaults.

use serde::{Deserialize, Serialize};

/// Main configuration struct for ZeptoClaw
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    /// Agent configuration (models, tokens, iterations)
    pub agents: AgentConfig,
    /// Channel configurations (Telegram, Discord, Slack, etc.)
    pub channels: ChannelsConfig,
    /// LLM provider configurations (Claude, OpenAI, OpenRouter, etc.)
    pub providers: ProvidersConfig,
    /// Gateway server configuration
    pub gateway: GatewayConfig,
    /// Tools configuration
    pub tools: ToolsConfig,
    /// Memory configuration
    pub memory: MemoryConfig,
    /// Heartbeat background task configuration
    pub heartbeat: HeartbeatConfig,
    /// Skills system configuration
    pub skills: SkillsConfig,
    /// Runtime configuration for container isolation
    pub runtime: RuntimeConfig,
    /// Containerized agent configuration
    pub container_agent: ContainerAgentConfig,
    /// Swarm / multi-agent delegation configuration
    pub swarm: SwarmConfig,
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AgentConfig {
    /// Default agent settings
    pub defaults: AgentDefaults,
}

/// Default agent settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDefaults {
    /// Workspace directory path
    pub workspace: String,
    /// Default model to use
    pub model: String,
    /// Maximum tokens for responses
    pub max_tokens: u32,
    /// Temperature for generation
    pub temperature: f32,
    /// Maximum tool iterations per turn
    pub max_tool_iterations: u32,
    /// Maximum wall-clock time (seconds) for a single agent run.
    pub agent_timeout_secs: u64,
    /// How to handle messages arriving during an active run.
    pub message_queue_mode: MessageQueueMode,
    /// Whether to stream the final LLM response token-by-token in CLI mode.
    pub streaming: bool,
}

/// Default model compile-time configuration.
/// Set `ZEPTOCLAW_DEFAULT_MODEL` at compile time to override.
const COMPILE_TIME_DEFAULT_MODEL: &str = match option_env!("ZEPTOCLAW_DEFAULT_MODEL") {
    Some(v) => v,
    None => "claude-sonnet-4-5-20250929",
};

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.zeptoclaw/workspace".to_string(),
            model: COMPILE_TIME_DEFAULT_MODEL.to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            max_tool_iterations: 20,
            agent_timeout_secs: 300,
            message_queue_mode: MessageQueueMode::default(),
            streaming: false,
        }
    }
}

/// How to handle messages that arrive while an agent run is active.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageQueueMode {
    /// Buffer messages, concatenate into one when current run finishes.
    #[default]
    Collect,
    /// Buffer messages, replay each as a separate run after current finishes.
    Followup,
}

// ============================================================================
// Channel Configurations
// ============================================================================

/// All channel configurations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChannelsConfig {
    /// Telegram bot configuration
    pub telegram: Option<TelegramConfig>,
    /// Discord bot configuration
    pub discord: Option<DiscordConfig>,
    /// Slack bot configuration
    pub slack: Option<SlackConfig>,
    /// WhatsApp bridge configuration
    pub whatsapp: Option<WhatsAppConfig>,
    /// Feishu (Lark) configuration
    pub feishu: Option<FeishuConfig>,
    /// MaixCam configuration
    pub maixcam: Option<MaixCamConfig>,
    /// QQ configuration
    pub qq: Option<QQConfig>,
    /// DingTalk configuration
    pub dingtalk: Option<DingTalkConfig>,
}

/// Telegram channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Bot token from BotFather
    pub token: String,
    /// Allowlist of user IDs/usernames (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// Discord channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Bot token from Discord Developer Portal
    pub token: String,
    /// Allowlist of user IDs (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// Slack channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Bot token (xoxb-...)
    pub bot_token: String,
    /// App-level token (xapp-...)
    pub app_token: String,
    /// Allowlist of user IDs (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// WhatsApp channel configuration (via bridge)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// WebSocket bridge URL
    #[serde(default = "default_whatsapp_bridge_url")]
    pub bridge_url: String,
    /// Allowlist of phone numbers (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

fn default_whatsapp_bridge_url() -> String {
    "ws://localhost:3001".to_string()
}

impl Default for WhatsAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_url: default_whatsapp_bridge_url(),
            allow_from: Vec::new(),
        }
    }
}

/// Feishu (Lark) channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// App ID
    pub app_id: String,
    /// App Secret
    pub app_secret: String,
    /// Encrypt Key for event subscription
    #[serde(default)]
    pub encrypt_key: String,
    /// Verification Token
    #[serde(default)]
    pub verification_token: String,
    /// Allowlist of user IDs (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// MaixCam channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaixCamConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Host to bind to
    #[serde(default = "default_maixcam_host")]
    pub host: String,
    /// Port to listen on
    #[serde(default = "default_maixcam_port")]
    pub port: u16,
    /// Allowlist of device IDs (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

fn default_maixcam_host() -> String {
    "0.0.0.0".to_string()
}

fn default_maixcam_port() -> u16 {
    18790
}

impl Default for MaixCamConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_maixcam_host(),
            port: default_maixcam_port(),
            allow_from: Vec::new(),
        }
    }
}

/// QQ channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QQConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// App ID
    pub app_id: String,
    /// App Secret
    pub app_secret: String,
    /// Allowlist of QQ numbers (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

/// DingTalk channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingTalkConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Client ID
    pub client_id: String,
    /// Client Secret
    pub client_secret: String,
    /// Allowlist of user IDs (empty = allow all)
    #[serde(default)]
    pub allow_from: Vec<String>,
}

// ============================================================================
// Provider Configurations
// ============================================================================

/// All LLM provider configurations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProvidersConfig {
    /// Anthropic Claude configuration
    pub anthropic: Option<ProviderConfig>,
    /// OpenAI configuration
    pub openai: Option<ProviderConfig>,
    /// OpenRouter configuration
    pub openrouter: Option<ProviderConfig>,
    /// Groq configuration
    pub groq: Option<ProviderConfig>,
    /// Zhipu (GLM) configuration
    pub zhipu: Option<ProviderConfig>,
    /// VLLM configuration
    pub vllm: Option<ProviderConfig>,
    /// Google Gemini configuration
    pub gemini: Option<ProviderConfig>,
}

/// Generic provider configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// API key for authentication
    #[serde(default)]
    pub api_key: Option<String>,
    /// Custom API base URL
    #[serde(default)]
    pub api_base: Option<String>,
    /// Authentication method (e.g., "oauth", "api_key")
    #[serde(default)]
    pub auth_method: Option<String>,
}

// ============================================================================
// Gateway Configuration
// ============================================================================

/// Gateway server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// Host to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
        }
    }
}

// ============================================================================
// Tools Configuration
// ============================================================================

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsConfig {
    /// Web tools configuration
    pub web: WebToolsConfig,
    /// WhatsApp Cloud API tool configuration
    pub whatsapp: WhatsAppToolConfig,
    /// Google Sheets tool configuration
    pub google_sheets: GoogleSheetsToolConfig,
}

/// Web tools configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WebToolsConfig {
    /// Web search configuration
    pub search: WebSearchConfig,
}

/// Web search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchConfig {
    /// API key for search service
    #[serde(default)]
    pub api_key: Option<String>,
    /// Maximum search results to return
    pub max_results: u32,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            max_results: 5,
        }
    }
}

/// WhatsApp Cloud API tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WhatsAppToolConfig {
    /// WhatsApp Business account ID (optional, informational)
    #[serde(default)]
    pub business_account_id: Option<String>,
    /// Phone number ID used in Cloud API endpoint path
    #[serde(default)]
    pub phone_number_id: Option<String>,
    /// Permanent access token for Cloud API
    #[serde(default)]
    pub access_token: Option<String>,
    /// Optional webhook verify token
    #[serde(default)]
    pub webhook_verify_token: Option<String>,
    /// Default template language code
    pub default_language: String,
}

impl Default for WhatsAppToolConfig {
    fn default() -> Self {
        Self {
            business_account_id: None,
            phone_number_id: None,
            access_token: None,
            webhook_verify_token: None,
            default_language: "ms".to_string(),
        }
    }
}

/// Google Sheets tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GoogleSheetsToolConfig {
    /// OAuth bearer access token (recommended for tool usage)
    #[serde(default)]
    pub access_token: Option<String>,
    /// Optional service account JSON encoded as base64
    #[serde(default)]
    pub service_account_base64: Option<String>,
}

// ============================================================================
// Memory Configuration
// ============================================================================

/// Memory backend selection.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryBackend {
    /// Disable memory tools.
    #[serde(rename = "none")]
    Disabled,
    /// Built-in workspace markdown memory.
    #[default]
    Builtin,
    /// QMD backend (falls back safely when unavailable).
    Qmd,
}

/// Memory citation mode.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCitationsMode {
    /// Show citations depending on channel context.
    #[default]
    Auto,
    /// Always include citations in snippets.
    On,
    /// Never include citations in snippets.
    Off,
}

/// Memory configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Memory backend to use.
    pub backend: MemoryBackend,
    /// Citation mode for memory snippets.
    pub citations: MemoryCitationsMode,
    /// Whether to include MEMORY.md + memory/**/*.md by default.
    pub include_default_memory: bool,
    /// Default maximum memory search results.
    pub max_results: u32,
    /// Minimum score threshold for memory search results.
    pub min_score: f32,
    /// Maximum snippet length returned per result.
    pub max_snippet_chars: u32,
    /// Extra workspace-relative file/dir paths to include.
    #[serde(default)]
    pub extra_paths: Vec<String>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: MemoryBackend::Builtin,
            citations: MemoryCitationsMode::Auto,
            include_default_memory: true,
            max_results: 6,
            min_score: 0.2,
            max_snippet_chars: 700,
            extra_paths: Vec::new(),
        }
    }
}

// ============================================================================
// Heartbeat Configuration
// ============================================================================

/// Heartbeat background service configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HeartbeatConfig {
    /// Enable or disable heartbeat service.
    pub enabled: bool,
    /// Heartbeat interval in seconds.
    pub interval_secs: u64,
    /// Optional heartbeat file path override.
    #[serde(default)]
    pub file_path: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 30 * 60,
            file_path: None,
        }
    }
}

// ============================================================================
// Skills Configuration
// ============================================================================

/// Skills system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Enable or disable the skills system.
    pub enabled: bool,
    /// Optional workspace skills directory override.
    #[serde(default)]
    pub workspace_dir: Option<String>,
    /// Skills that should always be injected into context.
    #[serde(default)]
    pub always_load: Vec<String>,
    /// Built-in or workspace skills to disable by name.
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_dir: None,
            always_load: Vec::new(),
            disabled: Vec::new(),
        }
    }
}

// ============================================================================
// Swarm / Multi-Agent Delegation Configuration
// ============================================================================

/// Swarm / multi-agent delegation configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SwarmConfig {
    /// Whether delegation is enabled.
    pub enabled: bool,
    /// Maximum delegation depth (1 = no sub-sub-agents).
    pub max_depth: u32,
    /// Maximum concurrent sub-agents (for future parallel mode).
    pub max_concurrent: u32,
    /// Pre-defined role presets with tool whitelists.
    pub roles: std::collections::HashMap<String, SwarmRole>,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 1,
            max_concurrent: 3,
            roles: std::collections::HashMap::new(),
        }
    }
}

/// A pre-defined sub-agent role with system prompt and tool whitelist.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SwarmRole {
    /// System prompt for this role.
    pub system_prompt: String,
    /// Allowed tool names (empty = all minus delegate/spawn).
    pub tools: Vec<String>,
}

// ============================================================================
// Runtime Configuration
// ============================================================================

/// Container runtime type for shell command execution
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeType {
    /// Native execution (no container isolation)
    #[default]
    Native,
    /// Docker container isolation
    Docker,
    /// Apple Container isolation (macOS only)
    #[serde(rename = "apple")]
    AppleContainer,
}

/// Runtime configuration for shell execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Type of container runtime to use
    pub runtime_type: RuntimeType,
    /// Whether to fall back to native runtime if configured runtime is unavailable
    pub allow_fallback_to_native: bool,
    /// Path to JSON allowlist used to validate runtime extra mounts
    #[serde(default = "default_mount_allowlist_path")]
    pub mount_allowlist_path: String,
    /// Docker-specific configuration
    pub docker: DockerConfig,
    /// Apple Container-specific configuration (macOS)
    pub apple: AppleContainerConfig,
}

fn default_mount_allowlist_path() -> String {
    "~/.zeptoclaw/mount-allowlist.json".to_string()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            runtime_type: RuntimeType::Native,
            allow_fallback_to_native: false,
            mount_allowlist_path: default_mount_allowlist_path(),
            docker: DockerConfig::default(),
            apple: AppleContainerConfig::default(),
        }
    }
}

/// Docker runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DockerConfig {
    /// Docker image to use for shell execution
    pub image: String,
    /// Additional volume mounts (host:container format)
    pub extra_mounts: Vec<String>,
    /// Memory limit (e.g., "512m")
    pub memory_limit: Option<String>,
    /// CPU limit (e.g., "1.0")
    pub cpu_limit: Option<String>,
    /// Network mode (default: none for security)
    pub network: String,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".to_string(),
            extra_mounts: Vec::new(),
            memory_limit: Some("512m".to_string()),
            cpu_limit: Some("1.0".to_string()),
            network: "none".to_string(),
        }
    }
}

/// Apple Container runtime configuration (macOS only)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppleContainerConfig {
    /// Container image/bundle path
    pub image: String,
    /// Additional directory mounts
    pub extra_mounts: Vec<String>,
}

// ============================================================================
// Containerized Agent Configuration
// ============================================================================

/// Container backend for the containerized agent proxy.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerAgentBackend {
    /// Auto-detect: on macOS try Apple Container first, then Docker.
    #[default]
    Auto,
    /// Always use Docker.
    Docker,
    /// Use Apple Container (macOS only).
    #[cfg(target_os = "macos")]
    #[serde(rename = "apple")]
    Apple,
}

/// Configuration for containerized agent mode.
///
/// When running with `--containerized`, the gateway spawns each agent
/// in an isolated container for multi-user safety.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerAgentConfig {
    /// Container backend to use (auto, docker, apple).
    pub backend: ContainerAgentBackend,
    /// Container image for the agent.
    pub image: String,
    /// Docker binary path/name override (Docker backend only).
    pub docker_binary: Option<String>,
    /// Memory limit (e.g., "1g") — Docker only, ignored by Apple Container.
    pub memory_limit: Option<String>,
    /// CPU limit (e.g., "2.0") — Docker only, ignored by Apple Container.
    pub cpu_limit: Option<String>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Network mode (default: "none" for security) — Docker only.
    pub network: String,
    /// Extra volume mounts (host:container format).
    pub extra_mounts: Vec<String>,
    /// Maximum number of concurrent container invocations.
    pub max_concurrent: usize,
}

impl Default for ContainerAgentConfig {
    fn default() -> Self {
        Self {
            backend: ContainerAgentBackend::Auto,
            image: "zeptoclaw:latest".to_string(),
            docker_binary: None,
            memory_limit: Some("1g".to_string()),
            cpu_limit: Some("2.0".to_string()),
            timeout_secs: 300,
            network: "none".to_string(),
            extra_mounts: Vec::new(),
            max_concurrent: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swarm_config_defaults() {
        let config = SwarmConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_depth, 1);
        assert_eq!(config.max_concurrent, 3);
        assert!(config.roles.is_empty());
    }

    #[test]
    fn test_swarm_config_deserialize() {
        let json = r#"{
            "enabled": true,
            "roles": {
                "researcher": {
                    "system_prompt": "You are a researcher.",
                    "tools": ["web_search", "web_fetch"]
                }
            }
        }"#;
        let config: SwarmConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.roles.len(), 1);
        let role = config.roles.get("researcher").unwrap();
        assert_eq!(role.tools, vec!["web_search", "web_fetch"]);
    }

    #[test]
    fn test_swarm_role_defaults() {
        let role = SwarmRole::default();
        assert!(role.system_prompt.is_empty());
        assert!(role.tools.is_empty());
    }

    #[test]
    fn test_streaming_defaults_to_false() {
        let defaults = AgentDefaults::default();
        assert!(!defaults.streaming);
    }

    #[test]
    fn test_streaming_config_deserialize() {
        let json = r#"{"streaming": true}"#;
        let defaults: AgentDefaults = serde_json::from_str(json).unwrap();
        assert!(defaults.streaming);
    }

    #[test]
    fn test_config_with_swarm_deserialize() {
        let json = r#"{
            "swarm": {
                "enabled": false,
                "max_depth": 2
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.swarm.enabled);
        assert_eq!(config.swarm.max_depth, 2);
    }
}
