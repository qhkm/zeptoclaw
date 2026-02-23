//! Configuration type definitions for ZeptoClaw
//!
//! This module defines all configuration structs used throughout the framework.
//! All types implement serde traits for JSON serialization and have sensible defaults.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Project management backend selection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectBackend {
    /// GitHub Issues REST API.
    #[default]
    Github,
    /// Jira REST API v3.
    Jira,
    /// Linear GraphQL API.
    Linear,
}

/// Project management tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// Backend to use (github, jira, linear).
    pub backend: ProjectBackend,
    /// Default project key/repo (e.g., "owner/repo" for GitHub, "PROJ" for Jira).
    pub default_project: String,
    /// Jira base URL (e.g., "https://your-org.atlassian.net").
    pub jira_url: String,
    /// Jira API token (base64 encoded "email:token").
    pub jira_token: Option<String>,
    /// GitHub personal access token.
    pub github_token: Option<String>,
    /// Linear API key.
    pub linear_api_key: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            backend: ProjectBackend::Github,
            default_project: String::new(),
            jira_url: String::new(),
            jira_token: None,
            github_token: None,
            linear_api_key: None,
        }
    }
}

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
    /// Tool approval configuration
    pub approval: crate::tools::approval::ApprovalConfig,
    /// Plugin system configuration
    pub plugins: crate::plugins::types::PluginConfig,
    /// Telemetry export configuration
    pub telemetry: crate::utils::telemetry::TelemetryConfig,
    /// Cost tracking configuration
    pub cost: crate::utils::cost::CostConfig,
    /// Batch processing configuration
    pub batch: crate::batch::BatchConfig,
    /// Hook system configuration
    pub hooks: crate::hooks::HooksConfig,
    /// Safety layer configuration
    pub safety: crate::safety::SafetyConfig,
    /// Context compaction configuration
    pub compaction: CompactionConfig,
    /// MCP (Model Context Protocol) server configuration
    pub mcp: McpConfig,
    /// Routines (event/webhook/cron triggers) configuration
    pub routines: RoutinesConfig,
    /// Tunnel configuration for exposing local ports publicly
    pub tunnel: TunnelConfig,
    /// Stripe payment integration configuration.
    pub stripe: StripeConfig,
    /// LLM response cache configuration
    pub cache: CacheConfig,
    /// Agent mode configuration (observer/assistant/autonomous)
    pub agent_mode: crate::security::agent_mode::AgentModeConfig,
    /// Device pairing configuration (bearer token auth for gateway)
    pub pairing: PairingConfig,
    /// Custom CLI-defined tools (shell commands as agent tools).
    #[serde(default)]
    pub custom_tools: Vec<CustomToolDef>,
    /// Audio transcription configuration.
    pub transcription: TranscriptionConfig,
    /// Named tool profiles for per-channel/context tool filtering.
    /// Key = profile name, Value = None means all tools, Some(vec) means only those tools.
    #[serde(default)]
    pub tool_profiles: HashMap<String, Option<Vec<String>>>,
    /// Project management tool configuration (GitHub Issues, Jira, Linear).
    pub project: ProjectConfig,
    /// HTTP health server configuration.
    #[serde(default)]
    pub health: HealthConfig,
    /// Device event system configuration (USB hotplug monitoring).
    #[serde(default)]
    pub devices: DevicesConfig,
    /// Logging configuration (format, level, optional file output).
    #[serde(default)]
    pub logging: LoggingConfig,
}

// ============================================================================
// Logging Configuration
// ============================================================================

/// Log output format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Default tracing pretty-print.
    Pretty,
    /// Component-tagged format — grep-friendly (`[component] message`).
    Component,
    /// Structured JSON lines for log aggregators.
    Json,
}

fn default_log_format() -> LogFormat {
    LogFormat::Component
}

fn default_log_level() -> String {
    "info".to_string()
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log output format (default: component).
    #[serde(default = "default_log_format")]
    pub format: LogFormat,
    /// Optional path to a log file. When set and format is `json`, logs are
    /// written to this file in addition to (or instead of) stdout.
    pub file: Option<String>,
    /// Log level filter string (default: "info").
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            format: default_log_format(),
            file: None,
            level: default_log_level(),
        }
    }
}

// ============================================================================
// Device Event System Configuration
// ============================================================================

/// Device event monitoring configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DevicesConfig {
    /// Enable device event monitoring (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Monitor USB hotplug events (default: false).
    #[serde(default)]
    pub monitor_usb: bool,
}

// ============================================================================
// Cache Configuration
// ============================================================================

/// LLM response cache configuration.
///
/// When enabled, caches LLM responses keyed by SHA-256 of
/// `(model, system_prompt, user_prompt)`. Supports TTL expiry and LRU eviction.
/// Persists to `~/.zeptoclaw/cache/responses.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Whether the response cache is enabled.
    pub enabled: bool,
    /// Time-to-live for cache entries in seconds.
    pub ttl_secs: u64,
    /// Maximum number of cached entries before LRU eviction.
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_secs: 3600,
            max_entries: 500,
        }
    }
}

// ============================================================================
// Pairing Configuration
// ============================================================================

/// Device pairing configuration.
///
/// When enabled, the gateway requires a valid bearer token from paired devices.
/// Devices are paired via a 6-digit one-time code exchanged for a bearer token.
/// Tokens are stored as SHA-256 hashes for security.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PairingConfig {
    /// Whether device pairing is required for gateway access.
    pub enabled: bool,
    /// Maximum failed pairing/validation attempts before lockout.
    pub max_attempts: u32,
    /// Duration in seconds to lock out after max_attempts is exceeded.
    pub lockout_secs: u64,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_attempts: 5,
            lockout_secs: 300,
        }
    }
}

