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
    /// Runtime configuration for container isolation
    pub runtime: RuntimeConfig,
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
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.zeptoclaw/workspace".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 8096,
            temperature: 0.7,
            max_tool_iterations: 20,
        }
    }
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