// ============================================================================
// Health Server Configuration
// ============================================================================

fn default_health_host() -> String {
    "127.0.0.1".to_string()
}

fn default_health_port() -> u16 {
    9090
}

/// HTTP health server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Whether the health HTTP server is enabled (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Host/IP to bind the health server (default: 127.0.0.1).
    #[serde(default = "default_health_host")]
    pub host: String,
    /// Port to bind the health server (default: 9090).
    #[serde(default = "default_health_port")]
    pub port: u16,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_health_host(),
            port: default_health_port(),
        }
    }
}

// ============================================================================
// Compaction Configuration
// ============================================================================

/// Context compaction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// Whether automatic context compaction is enabled.
    pub enabled: bool,
    /// Maximum context window size in tokens.
    pub context_limit: usize,
    /// Fraction (0.0-1.0) of context_limit that triggers compaction.
    pub threshold: f64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            context_limit: 100_000,
            threshold: 0.80,
        }
    }
}

// ============================================================================
// MCP Configuration
// ============================================================================

/// MCP (Model Context Protocol) server configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// MCP server definitions.
    pub servers: Vec<McpServerConfig>,
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Human-readable server name (used as tool name prefix).
    pub name: String,
    /// Server URL endpoint.
    pub url: String,
    /// Request timeout in seconds (default: 30).
    #[serde(default = "default_mcp_timeout")]
    pub timeout_secs: u64,
}

fn default_mcp_timeout() -> u64 {
    30
}

// ============================================================================
// Routines Configuration
// ============================================================================

/// Routines (event/webhook/cron triggers) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutinesConfig {
    /// Whether the routines engine is enabled.
    pub enabled: bool,
    /// Cron tick interval in seconds.
    pub cron_interval_secs: u64,
    /// Maximum concurrent routine executions.
    pub max_concurrent: usize,
    /// Random jitter in milliseconds added to cron tick intervals.
    #[serde(default)]
    pub jitter_ms: u64,
    /// Policy for missed schedules when process restarts.
    #[serde(default)]
    pub on_miss: crate::cron::OnMiss,
}

impl Default for RoutinesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cron_interval_secs: 60,
            max_concurrent: 3,
            jitter_ms: 0,
            on_miss: crate::cron::OnMiss::Skip,
        }
    }
}

// ============================================================================
// Stripe Configuration
// ============================================================================

/// Stripe payment integration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StripeConfig {
    /// Stripe secret key (sk_live_... or sk_test_...). Supports ENC[...] encryption.
    pub secret_key: Option<String>,
    /// Default currency code for payment intents (e.g., "usd", "myr", "sgd").
    pub default_currency: String,
    /// Webhook signing secret for signature verification. Optional.
    pub webhook_secret: Option<String>,
}

impl Default for StripeConfig {
    fn default() -> Self {
        Self {
            secret_key: None,
            default_currency: "usd".to_string(),
            webhook_secret: None,
        }
    }
}

// ============================================================================
// Tunnel Configuration
// ============================================================================

/// Tunnel configuration for exposing local ports via public URLs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TunnelConfig {
    /// Tunnel provider name ("cloudflare", "ngrok", "tailscale", or "auto").
    pub provider: Option<String>,
    /// Cloudflare Tunnel configuration.
    pub cloudflare: Option<CloudflareTunnelConfig>,
    /// ngrok tunnel configuration.
    pub ngrok: Option<NgrokTunnelConfig>,
    /// Tailscale Funnel/Serve configuration.
    pub tailscale: Option<TailscaleTunnelConfig>,
}

/// Cloudflare Tunnel provider configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudflareTunnelConfig {
    /// Cloudflare Tunnel token for named tunnels. If omitted, uses quick tunnel (trycloudflare.com).
    pub token: Option<String>,
}

/// ngrok tunnel provider configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NgrokTunnelConfig {
    /// ngrok authtoken for authenticated tunnels.
    pub authtoken: Option<String>,
    /// Custom domain to use (requires ngrok paid plan).
    pub domain: Option<String>,
}

/// Tailscale Funnel/Serve provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TailscaleTunnelConfig {
    /// Use Tailscale Funnel (public) instead of Serve (tailnet-only). Default: true.
    #[serde(default = "default_true")]
    pub funnel: bool,
}

impl Default for TailscaleTunnelConfig {
    fn default() -> Self {
        Self { funnel: true }
    }
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Transcription Configuration
// ============================================================================

/// Configuration for audio transcription (voice messages).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    /// Whether to transcribe audio messages (default: true).
    pub enabled: bool,
    /// Whisper-compatible model name (default: "whisper-1").
    pub model: String,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "whisper-1".to_string(),
        }
    }
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
    /// Per-session token budget (input + output). 0 = unlimited.
    pub token_budget: u64,
    /// Use compact (shorter) tool descriptions to save tokens.
    #[serde(default)]
    pub compact_tools: bool,
    /// Default tool profile name (from `tool_profiles`). Omit for all tools.
    #[serde(default)]
    pub tool_profile: Option<String>,
    /// IANA timezone for the agent (e.g., "Asia/Kuala_Lumpur", "US/Pacific").
    /// Used for time-aware system prompts and message timestamps.
    /// Defaults to system local timezone, falls back to "UTC".
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

/// Detect the system's IANA timezone.
///
/// Priority: `TZ` env → `/etc/localtime` symlink → `"UTC"`.
fn default_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    #[cfg(unix)]
    {
        if let Ok(target) = std::fs::read_link("/etc/localtime") {
            let path = target.to_string_lossy();
            if let Some(pos) = path.find("zoneinfo/") {
                return path[pos + 9..].to_string();
            }
        }
    }
    "UTC".to_string()
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
            token_budget: 0,
            compact_tools: false,
            tool_profile: None,
            timezone: default_timezone(),
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
    /// WhatsApp Cloud API configuration (official API, no bridge)
    pub whatsapp_cloud: Option<WhatsAppCloudConfig>,
    /// Feishu (Lark) configuration
    pub feishu: Option<FeishuConfig>,
    /// Lark/Feishu WS long-connection configuration
    pub lark: Option<LarkConfig>,
    /// MaixCam configuration
    pub maixcam: Option<MaixCamConfig>,
    /// QQ configuration
    pub qq: Option<QQConfig>,
    /// DingTalk configuration
    pub dingtalk: Option<DingTalkConfig>,
    /// Webhook inbound channel configuration
    pub webhook: Option<WebhookConfig>,
    /// Email channel configuration (IMAP IDLE + SMTP). Feature-gated behind channel-email.
    pub email: Option<EmailConfig>,
    /// Directory for channel plugins (default: ~/.zeptoclaw/channels/)
    #[serde(default)]
    pub channel_plugins_dir: Option<String>,
}

/// Webhook inbound channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Address to bind the HTTP server to
    #[serde(default = "default_webhook_bind_address")]
    pub bind_address: String,
    /// Port to listen on
    #[serde(default = "default_webhook_port")]
    pub port: u16,
    /// URL path to accept webhook requests on
    #[serde(default = "default_webhook_path")]
    pub path: String,
    /// Optional Bearer token for request authentication
    #[serde(default)]
    pub auth_token: Option<String>,
    /// Allowlist of sender IDs (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
}

fn default_webhook_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_webhook_port() -> u16 {
    9876
}

fn default_webhook_path() -> String {
    "/webhook".to_string()
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: default_webhook_bind_address(),
            port: default_webhook_port(),
            path: default_webhook_path(),
            auth_token: None,
            allow_from: Vec::new(),
            deny_by_default: false,
        }
    }
}

/// Telegram channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Bot token from BotFather
    pub token: String,
    /// Allowlist of user IDs/usernames (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
}

/// Discord channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Bot token from Discord Developer Portal
    pub token: String,
    /// Allowlist of user IDs (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
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
    /// Allowlist of user IDs (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
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
    /// Optional Bearer token for authenticating to the bridge WebSocket.
    #[serde(default)]
    pub bridge_token: Option<String>,
    /// Allowlist of phone numbers (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
    /// Whether ZeptoClaw manages the bridge binary lifecycle.
    /// When true, `channel setup` and `gateway` will auto-install and start the bridge.
    /// When false, the user manages the bridge process externally.
    #[serde(default = "default_bridge_managed")]
    pub bridge_managed: bool,
}

fn default_whatsapp_bridge_url() -> String {
    "ws://localhost:3001".to_string()
}

fn default_bridge_managed() -> bool {
    true
}

impl Default for WhatsAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bridge_url: default_whatsapp_bridge_url(),
            bridge_token: None,
            allow_from: Vec::new(),
            deny_by_default: false,
            bridge_managed: default_bridge_managed(),
        }
    }
}

/// WhatsApp Cloud API channel configuration (official Meta API).
///
/// Uses Meta's webhook system for inbound messages and the Cloud API
/// for outbound replies. Does not require the whatsmeow-rs bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppCloudConfig {
    /// Whether the channel is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Phone number ID from Meta Business dashboard.
    #[serde(default)]
    pub phone_number_id: String,
    /// Permanent access token for Cloud API.
    #[serde(default)]
    pub access_token: String,
    /// Webhook verify token (you choose this secret, must match Meta dashboard).
    #[serde(default)]
    pub webhook_verify_token: String,
    /// Address to bind the webhook HTTP server to.
    #[serde(default = "default_whatsapp_cloud_bind")]
    pub bind_address: String,
    /// Port for the webhook HTTP server.
    #[serde(default = "default_whatsapp_cloud_port")]
    pub port: u16,
    /// URL path for the webhook endpoint.
    #[serde(default = "default_whatsapp_cloud_path")]
    pub path: String,
    /// Allowlist of phone numbers (empty = allow all unless `deny_by_default` is set).
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
}

fn default_whatsapp_cloud_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_whatsapp_cloud_port() -> u16 {
    9877
}

fn default_whatsapp_cloud_path() -> String {
    "/whatsapp".to_string()
}

impl Default for WhatsAppCloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            phone_number_id: String::new(),
            access_token: String::new(),
            webhook_verify_token: String::new(),
            bind_address: default_whatsapp_cloud_bind(),
            port: default_whatsapp_cloud_port(),
            path: default_whatsapp_cloud_path(),
            allow_from: Vec::new(),
            deny_by_default: false,
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
    /// Allowlist of user IDs (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
}

/// Lark (international) / Feishu (China) channel configuration.
///
/// Uses the Lark WS long-connection (pbbp2) for receiving events —
/// no public HTTPS endpoint required.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LarkConfig {
    /// Whether the channel is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Lark / Feishu application ID
    pub app_id: String,
    /// Lark / Feishu application secret
    pub app_secret: String,
    /// When true, use Feishu (open.feishu.cn); when false, use Lark (open.larksuite.com)
    #[serde(default)]
    pub feishu: bool,
    /// Allowlist of sender open_ids (empty = allow all unless deny_by_default)
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// Bot's own open_id — messages from the bot itself are silently dropped
    #[serde(default)]
    pub bot_open_id: Option<String>,
    /// When true, empty allowed_senders rejects all senders (strict mode)
    #[serde(default)]
    pub deny_by_default: bool,
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
    /// Allowlist of device IDs (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
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
            deny_by_default: false,
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
    /// Allowlist of QQ numbers (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
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
    /// Allowlist of user IDs (empty = allow all unless `deny_by_default` is set)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// When true, empty `allow_from` rejects all senders (strict mode).
    #[serde(default)]
    pub deny_by_default: bool,
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
    /// Ollama (local models) configuration
    pub ollama: Option<ProviderConfig>,
    /// Nvidia NIM configuration
    pub nvidia: Option<ProviderConfig>,
    /// Retry behavior for runtime provider calls
    pub retry: RetryConfig,
    /// Fallback behavior across multiple configured runtime providers
    pub fallback: FallbackConfig,
    /// Provider rotation configuration for 3+ health-aware providers
    pub rotation: RotationConfig,
    /// External binary provider plugins (JSON-RPC 2.0 over stdin/stdout)
    #[serde(default)]
    pub plugins: Vec<ProviderPluginConfig>,
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
    /// Authentication method: "api_key" (default), "oauth", or "auto"
    #[serde(default)]
    pub auth_method: Option<String>,
}

impl ProviderConfig {
    /// Resolve the authentication method for this provider.
    pub fn resolved_auth_method(&self) -> crate::auth::AuthMethod {
        crate::auth::AuthMethod::from_option(self.auth_method.as_deref())
    }
}

/// Configuration for an external binary LLM provider plugin.
///
/// The binary is invoked once per `chat()` call and communicates via
/// JSON-RPC 2.0 over stdin/stdout.
///
/// # Example (config.json)
/// ```json
/// {
///   "providers": {
///     "plugins": [
///       {"name": "myprovider", "command": "/usr/local/bin/my-provider", "args": ["--mode", "chat"]}
///     ]
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPluginConfig {
    /// Unique provider name. Plugin providers activate when no built-in provider (Anthropic/OpenAI) is configured.
    pub name: String,
    /// Path to the provider binary
    pub command: String,
    /// Additional arguments passed to the binary
    #[serde(default)]
    pub args: Vec<String>,
}

/// Retry behavior for runtime provider calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Enable automatic retry for transient provider errors.
    pub enabled: bool,
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff.
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds for exponential backoff.
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_retries: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        }
    }
}

/// Fallback behavior across multiple configured runtime providers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FallbackConfig {
    /// Enable provider fallback (primary -> secondary) when possible.
    pub enabled: bool,
    /// Optional preferred fallback provider id (e.g. "openai", "anthropic").
    pub provider: Option<String>,
}

/// Provider rotation configuration for 3+ health-aware providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RotationConfig {
    /// Enable provider rotation.
    #[serde(default)]
    pub enabled: bool,
    /// Provider names in rotation order (e.g., \["anthropic", "openai", "groq"\]).
    #[serde(default)]
    pub order: Vec<String>,
    /// Rotation strategy (priority or round_robin).
    #[serde(default)]
    pub strategy: crate::providers::rotation::RotationStrategy,
    /// Consecutive failures before marking provider unhealthy (default: 3).
    #[serde(default = "default_rotation_failure_threshold")]
    pub failure_threshold: u32,
    /// Seconds to wait before retrying unhealthy provider (default: 30).
    #[serde(default = "default_rotation_cooldown_secs")]
    pub cooldown_secs: u64,
}

fn default_rotation_failure_threshold() -> u32 {
    3
}

fn default_rotation_cooldown_secs() -> u64 {
    30
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            order: Vec::new(),
            strategy: crate::providers::rotation::RotationStrategy::default(),
            failure_threshold: default_rotation_failure_threshold(),
            cooldown_secs: default_rotation_cooldown_secs(),
        }
    }
}

// ============================================================================
// Gateway Configuration
// ============================================================================

/// Rate limiting configuration for gateway endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Max pairing requests per minute per IP (0 = unlimited).
    pub pair_per_min: u32,
    /// Max webhook requests per minute per IP (0 = unlimited).
    pub webhook_per_min: u32,
}

/// Gateway server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// Host to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// Per-IP rate limiting for gateway endpoints.
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            rate_limit: RateLimitConfig::default(),
        }
    }
}

// ============================================================================
// Tools Configuration
// ============================================================================

/// Voice transcription tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TranscribeConfig {
    /// Enable the transcribe tool
    #[serde(default)]
    pub enabled: bool,
    /// Groq API key for Whisper transcription
    pub groq_api_key: Option<String>,
    /// Whisper model to use
    #[serde(default = "default_transcribe_model")]
    pub model: String,
}

fn default_transcribe_model() -> String {
    "whisper-large-v3-turbo".to_string()
}

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
    /// HTTP request tool configuration
    pub http_request: Option<HttpRequestConfig>,
    /// Voice transcription tool configuration
    #[serde(default)]
    pub transcribe: TranscribeConfig,
    /// Skills marketplace (ClawHub) configuration
    #[serde(default)]
    pub skills: SkillsMarketplaceConfig,
}

/// Configuration for the HTTP request tool.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpRequestConfig {
    /// Allowlist of domains the agent may call. Required — tool fails fast if empty.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Request timeout in seconds. Default: 30.
    #[serde(default = "default_http_request_timeout")]
    pub timeout_secs: u64,
    /// Maximum response body size in bytes. Default: 512KB.
    #[serde(default = "default_http_request_max_bytes")]
    pub max_response_bytes: usize,
}

fn default_http_request_timeout() -> u64 {
    30
}

fn default_http_request_max_bytes() -> usize {
    512 * 1024
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
    /// Built-in substring search (default, zero cost).
    #[default]
    Builtin,
    /// BM25 keyword scoring (feature: memory-bm25).
    Bm25,
    /// LLM embedding + cosine similarity (feature: memory-embedding).
    Embedding,
    /// HNSW approximate nearest neighbor (feature: memory-hnsw).
    Hnsw,
    /// Tantivy full-text search engine (feature: memory-tantivy).
    Tantivy,
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
    /// Embedding provider name. Only used when backend is "embedding".
    #[serde(default)]
    pub embedding_provider: Option<String>,
    /// Embedding model name. Only used when backend is "embedding".
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// HNSW index file path override. Only used when backend is "hnsw".
    #[serde(default)]
    pub hnsw_index_path: Option<String>,
    /// Tantivy index directory path override. Only used when backend is "tantivy".
    #[serde(default)]
    pub tantivy_index_path: Option<String>,
    /// Memory hygiene scheduler configuration.
    #[serde(default)]
    pub hygiene: crate::memory::hygiene::HygieneConfig,
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
            embedding_provider: None,
            embedding_model: None,
            hnsw_index_path: None,
            tantivy_index_path: None,
            hygiene: crate::memory::hygiene::HygieneConfig::default(),
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
    /// Channel to deliver heartbeat results to (e.g., "telegram", "slack").
    /// If empty/none, heartbeat runs but results are not pushed.
    #[serde(default)]
    pub deliver_to: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 30 * 60,
            file_path: None,
            deliver_to: None,
        }
    }
}

// ============================================================================

// ============================================================================
// Skills Marketplace (ClawHub) Configuration
// ============================================================================

/// Skills marketplace (ClawHub) tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SkillsMarketplaceConfig {
    /// Enable skills marketplace tools (find_skills, install_skill).
    #[serde(default)]
    pub enabled: bool,
    /// ClawHub registry settings.
    #[serde(default)]
    pub clawhub: ClawHubConfig,
    /// In-memory search cache settings.
    #[serde(default)]
    pub search_cache: SearchCacheConfig,
}

/// ClawHub registry connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClawHubConfig {
    /// Enable the ClawHub registry (requires skills.enabled too).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Base URL for the ClawHub API.
    #[serde(default = "default_clawhub_url")]
    pub base_url: String,
    /// Optional Bearer token for authenticated registry access.
    #[serde(default)]
    pub auth_token: Option<String>,
}

fn default_clawhub_url() -> String {
    "https://clawhub.ai".to_string()
}

impl Default for ClawHubConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: default_clawhub_url(),
            auth_token: None,
        }
    }
}

/// In-memory search result cache settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchCacheConfig {
    /// Maximum number of cached queries.
    #[serde(default = "default_cache_size")]
    pub max_size: usize,
    /// Cache entry TTL in seconds.
    #[serde(default = "default_cache_ttl")]
    pub ttl_seconds: u64,
}

fn default_cache_size() -> usize {
    50
}

fn default_cache_ttl() -> u64 {
    300
}

impl Default for SearchCacheConfig {
    fn default() -> Self {
        Self {
            max_size: default_cache_size(),
            ttl_seconds: default_cache_ttl(),
        }
    }
}

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
    /// Landlock kernel LSM sandbox (Linux only, requires kernel 5.13+)
    #[cfg(target_os = "linux")]
    Landlock,
    /// Firejail userspace sandbox (Linux only, requires firejail binary)
    #[cfg(target_os = "linux")]
    Firejail,
    /// Bubblewrap OCI sandbox (Linux only, requires bwrap binary)
    #[cfg(target_os = "linux")]
    Bubblewrap,
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
    /// Landlock sandbox configuration (Linux only).
    pub landlock: LandlockConfig,
    /// Firejail sandbox configuration (Linux only).
    pub firejail: FirejailConfig,
    /// Bubblewrap sandbox configuration (Linux only).
    pub bubblewrap: BubblewrapConfig,
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
            landlock: LandlockConfig::default(),
            firejail: FirejailConfig::default(),
            bubblewrap: BubblewrapConfig::default(),
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
    /// PID limit to prevent fork bombs (e.g., 100). None = no limit.
    #[serde(default)]
    pub pids_limit: Option<u32>,
    /// Container stop timeout in seconds (matches agent timeout).
    /// Docker sends SIGTERM, waits this long, then SIGKILL.
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout_secs: u64,
}

fn default_stop_timeout() -> u64 {
    300 // match agent default timeout
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".to_string(),
            extra_mounts: Vec::new(),
            memory_limit: Some("512m".to_string()),
            cpu_limit: Some("1.0".to_string()),
            network: "none".to_string(),
            pids_limit: Some(100),
            stop_timeout_secs: default_stop_timeout(),
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
    /// Allow use of Apple Container runtime (experimental).
    /// When false (default), requesting the Apple Container runtime returns an error.
    pub allow_experimental: bool,
}

// ============================================================================
// Linux Sandbox Runtime Configuration
// ============================================================================

/// Landlock LSM sandbox configuration (Linux only).
///
/// Restricts filesystem access at the kernel level using the Linux Landlock LSM.
/// Requires Linux kernel 5.13+. Degrades gracefully on older kernels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LandlockConfig {
    /// Directories the sandboxed process may read (recursive).
    pub fs_read_dirs: Vec<String>,
    /// Directories the sandboxed process may write (recursive).
    pub fs_write_dirs: Vec<String>,
    /// Automatically add the agent workspace to the read allow list.
    pub allow_read_workspace: bool,
    /// Automatically add the agent workspace to the write allow list.
    pub allow_write_workspace: bool,
}

impl Default for LandlockConfig {
    fn default() -> Self {
        Self {
            fs_read_dirs: vec![
                "/usr".to_string(),
                "/lib".to_string(),
                "/lib64".to_string(),
                "/etc".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
                "/tmp".to_string(),
            ],
            fs_write_dirs: vec!["/tmp".to_string()],
            allow_read_workspace: true,
            allow_write_workspace: true,
        }
    }
}

/// Firejail sandbox configuration (Linux only).
///
/// Wraps commands with `firejail` using Linux namespaces + seccomp.
/// Requires the `firejail` binary on PATH.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FirejailConfig {
    /// Path to a custom firejail profile file.
    /// When None, `--noprofile` is used.
    pub profile: Option<String>,
    /// Extra arguments passed verbatim to firejail before the command.
    pub extra_args: Vec<String>,
}

/// Bubblewrap sandbox configuration (Linux only).
///
/// Wraps commands with `bwrap` (bubblewrap), a lightweight OCI-compatible sandbox.
/// Requires the `bwrap` binary on PATH.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BubblewrapConfig {
    /// Read-only bind mounts (each entry is a host path bound at the same container path).
    pub ro_binds: Vec<String>,
    /// Bind /dev into the sandbox (needed for most commands).
    pub dev_bind: bool,
    /// Bind /proc into the sandbox.
    pub proc_bind: bool,
    /// Extra arguments passed verbatim to bwrap before the command.
    pub extra_args: Vec<String>,
}

impl Default for BubblewrapConfig {
    fn default() -> Self {
        Self {
            ro_binds: vec![
                "/usr".to_string(),
                "/lib".to_string(),
                "/lib64".to_string(),
                "/etc".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
            ],
            dev_bind: true,
            proc_bind: true,
            extra_args: vec![],
        }
    }
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

/// A tool defined as a shell command in config.
///
/// Custom tools let users expose any shell command as an agent tool
/// without writing Rust code, plugin manifests, or MCP servers.
///
/// # Example
///
/// ```
/// use zeptoclaw::config::CustomToolDef;
/// use std::collections::HashMap;
///
/// let def = CustomToolDef {
///     name: "cpu_temp".to_string(),
///     description: "Read CPU temperature".to_string(),
///     command: "cat /sys/class/thermal/thermal_zone0/temp".to_string(),
///     parameters: None,
///     working_dir: None,
///     timeout_secs: None,
///     env: None,
/// };
/// assert_eq!(def.name, "cpu_temp");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDef {
    /// Tool name (alphanumeric + underscore only, used by LLM to invoke).
    pub name: String,
    /// Short description (keep under 60 chars for token efficiency).
    pub description: String,
    /// Shell command to execute. Supports {{param}} interpolation.
    pub command: String,
    /// Optional parameter definitions. Keys are param names, values are JSON Schema types.
    /// If omitted, tool takes no parameters (zero schema overhead).
    #[serde(default)]
    pub parameters: Option<HashMap<String, String>>,
    /// Optional working directory override.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Command timeout in seconds (default: 30).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Optional environment variables.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
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

    #[test]
    fn test_heartbeat_config_default_deliver_to() {
        let config = HeartbeatConfig::default();
        assert!(config.deliver_to.is_none());
    }

    #[test]
    fn test_heartbeat_config_deserialize_deliver_to() {
        let json = r#"{"enabled": true, "interval_secs": 600, "deliver_to": "telegram"}"#;
        let config: HeartbeatConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.deliver_to, Some("telegram".to_string()));
    }

    #[test]
    fn test_heartbeat_config_deserialize_no_deliver_to() {
        let json = r#"{"enabled": true, "interval_secs": 600}"#;
        let config: HeartbeatConfig = serde_json::from_str(json).unwrap();
        assert!(config.deliver_to.is_none());
    }

    #[test]
    fn test_custom_tool_def_deserialize() {
        let json = r#"{
            "name": "cpu_temp",
            "description": "Read CPU temp",
            "command": "cat /sys/class/thermal/thermal_zone0/temp",
            "parameters": {"zone": "string"},
            "working_dir": "/tmp",
            "timeout_secs": 10,
            "env": {"LANG": "C"}
        }"#;
        let def: CustomToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.name, "cpu_temp");
        assert_eq!(def.description, "Read CPU temp");
        assert_eq!(def.command, "cat /sys/class/thermal/thermal_zone0/temp");
        assert_eq!(
            def.parameters.as_ref().unwrap().get("zone").unwrap(),
            "string"
        );
        assert_eq!(def.working_dir.as_ref().unwrap(), "/tmp");
        assert_eq!(def.timeout_secs.unwrap(), 10);
        assert_eq!(def.env.as_ref().unwrap().get("LANG").unwrap(), "C");
    }

    #[test]
    fn test_custom_tool_def_minimal() {
        let json = r#"{"name": "test", "description": "Test tool", "command": "echo hi"}"#;
        let def: CustomToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.name, "test");
        assert!(def.parameters.is_none());
        assert!(def.working_dir.is_none());
        assert!(def.timeout_secs.is_none());
        assert!(def.env.is_none());
    }

    #[test]
    fn test_custom_tool_def_with_parameters() {
        let json = r#"{
            "name": "search_logs",
            "description": "Search logs",
            "command": "grep {{pattern}} /var/log/app.log",
            "parameters": {"pattern": "string"}
        }"#;
        let def: CustomToolDef = serde_json::from_str(json).unwrap();
        let params = def.parameters.unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params.get("pattern").unwrap(), "string");
    }

    #[test]
    fn test_custom_tools_default_empty() {
        let config = Config::default();
        assert!(config.custom_tools.is_empty());
    }

    #[test]
    fn test_tool_profiles_deserialize() {
        let json = r#"{
            "tool_profiles": {
                "minimal": ["shell", "longterm_memory"],
                "full": null
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.tool_profiles.len(), 2);
        let minimal = config.tool_profiles.get("minimal").unwrap();
        assert_eq!(minimal.as_ref().unwrap().len(), 2);
        assert!(config.tool_profiles.get("full").unwrap().is_none());
    }

    #[test]
    fn test_tool_profiles_default_empty() {
        let config = Config::default();
        assert!(config.tool_profiles.is_empty());
    }

    #[test]
    fn test_compact_tools_default_false() {
        let defaults = AgentDefaults::default();
        assert!(!defaults.compact_tools);
        assert!(defaults.tool_profile.is_none());
    }

    #[test]
    fn test_compact_tools_deserialize() {
        let json =
            r#"{"agents": {"defaults": {"compact_tools": true, "tool_profile": "minimal"}}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.agents.defaults.compact_tools);
        assert_eq!(
            config.agents.defaults.tool_profile.as_ref().unwrap(),
            "minimal"
        );
    }

    #[test]
    fn test_routines_config_jitter_default() {
        let config = RoutinesConfig::default();
        assert_eq!(config.jitter_ms, 0);
    }

    #[test]
    fn test_routines_config_jitter_deserialize() {
        let json = r#"{"enabled": true, "cron_interval_secs": 60, "max_concurrent": 3, "jitter_ms": 5000}"#;
        let config: RoutinesConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.jitter_ms, 5000);
    }

    #[test]
    fn test_tunnel_config_defaults() {
        let config = TunnelConfig::default();
        assert!(config.provider.is_none());
        assert!(config.cloudflare.is_none());
        assert!(config.ngrok.is_none());
        assert!(config.tailscale.is_none());
    }

    #[test]
    fn test_tunnel_config_deserialize() {
        let json = r#"{"tunnel": {"provider": "cloudflare", "cloudflare": {"token": "abc"}}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.tunnel.provider.as_deref(), Some("cloudflare"));
        assert_eq!(
            config.tunnel.cloudflare.as_ref().unwrap().token.as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn test_tailscale_tunnel_config_default_funnel_true() {
        let config = TailscaleTunnelConfig::default();
        assert!(config.funnel);
    }

    #[test]
    fn test_ngrok_tunnel_config_deserialize() {
        let json = r#"{"tunnel": {"provider": "ngrok", "ngrok": {"authtoken": "tok_123", "domain": "my.ngrok.io"}}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.tunnel.provider.as_deref(), Some("ngrok"));
        let ngrok = config.tunnel.ngrok.as_ref().unwrap();
        assert_eq!(ngrok.authtoken.as_deref(), Some("tok_123"));
        assert_eq!(ngrok.domain.as_deref(), Some("my.ngrok.io"));
    }

    #[test]
    fn test_whatsapp_cloud_config_defaults() {
        let config = WhatsAppCloudConfig::default();
        assert!(!config.enabled);
        assert!(config.phone_number_id.is_empty());
        assert!(config.access_token.is_empty());
        assert!(config.webhook_verify_token.is_empty());
        assert_eq!(config.bind_address, "127.0.0.1");
        assert_eq!(config.port, 9877);
        assert_eq!(config.path, "/whatsapp");
        assert!(config.allow_from.is_empty());
        assert!(!config.deny_by_default);
    }

    #[test]
    fn test_whatsapp_cloud_config_deserialize() {
        let json = r#"{
            "enabled": true,
            "phone_number_id": "123456",
            "access_token": "EAAx...",
            "webhook_verify_token": "my-verify-secret",
            "port": 8443,
            "allow_from": ["60123456789"]
        }"#;
        let config: WhatsAppCloudConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.phone_number_id, "123456");
        assert_eq!(config.access_token, "EAAx...");
        assert_eq!(config.webhook_verify_token, "my-verify-secret");
        assert_eq!(config.port, 8443);
        assert_eq!(config.allow_from, vec!["60123456789"]);
    }

    #[test]
    fn test_channels_config_with_whatsapp_cloud() {
        let json = r#"{
            "channels": {
                "whatsapp_cloud": {
                    "enabled": true,
                    "phone_number_id": "999",
                    "access_token": "tok",
                    "webhook_verify_token": "verify"
                }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let wac = config.channels.whatsapp_cloud.unwrap();
        assert!(wac.enabled);
        assert_eq!(wac.phone_number_id, "999");
    }

    #[test]
    fn test_memory_backend_bm25_deserialize() {
        let json = r#"{"memory": {"backend": "bm25"}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.memory.backend, MemoryBackend::Bm25);
    }

    #[test]
    fn test_memory_backend_embedding_deserialize() {
        let json = r#"{"memory": {"backend": "embedding", "embedding_provider": "openai", "embedding_model": "text-embedding-3-small"}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.memory.backend, MemoryBackend::Embedding);
        assert_eq!(config.memory.embedding_provider.as_deref(), Some("openai"));
        assert_eq!(
            config.memory.embedding_model.as_deref(),
            Some("text-embedding-3-small")
        );
    }

    #[test]
    fn test_memory_backend_hnsw_deserialize() {
        let json = r#"{"memory": {"backend": "hnsw"}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.memory.backend, MemoryBackend::Hnsw);
    }

    #[test]
    fn test_memory_backend_tantivy_deserialize() {
        let json = r#"{"memory": {"backend": "tantivy"}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.memory.backend, MemoryBackend::Tantivy);
    }

    #[test]
    fn test_transcription_config_defaults() {
        let config = Config::default();
        assert_eq!(config.transcription.model, "whisper-1");
        assert!(config.transcription.enabled);
    }

    #[test]
    fn test_memory_config_new_fields_default_none() {
        let config = MemoryConfig::default();
        assert!(config.embedding_provider.is_none());
        assert!(config.embedding_model.is_none());
        assert!(config.hnsw_index_path.is_none());
        assert!(config.tantivy_index_path.is_none());
    }

    #[test]
    fn test_docker_config_defaults() {
        let config = DockerConfig::default();
        assert_eq!(config.pids_limit, Some(100));
        assert_eq!(config.stop_timeout_secs, 300);
        assert_eq!(config.memory_limit, Some("512m".to_string()));
        assert_eq!(config.cpu_limit, Some("1.0".to_string()));
        assert_eq!(config.network, "none");
    }

    #[test]
    fn test_docker_config_deserialize_new_fields() {
        let json = r#"{"pids_limit": 50, "stop_timeout_secs": 120}"#;
        let config: DockerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.pids_limit, Some(50));
        assert_eq!(config.stop_timeout_secs, 120);
    }

    #[test]
    fn test_docker_config_deserialize_no_pids_limit() {
        let json = r#"{}"#;
        let config: DockerConfig = serde_json::from_str(json).unwrap();
        // Field-level #[serde(default)] on Option<u32> yields None when the key is absent.
        // The struct-level #[serde(default)] only applies when the whole struct key is missing.
        assert_eq!(config.pids_limit, None);
        assert_eq!(config.stop_timeout_secs, 300);
    }

    #[test]
    fn test_landlock_config_default_read_dirs() {
        let cfg = LandlockConfig::default();
        assert!(cfg.fs_read_dirs.iter().any(|d| d == "/usr"));
        assert!(cfg.allow_read_workspace);
        assert!(cfg.allow_write_workspace);
    }

    #[test]
    fn test_firejail_config_default_no_profile() {
        let cfg = FirejailConfig::default();
        assert!(cfg.profile.is_none());
        assert!(cfg.extra_args.is_empty());
    }

    #[test]
    fn test_bubblewrap_config_default_ro_binds() {
        let cfg = BubblewrapConfig::default();
        assert!(cfg.ro_binds.iter().any(|d| d == "/usr"));
        assert!(cfg.dev_bind);
        assert!(cfg.proc_bind);
    }

    #[test]
    fn test_runtime_config_has_sandbox_fields() {
        let cfg = RuntimeConfig::default();
        assert!(cfg.landlock.fs_read_dirs.contains(&"/usr".to_string()));
        assert!(cfg.firejail.profile.is_none());
        assert!(cfg.bubblewrap.dev_bind);
    }
}

// ---------------------------------------------------------------------------
// EmailConfig  (used by channels::EmailChannel, feature-gated: channel-email)
// ---------------------------------------------------------------------------

fn default_email_imap_port() -> u16 {
    993
}
fn default_email_smtp_port() -> u16 {
    587
}
fn default_email_imap_folder() -> String {
    "INBOX".into()
}
fn default_email_idle_timeout_secs() -> u64 {
    1740
}

/// Email channel configuration (IMAP IDLE inbound + SMTP outbound).
///
/// Stored under `channels.email` in `config.json`.
/// The channel is only functional when built with `--features channel-email`.
#[derive(Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// IMAP server hostname (e.g. `imap.gmail.com`)
    pub imap_host: String,
    /// IMAP server port. Default: 993 (implicit TLS).
    #[serde(default = "default_email_imap_port")]
    pub imap_port: u16,
    /// SMTP server hostname (e.g. `smtp.gmail.com`)
    pub smtp_host: String,
    /// SMTP server port. Default: 587 (STARTTLS).
    #[serde(default = "default_email_smtp_port")]
    pub smtp_port: u16,
    /// IMAP/SMTP login username.
    pub username: String,
    /// IMAP/SMTP login password (or app-password).
    pub password: String,
    /// IMAP mailbox folder to watch. Default: `INBOX`.
    #[serde(default = "default_email_imap_folder")]
    pub imap_folder: String,
    /// Optional display name used as "From" header in outgoing mail.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Allowlist of sender email addresses or domains.
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// When `true` and `allowed_senders` is empty, all senders are denied.
    #[serde(default)]
    pub deny_by_default: bool,
    /// Seconds before restarting IDLE (RFC 2177 recommends < 30 min). Default: 1740.
    #[serde(default = "default_email_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// When `true`, the channel is active. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: default_email_imap_port(),
            smtp_host: String::new(),
            smtp_port: default_email_smtp_port(),
            username: String::new(),
            password: String::new(),
            imap_folder: default_email_imap_folder(),
            display_name: None,
            allowed_senders: Vec::new(),
            deny_by_default: false,
            idle_timeout_secs: default_email_idle_timeout_secs(),
            enabled: false,
        }
    }
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig")
            .field("imap_host", &self.imap_host)
            .field("imap_port", &self.imap_port)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .field("imap_folder", &self.imap_folder)
            .field("display_name", &self.display_name)
            .field("allowed_senders", &self.allowed_senders)
            .field("deny_by_default", &self.deny_by_default)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .field("enabled", &self.enabled)
            .finish()
    }
}
