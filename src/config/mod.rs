//! Configuration management for ZeptoClaw
//!
//! This module provides configuration loading, saving, and global state management.
//! Configuration is loaded from `~/.zeptoclaw/config.json` with environment variable overrides.

pub mod templates;
mod types;
pub mod validate;

pub use types::*;

use crate::error::{Result, ZeptoError};
use once_cell::sync::OnceCell;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::RwLock;

/// Global configuration instance
static CONFIG: OnceCell<RwLock<Config>> = OnceCell::new();

impl Config {
    /// Returns the ZeptoClaw configuration directory path (~/.zeptoclaw)
    pub fn dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
    }

    /// Returns the path to the config file (~/.zeptoclaw/config.json)
    pub fn path() -> PathBuf {
        Self::dir().join("config.json")
    }

    /// Load configuration from the default path with environment overrides.
    ///
    /// If the config file doesn't exist, returns default configuration.
    /// Environment variables can override config values using the pattern:
    /// `ZEPTOCLAW_SECTION_SUBSECTION_KEY`
    pub fn load() -> Result<Self> {
        Self::load_from_path(&Self::path())
    }

    /// Load configuration from a specific path with environment overrides.
    ///
    /// If the config file contains `ENC[...]` encrypted values, they are
    /// transparently decrypted before the JSON is deserialized into `Config`.
    /// The master key is resolved via `ZEPTOCLAW_MASTER_KEY` env var or, when
    /// running in an interactive terminal, an interactive passphrase prompt.
    pub fn load_from_path(path: &PathBuf) -> Result<Self> {
        let mut config = if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let mut raw: serde_json::Value = serde_json::from_str(&content)?;

            // Decrypt ENC[...] values if present
            if has_encrypted_values(&raw) {
                let interactive = std::io::stdin().is_terminal();
                let enc = crate::security::encryption::resolve_master_key(interactive)?;
                decrypt_config_values(&mut raw, &enc)?;
            }

            serde_json::from_value(raw)?
        } else {
            Config::default()
        };

        // Apply environment variable overrides
        config.apply_env_overrides();

        Ok(config)
    }

    /// Apply environment variable overrides to the configuration.
    ///
    /// Environment variables follow the pattern: ZEPTOCLAW_SECTION_SUBSECTION_KEY
    fn apply_env_overrides(&mut self) {
        // Agent defaults
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_WORKSPACE") {
            self.agents.defaults.workspace = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_MODEL") {
            self.agents.defaults.model = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_MAX_TOKENS") {
            if let Ok(v) = val.parse() {
                self.agents.defaults.max_tokens = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_TEMPERATURE") {
            if let Ok(v) = val.parse() {
                self.agents.defaults.temperature = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS") {
            if let Ok(v) = val.parse() {
                self.agents.defaults.max_tool_iterations = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS") {
            if let Ok(v) = val.parse() {
                self.agents.defaults.agent_timeout_secs = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_TOKEN_BUDGET") {
            if let Ok(v) = val.parse() {
                self.agents.defaults.token_budget = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_MESSAGE_QUEUE_MODE") {
            match val.trim().to_ascii_lowercase().as_str() {
                "collect" => self.agents.defaults.message_queue_mode = MessageQueueMode::Collect,
                "followup" => self.agents.defaults.message_queue_mode = MessageQueueMode::Followup,
                _ => {}
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_COMPACT_TOOLS") {
            self.agents.defaults.compact_tools = val == "true" || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_TOOL_PROFILE") {
            self.agents.defaults.tool_profile = if val.is_empty() { None } else { Some(val) };
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_AGENTS_DEFAULTS_TIMEZONE") {
            self.agents.defaults.timezone = val;
        }

        // Gateway
        if let Ok(val) = std::env::var("ZEPTOCLAW_GATEWAY_HOST") {
            self.gateway.host = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_GATEWAY_PORT") {
            if let Ok(v) = val.parse() {
                self.gateway.port = v;
            }
        }

        // Provider API keys
        self.apply_provider_env_overrides();

        // Channel overrides
        self.apply_channel_env_overrides();

        // Memory-specific overrides
        self.apply_memory_env_overrides();

        // Heartbeat-specific overrides
        self.apply_heartbeat_env_overrides();

        // Skills-specific overrides
        self.apply_skills_env_overrides();

        // Tool-specific overrides
        self.apply_tool_env_overrides();

        // Hooks
        if let Ok(val) = std::env::var("ZEPTOCLAW_HOOKS_ENABLED") {
            self.hooks.enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }

        // Safety layer
        self.apply_safety_env_overrides();

        // Context compaction
        self.apply_compaction_env_overrides();

        // Routines
        self.apply_routines_env_overrides();
    }

    /// Apply provider-specific environment variable overrides
    fn apply_provider_env_overrides(&mut self) {
        // Anthropic
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY") {
            let provider = self
                .providers
                .anthropic
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_BASE") {
            let provider = self
                .providers
                .anthropic
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // OpenAI
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY") {
            let provider = self
                .providers
                .openai
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_OPENAI_API_BASE") {
            let provider = self
                .providers
                .openai
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // OpenRouter
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_OPENROUTER_API_KEY") {
            let provider = self
                .providers
                .openrouter
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_OPENROUTER_API_BASE") {
            let provider = self
                .providers
                .openrouter
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // Groq
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_GROQ_API_KEY") {
            let provider = self
                .providers
                .groq
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }

        // Zhipu
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_ZHIPU_API_KEY") {
            let provider = self
                .providers
                .zhipu
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_ZHIPU_API_BASE") {
            let provider = self
                .providers
                .zhipu
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // Groq api_base override
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_GROQ_API_BASE") {
            let provider = self
                .providers
                .groq
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // Gemini
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_GEMINI_API_KEY") {
            let provider = self
                .providers
                .gemini
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_GEMINI_API_BASE") {
            let provider = self
                .providers
                .gemini
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // vLLM
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_VLLM_API_KEY") {
            let provider = self
                .providers
                .vllm
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_VLLM_API_BASE") {
            let provider = self
                .providers
                .vllm
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // Ollama (local models)
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_OLLAMA_API_KEY") {
            let provider = self
                .providers
                .ollama
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_OLLAMA_API_BASE") {
            let provider = self
                .providers
                .ollama
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // Nvidia NIM
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_NVIDIA_API_KEY") {
            let provider = self
                .providers
                .nvidia
                .get_or_insert_with(ProviderConfig::default);
            provider.api_key = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_NVIDIA_API_BASE") {
            let provider = self
                .providers
                .nvidia
                .get_or_insert_with(ProviderConfig::default);
            provider.api_base = Some(val);
        }

        // Provider retry behavior
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_RETRY_ENABLED") {
            if let Ok(enabled) = val.parse() {
                self.providers.retry.enabled = enabled;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_RETRY_MAX_RETRIES") {
            if let Ok(v) = val.parse() {
                self.providers.retry.max_retries = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_RETRY_BASE_DELAY_MS") {
            if let Ok(v) = val.parse() {
                self.providers.retry.base_delay_ms = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_RETRY_MAX_DELAY_MS") {
            if let Ok(v) = val.parse() {
                self.providers.retry.max_delay_ms = v;
            }
        }

        // Provider fallback behavior
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_FALLBACK_ENABLED") {
            if let Ok(enabled) = val.parse() {
                self.providers.fallback.enabled = enabled;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_PROVIDERS_FALLBACK_PROVIDER") {
            let value = val.trim().to_string();
            self.providers.fallback.provider = if value.is_empty() { None } else { Some(value) };
        }
    }

    /// Apply channel-specific environment variable overrides
    fn apply_channel_env_overrides(&mut self) {
        // Telegram
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_TELEGRAM_TOKEN")
            .or_else(|_| std::env::var("ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN"))
        {
            let channel = self
                .channels
                .telegram
                .get_or_insert_with(TelegramConfig::default);
            channel.token = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_TELEGRAM_ENABLED") {
            if let Ok(enabled) = val.parse() {
                let channel = self
                    .channels
                    .telegram
                    .get_or_insert_with(TelegramConfig::default);
                channel.enabled = enabled;
            }
        }

        // Discord
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_DISCORD_TOKEN") {
            let channel = self
                .channels
                .discord
                .get_or_insert_with(DiscordConfig::default);
            channel.token = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_DISCORD_ENABLED") {
            if let Ok(enabled) = val.parse() {
                let channel = self
                    .channels
                    .discord
                    .get_or_insert_with(DiscordConfig::default);
                channel.enabled = enabled;
            }
        }

        // Slack
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_SLACK_BOT_TOKEN") {
            let channel = self.channels.slack.get_or_insert_with(SlackConfig::default);
            channel.bot_token = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_SLACK_APP_TOKEN") {
            let channel = self.channels.slack.get_or_insert_with(SlackConfig::default);
            channel.app_token = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_SLACK_ENABLED") {
            if let Ok(enabled) = val.parse() {
                let channel = self.channels.slack.get_or_insert_with(SlackConfig::default);
                channel.enabled = enabled;
            }
        }

        // WhatsApp
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_WHATSAPP_BRIDGE_URL") {
            let channel = self
                .channels
                .whatsapp
                .get_or_insert_with(WhatsAppConfig::default);
            channel.bridge_url = val;
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_WHATSAPP_BRIDGE_TOKEN") {
            let channel = self
                .channels
                .whatsapp
                .get_or_insert_with(WhatsAppConfig::default);
            channel.bridge_token = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_CHANNELS_WHATSAPP_ENABLED") {
            if let Ok(enabled) = val.parse() {
                let channel = self
                    .channels
                    .whatsapp
                    .get_or_insert_with(WhatsAppConfig::default);
                channel.enabled = enabled;
            }
        }

        // Runtime: Apple Container
        if let Ok(val) = std::env::var("ZEPTOCLAW_RUNTIME_APPLE_ALLOW_EXPERIMENTAL") {
            if let Ok(v) = val.parse() {
                self.runtime.apple.allow_experimental = v;
            }
        }
    }

    /// Apply tool-specific environment variable overrides
    fn apply_tool_env_overrides(&mut self) {
        // Web search API key (prefer explicit tool-scoped variable).
        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_WEB_SEARCH_API_KEY") {
            self.tools.web.search.api_key = Some(val);
        } else if let Ok(val) = std::env::var("ZEPTOCLAW_INTEGRATIONS_BRAVE_API_KEY") {
            self.tools.web.search.api_key = Some(val);
        } else if let Ok(val) = std::env::var("BRAVE_API_KEY") {
            self.tools.web.search.api_key = Some(val);
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_WEB_SEARCH_MAX_RESULTS") {
            if let Ok(v) = val.parse::<u32>() {
                self.tools.web.search.max_results = v.clamp(1, 10);
            }
        }

        // WhatsApp tool configuration
        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_WHATSAPP_PHONE_NUMBER_ID") {
            self.tools.whatsapp.phone_number_id = Some(val);
        } else if let Ok(val) = std::env::var("ZEPTOCLAW_INTEGRATIONS_WHATSAPP_PHONE_NUMBER_ID") {
            self.tools.whatsapp.phone_number_id = Some(val);
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_WHATSAPP_ACCESS_TOKEN") {
            self.tools.whatsapp.access_token = Some(val);
        } else if let Ok(val) = std::env::var("ZEPTOCLAW_INTEGRATIONS_WHATSAPP_ACCESS_TOKEN") {
            self.tools.whatsapp.access_token = Some(val);
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_WHATSAPP_DEFAULT_LANGUAGE") {
            if !val.trim().is_empty() {
                self.tools.whatsapp.default_language = val;
            }
        }

        // Google Sheets tool configuration
        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_GOOGLE_SHEETS_ACCESS_TOKEN") {
            self.tools.google_sheets.access_token = Some(val);
        } else if let Ok(val) = std::env::var("ZEPTOCLAW_INTEGRATIONS_GOOGLE_SHEETS_ACCESS_TOKEN") {
            self.tools.google_sheets.access_token = Some(val);
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_TOOLS_GOOGLE_SHEETS_SERVICE_ACCOUNT_BASE64") {
            self.tools.google_sheets.service_account_base64 = Some(val);
        } else if let Ok(val) =
            std::env::var("ZEPTOCLAW_INTEGRATIONS_GOOGLE_SHEETS_SERVICE_ACCOUNT_BASE64")
        {
            self.tools.google_sheets.service_account_base64 = Some(val);
        }
    }

    /// Apply memory-specific environment variable overrides.
    fn apply_memory_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_BACKEND") {
            let normalized = val.trim().to_ascii_lowercase();
            if let Some(parsed) = match normalized.as_str() {
                "none" | "disabled" => Some(MemoryBackend::Disabled),
                "builtin" => Some(MemoryBackend::Builtin),
                "qmd" => Some(MemoryBackend::Qmd),
                "bm25" => Some(MemoryBackend::Bm25),
                "embedding" => Some(MemoryBackend::Embedding),
                "hnsw" => Some(MemoryBackend::Hnsw),
                "tantivy" => Some(MemoryBackend::Tantivy),
                _ => None,
            } {
                self.memory.backend = parsed;
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_CITATIONS") {
            let normalized = val.trim().to_ascii_lowercase();
            if let Some(parsed) = match normalized.as_str() {
                "on" | "true" => Some(MemoryCitationsMode::On),
                "off" | "false" => Some(MemoryCitationsMode::Off),
                "auto" => Some(MemoryCitationsMode::Auto),
                _ => None,
            } {
                self.memory.citations = parsed;
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_MAX_RESULTS") {
            if let Ok(v) = val.parse::<u32>() {
                self.memory.max_results = v.clamp(1, 50);
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_MIN_SCORE") {
            if let Ok(v) = val.parse::<f32>() {
                self.memory.min_score = v.clamp(0.0, 1.0);
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_MAX_SNIPPET_CHARS") {
            if let Ok(v) = val.parse::<u32>() {
                self.memory.max_snippet_chars = v.clamp(64, 10_000);
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_INCLUDE_DEFAULT_MEMORY") {
            if let Ok(v) = val.parse::<bool>() {
                self.memory.include_default_memory = v;
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_EXTRA_PATHS") {
            self.memory.extra_paths = val
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_EMBEDDING_PROVIDER") {
            self.memory.embedding_provider = Some(val);
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_MEMORY_EMBEDDING_MODEL") {
            self.memory.embedding_model = Some(val);
        }
    }

    /// Apply heartbeat-specific environment variable overrides.
    fn apply_heartbeat_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ZEPTOCLAW_HEARTBEAT_ENABLED") {
            if let Ok(v) = val.parse::<bool>() {
                self.heartbeat.enabled = v;
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_HEARTBEAT_INTERVAL_SECS") {
            if let Ok(v) = val.parse::<u64>() {
                self.heartbeat.interval_secs = v.clamp(30, 24 * 60 * 60);
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_HEARTBEAT_FILE_PATH") {
            if !val.trim().is_empty() {
                self.heartbeat.file_path = Some(val);
            }
        }

        if let Ok(v) = std::env::var("ZEPTOCLAW_HEARTBEAT_DELIVER_TO") {
            self.heartbeat.deliver_to = if v.is_empty() { None } else { Some(v) };
        }
    }

    /// Apply skills-specific environment variable overrides.
    fn apply_skills_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ZEPTOCLAW_SKILLS_ENABLED") {
            if let Ok(v) = val.parse::<bool>() {
                self.skills.enabled = v;
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_SKILLS_WORKSPACE_DIR") {
            if !val.trim().is_empty() {
                self.skills.workspace_dir = Some(val);
            }
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_SKILLS_ALWAYS_LOAD") {
            self.skills.always_load = val
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
        }

        if let Ok(val) = std::env::var("ZEPTOCLAW_SKILLS_DISABLED") {
            self.skills.disabled = val
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
        }
    }

    /// Apply safety-layer environment variable overrides.
    fn apply_safety_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ZEPTOCLAW_SAFETY_ENABLED") {
            self.safety.enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_SAFETY_INJECTION_CHECK_ENABLED") {
            self.safety.injection_check_enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_SAFETY_LEAK_DETECTION_ENABLED") {
            self.safety.leak_detection_enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_SAFETY_MAX_OUTPUT_LENGTH") {
            if let Ok(v) = val.parse::<usize>() {
                self.safety.max_output_length = v.clamp(1_000, 10_000_000);
            }
        }
    }

    /// Apply context-compaction environment variable overrides.
    fn apply_compaction_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ZEPTOCLAW_COMPACTION_ENABLED") {
            self.compaction.enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_COMPACTION_CONTEXT_LIMIT") {
            if let Ok(v) = val.parse::<usize>() {
                self.compaction.context_limit = v.clamp(1_000, 1_000_000);
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_COMPACTION_THRESHOLD") {
            if let Ok(v) = val.parse::<f64>() {
                self.compaction.threshold = v.clamp(0.1, 1.0);
            }
        }
    }

    /// Apply routines environment variable overrides.
    fn apply_routines_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("ZEPTOCLAW_ROUTINES_ENABLED") {
            self.routines.enabled = val.eq_ignore_ascii_case("true") || val == "1";
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_ROUTINES_CRON_INTERVAL_SECS") {
            if let Ok(v) = val.parse::<u64>() {
                self.routines.cron_interval_secs = v.clamp(1, 3600);
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_ROUTINES_MAX_CONCURRENT") {
            if let Ok(v) = val.parse::<usize>() {
                self.routines.max_concurrent = v.clamp(1, 100);
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_ROUTINES_JITTER_MS") {
            if let Ok(v) = val.parse::<u64>() {
                self.routines.jitter_ms = v;
            }
        }
        if let Ok(val) = std::env::var("ZEPTOCLAW_ROUTINES_ON_MISS") {
            match val.to_lowercase().as_str() {
                "skip" => self.routines.on_miss = crate::cron::OnMiss::Skip,
                "run_once" => self.routines.on_miss = crate::cron::OnMiss::RunOnce,
                _ => {}
            }
        }
    }

    /// Save configuration to the default path
    pub fn save(&self) -> Result<()> {
        self.save_to_path(&Self::path())
    }

    /// Save configuration to a specific path
    pub fn save_to_path(&self, path: &PathBuf) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Initialize the global configuration.
    ///
    /// This should be called once at startup. Subsequent calls will return
    /// an error if the config is already initialized.
    pub fn init() -> Result<()> {
        let config = Self::load()?;
        CONFIG
            .set(RwLock::new(config))
            .map_err(|_| ZeptoError::Config("Configuration already initialized".to_string()))
    }

    /// Initialize the global configuration with a specific config.
    ///
    /// Useful for testing or custom initialization.
    pub fn init_with(config: Config) -> Result<()> {
        CONFIG
            .set(RwLock::new(config))
            .map_err(|_| ZeptoError::Config("Configuration already initialized".to_string()))
    }

    /// Get a clone of the current global configuration.
    ///
    /// Returns default configuration if not yet initialized.
    pub fn get() -> Config {
        CONFIG
            .get()
            .and_then(|lock| lock.read().ok())
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Update the global configuration.
    ///
    /// Returns an error if the config hasn't been initialized yet.
    pub fn update<F>(f: F) -> Result<()>
    where
        F: FnOnce(&mut Config),
    {
        let lock = CONFIG
            .get()
            .ok_or_else(|| ZeptoError::Config("Configuration not initialized".to_string()))?;
        let mut guard = lock
            .write()
            .map_err(|_| ZeptoError::Config("Failed to acquire config write lock".to_string()))?;
        f(&mut guard);
        Ok(())
    }

    /// Returns the expanded workspace path (resolves ~ to home directory)
    pub fn workspace_path(&self) -> PathBuf {
        expand_home(&self.agents.defaults.workspace)
    }

    /// Get the first available API key from configured providers.
    ///
    /// Checks providers in order: OpenRouter, Anthropic, OpenAI, Gemini, Zhipu, Groq
    pub fn get_api_key(&self) -> Option<String> {
        // Check providers in priority order
        let providers = [
            &self.providers.openrouter,
            &self.providers.anthropic,
            &self.providers.openai,
            &self.providers.gemini,
            &self.providers.zhipu,
            &self.providers.groq,
        ];

        for config in providers.into_iter().flatten() {
            if let Some(ref key) = config.api_key {
                if !key.is_empty() {
                    return Some(key.clone());
                }
            }
        }
        None
    }

    /// Get the API base URL for the first configured provider.
    pub fn get_api_base(&self) -> Option<String> {
        // OpenRouter
        if let Some(ref config) = self.providers.openrouter {
            if config
                .api_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false)
            {
                return config
                    .api_base
                    .clone()
                    .or_else(|| Some("https://openrouter.ai/api/v1".to_string()));
            }
        }

        // Zhipu
        if let Some(ref config) = self.providers.zhipu {
            if config
                .api_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false)
            {
                return config.api_base.clone();
            }
        }

        // VLLM
        if let Some(ref config) = self.providers.vllm {
            if config
                .api_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false)
            {
                return config.api_base.clone();
            }
        }

        None
    }
}

/// Check if any string value in the JSON tree starts with `ENC[`.
fn has_encrypted_values(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => {
            crate::security::encryption::SecretEncryption::is_encrypted(s)
        }
        serde_json::Value::Object(map) => map.values().any(has_encrypted_values),
        serde_json::Value::Array(arr) => arr.iter().any(has_encrypted_values),
        _ => false,
    }
}

/// Decrypt all `ENC[...]` string values in the JSON tree in place.
fn decrypt_config_values(
    value: &mut serde_json::Value,
    enc: &crate::security::encryption::SecretEncryption,
) -> Result<()> {
    match value {
        serde_json::Value::String(s) => {
            if crate::security::encryption::SecretEncryption::is_encrypted(s) {
                *s = enc.decrypt(s)?;
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values_mut() {
                decrypt_config_values(val, enc)?;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                decrypt_config_values(item, enc)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Expand ~ to home directory in a path string
fn expand_home(path: &str) -> PathBuf {
    if path.is_empty() {
        return PathBuf::from(path);
    }

    if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            if path.len() > 1 && path.chars().nth(1) == Some('/') {
                return home.join(&path[2..]);
            }
            return home;
        }
    }

    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.agents.defaults.model, "claude-sonnet-4-5-20250929");
        assert_eq!(config.agents.defaults.max_tokens, 8192);
        assert_eq!(config.agents.defaults.temperature, 0.7);
        assert_eq!(config.agents.defaults.max_tool_iterations, 20);
        assert_eq!(config.agents.defaults.workspace, "~/.zeptoclaw/workspace");
        assert_eq!(config.gateway.host, "0.0.0.0");
        assert_eq!(config.gateway.port, 8080);
        assert_eq!(config.memory.backend, MemoryBackend::Builtin);
        assert_eq!(config.memory.citations, MemoryCitationsMode::Auto);
        assert_eq!(config.memory.max_results, 6);
        assert_eq!(config.memory.min_score, 0.2);
        assert!(!config.heartbeat.enabled);
        assert_eq!(config.heartbeat.interval_secs, 30 * 60);
        assert!(config.skills.enabled);
        assert_eq!(config.runtime.runtime_type, RuntimeType::Native);
        assert!(!config.runtime.allow_fallback_to_native);
    }

    #[test]
    fn test_config_from_json() {
        let json = r#"{"agents": {"defaults": {"model": "gpt-4", "max_tokens": 4096}}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.agents.defaults.model, "gpt-4");
        assert_eq!(config.agents.defaults.max_tokens, 4096);
        // Defaults should apply to unspecified fields
        assert_eq!(config.agents.defaults.temperature, 0.7);
        assert_eq!(config.gateway.port, 8080);
    }

    #[test]
    fn test_config_to_json() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("claude-sonnet-4-5-20250929"));
        assert!(json.contains("8192"));
    }

    #[test]
    fn test_config_partial_json() {
        // Test that partial JSON works with defaults
        let json = r#"{"gateway": {"port": 9090}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.gateway.port, 9090);
        assert_eq!(config.gateway.host, "0.0.0.0"); // Default
        assert_eq!(config.agents.defaults.model, "claude-sonnet-4-5-20250929"); // Default
    }

    #[test]
    fn test_expand_home() {
        let home = dirs::home_dir().unwrap();

        // Test ~ expansion
        let expanded = expand_home("~/.zeptoclaw");
        assert_eq!(expanded, home.join(".zeptoclaw"));

        // Test ~/path expansion
        let expanded = expand_home("~/some/path");
        assert_eq!(expanded, home.join("some/path"));

        // Test absolute path (no expansion)
        let expanded = expand_home("/absolute/path");
        assert_eq!(expanded, PathBuf::from("/absolute/path"));

        // Test relative path (no expansion)
        let expanded = expand_home("relative/path");
        assert_eq!(expanded, PathBuf::from("relative/path"));

        // Test empty path
        let expanded = expand_home("");
        assert_eq!(expanded, PathBuf::from(""));
    }

    #[test]
    fn test_workspace_path() {
        let config = Config::default();
        let workspace = config.workspace_path();
        let home = dirs::home_dir().unwrap();
        assert_eq!(workspace, home.join(".zeptoclaw/workspace"));
    }

    #[test]
    fn test_config_dir() {
        let dir = Config::dir();
        let home = dirs::home_dir().unwrap();
        assert_eq!(dir, home.join(".zeptoclaw"));
    }

    #[test]
    fn test_config_path() {
        let path = Config::path();
        let home = dirs::home_dir().unwrap();
        assert_eq!(path, home.join(".zeptoclaw/config.json"));
    }

    #[test]
    fn test_channel_configs() {
        let json = r#"{
            "channels": {
                "telegram": {
                    "enabled": true,
                    "token": "bot123:ABC",
                    "allow_from": ["user1", "user2"]
                },
                "discord": {
                    "enabled": false,
                    "token": "discord-token"
                }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        let telegram = config.channels.telegram.unwrap();
        assert!(telegram.enabled);
        assert_eq!(telegram.token, "bot123:ABC");
        assert_eq!(telegram.allow_from, vec!["user1", "user2"]);

        let discord = config.channels.discord.unwrap();
        assert!(!discord.enabled);
        assert_eq!(discord.token, "discord-token");
    }

    #[test]
    fn test_provider_configs() {
        let json = r#"{
            "providers": {
                "anthropic": {
                    "api_key": "sk-ant-xxx"
                },
                "openai": {
                    "api_key": "sk-xxx",
                    "api_base": "https://api.openai.com/v1"
                }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        let anthropic = config.providers.anthropic.unwrap();
        assert_eq!(anthropic.api_key, Some("sk-ant-xxx".to_string()));

        let openai = config.providers.openai.unwrap();
        assert_eq!(openai.api_key, Some("sk-xxx".to_string()));
        assert_eq!(
            openai.api_base,
            Some("https://api.openai.com/v1".to_string())
        );
    }

    #[test]
    fn test_get_api_key() {
        let mut config = Config::default();

        // No API keys configured
        assert!(config.get_api_key().is_none());

        // Add OpenAI key
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("openai-key".to_string()),
            ..Default::default()
        });
        assert_eq!(config.get_api_key(), Some("openai-key".to_string()));

        // OpenRouter has higher priority
        config.providers.openrouter = Some(ProviderConfig {
            api_key: Some("openrouter-key".to_string()),
            ..Default::default()
        });
        assert_eq!(config.get_api_key(), Some("openrouter-key".to_string()));
    }

    #[test]
    fn test_env_override() {
        // Set env var
        env::set_var("ZEPTOCLAW_AGENTS_DEFAULTS_MODEL", "test-model");
        env::set_var("ZEPTOCLAW_AGENTS_DEFAULTS_MAX_TOKENS", "1000");
        env::set_var("BRAVE_API_KEY", "test-brave-key");
        env::set_var("ZEPTOCLAW_TOOLS_WEB_SEARCH_MAX_RESULTS", "9");
        env::set_var("ZEPTOCLAW_MEMORY_BACKEND", "none");
        env::set_var("ZEPTOCLAW_MEMORY_CITATIONS", "on");
        env::set_var("ZEPTOCLAW_MEMORY_MAX_RESULTS", "12");
        env::set_var("ZEPTOCLAW_MEMORY_MIN_SCORE", "0.55");
        env::set_var("ZEPTOCLAW_MEMORY_INCLUDE_DEFAULT_MEMORY", "false");
        env::set_var("ZEPTOCLAW_MEMORY_EXTRA_PATHS", "notes,archives/2026");
        env::set_var("ZEPTOCLAW_HEARTBEAT_ENABLED", "true");
        env::set_var("ZEPTOCLAW_HEARTBEAT_INTERVAL_SECS", "900");
        env::set_var("ZEPTOCLAW_HEARTBEAT_FILE_PATH", "/tmp/heartbeat.md");
        env::set_var("ZEPTOCLAW_SKILLS_ENABLED", "false");
        env::set_var("ZEPTOCLAW_SKILLS_ALWAYS_LOAD", "github,weather");
        env::set_var("ZEPTOCLAW_SKILLS_DISABLED", "experimental");
        env::set_var("ZEPTOCLAW_TOOLS_WHATSAPP_PHONE_NUMBER_ID", "123456");
        env::set_var("ZEPTOCLAW_TOOLS_WHATSAPP_ACCESS_TOKEN", "wa-token");
        env::set_var("ZEPTOCLAW_TOOLS_GOOGLE_SHEETS_ACCESS_TOKEN", "gs-token");

        let mut config = Config::default();
        config.apply_env_overrides();

        assert_eq!(config.agents.defaults.model, "test-model");
        assert_eq!(config.agents.defaults.max_tokens, 1000);
        assert_eq!(
            config.tools.web.search.api_key,
            Some("test-brave-key".to_string())
        );
        assert_eq!(config.tools.web.search.max_results, 9);
        assert_eq!(config.memory.backend, MemoryBackend::Disabled);
        assert_eq!(config.memory.citations, MemoryCitationsMode::On);
        assert_eq!(config.memory.max_results, 12);
        assert_eq!(config.memory.min_score, 0.55);
        assert!(!config.memory.include_default_memory);
        assert_eq!(
            config.memory.extra_paths,
            vec!["notes".to_string(), "archives/2026".to_string()]
        );
        assert!(config.heartbeat.enabled);
        assert_eq!(config.heartbeat.interval_secs, 900);
        assert_eq!(
            config.heartbeat.file_path,
            Some("/tmp/heartbeat.md".to_string())
        );
        assert!(!config.skills.enabled);
        assert_eq!(
            config.skills.always_load,
            vec!["github".to_string(), "weather".to_string()]
        );
        assert_eq!(config.skills.disabled, vec!["experimental".to_string()]);
        assert_eq!(
            config.tools.whatsapp.phone_number_id,
            Some("123456".to_string())
        );
        assert_eq!(
            config.tools.whatsapp.access_token,
            Some("wa-token".to_string())
        );
        assert_eq!(
            config.tools.google_sheets.access_token,
            Some("gs-token".to_string())
        );

        // Clean up
        env::remove_var("ZEPTOCLAW_AGENTS_DEFAULTS_MODEL");
        env::remove_var("ZEPTOCLAW_AGENTS_DEFAULTS_MAX_TOKENS");
        env::remove_var("BRAVE_API_KEY");
        env::remove_var("ZEPTOCLAW_TOOLS_WEB_SEARCH_MAX_RESULTS");
        env::remove_var("ZEPTOCLAW_MEMORY_BACKEND");
        env::remove_var("ZEPTOCLAW_MEMORY_CITATIONS");
        env::remove_var("ZEPTOCLAW_MEMORY_MAX_RESULTS");
        env::remove_var("ZEPTOCLAW_MEMORY_MIN_SCORE");
        env::remove_var("ZEPTOCLAW_MEMORY_INCLUDE_DEFAULT_MEMORY");
        env::remove_var("ZEPTOCLAW_MEMORY_EXTRA_PATHS");
        env::remove_var("ZEPTOCLAW_HEARTBEAT_ENABLED");
        env::remove_var("ZEPTOCLAW_HEARTBEAT_INTERVAL_SECS");
        env::remove_var("ZEPTOCLAW_HEARTBEAT_FILE_PATH");
        env::remove_var("ZEPTOCLAW_SKILLS_ENABLED");
        env::remove_var("ZEPTOCLAW_SKILLS_ALWAYS_LOAD");
        env::remove_var("ZEPTOCLAW_SKILLS_DISABLED");
        env::remove_var("ZEPTOCLAW_TOOLS_WHATSAPP_PHONE_NUMBER_ID");
        env::remove_var("ZEPTOCLAW_TOOLS_WHATSAPP_ACCESS_TOKEN");
        env::remove_var("ZEPTOCLAW_TOOLS_GOOGLE_SHEETS_ACCESS_TOKEN");
    }

    #[test]
    fn test_memory_config_from_json() {
        let json = r#"{
            "memory": {
                "backend": "qmd",
                "citations": "off",
                "include_default_memory": false,
                "max_results": 9,
                "min_score": 0.4,
                "max_snippet_chars": 320,
                "extra_paths": ["notes", "memory/archive"]
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.memory.backend, MemoryBackend::Qmd);
        assert_eq!(config.memory.citations, MemoryCitationsMode::Off);
        assert!(!config.memory.include_default_memory);
        assert_eq!(config.memory.max_results, 9);
        assert_eq!(config.memory.min_score, 0.4);
        assert_eq!(config.memory.max_snippet_chars, 320);
        assert_eq!(config.memory.extra_paths.len(), 2);
    }

    #[test]
    fn test_tools_config() {
        let json = r#"{
            "tools": {
                "web": {
                    "search": {
                        "api_key": "search-key",
                        "max_results": 10
                    }
                }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(
            config.tools.web.search.api_key,
            Some("search-key".to_string())
        );
        assert_eq!(config.tools.web.search.max_results, 10);
    }

    #[test]
    fn test_tools_config_defaults() {
        let config = Config::default();
        assert!(config.tools.web.search.api_key.is_none());
        assert_eq!(config.tools.web.search.max_results, 5);
        assert!(config.tools.whatsapp.phone_number_id.is_none());
        assert_eq!(config.tools.whatsapp.default_language, "ms");
        assert!(config.tools.google_sheets.access_token.is_none());
    }

    #[test]
    fn test_heartbeat_and_skills_config_from_json() {
        let json = r#"{
            "heartbeat": {
                "enabled": true,
                "interval_secs": 600,
                "file_path": "/tmp/heart.md"
            },
            "skills": {
                "enabled": false,
                "workspace_dir": "/tmp/skills",
                "always_load": ["github"],
                "disabled": ["legacy"]
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.heartbeat.enabled);
        assert_eq!(config.heartbeat.interval_secs, 600);
        assert_eq!(
            config.heartbeat.file_path,
            Some("/tmp/heart.md".to_string())
        );
        assert!(!config.skills.enabled);
        assert_eq!(config.skills.workspace_dir, Some("/tmp/skills".to_string()));
        assert_eq!(config.skills.always_load, vec!["github".to_string()]);
        assert_eq!(config.skills.disabled, vec!["legacy".to_string()]);
    }

    #[test]
    fn test_save_and_load() {
        use std::fs;

        // Create a temp directory
        let temp_dir = std::env::temp_dir().join("zeptoclaw_test");
        fs::create_dir_all(&temp_dir).unwrap();
        let config_path = temp_dir.join("config.json");

        // Create and save config
        let mut config = Config::default();
        config.agents.defaults.model = "test-model".to_string();
        config.gateway.port = 9999;
        config.save_to_path(&config_path).unwrap();

        // Load and verify
        let loaded = Config::load_from_path(&config_path).unwrap();
        assert_eq!(loaded.agents.defaults.model, "test-model");
        assert_eq!(loaded.gateway.port, 9999);

        // Clean up
        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_nonexistent() {
        let path = PathBuf::from("/nonexistent/path/config.json");
        let config = Config::load_from_path(&path).unwrap();

        // Should return defaults (check fields not affected by env var overrides)
        assert_eq!(config.agents.defaults.max_tool_iterations, 20);
        assert_eq!(config.agents.defaults.agent_timeout_secs, 300);
        assert!(!config.agents.defaults.model.is_empty());
    }

    #[test]
    fn test_agent_timeout_default() {
        let config = Config::default();
        assert_eq!(config.agents.defaults.agent_timeout_secs, 300);
    }

    #[test]
    fn test_agent_timeout_from_json() {
        let json = r#"{"agents": {"defaults": {"agent_timeout_secs": 600}}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.agents.defaults.agent_timeout_secs, 600);
    }

    #[test]
    fn test_message_queue_mode_default() {
        let config = Config::default();
        assert_eq!(
            config.agents.defaults.message_queue_mode,
            MessageQueueMode::Collect
        );
    }

    #[test]
    fn test_message_queue_mode_from_json() {
        let json = r#"{"agents": {"defaults": {"message_queue_mode": "followup"}}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.agents.defaults.message_queue_mode,
            MessageQueueMode::Followup
        );
    }

    #[test]
    fn test_env_override_compact_tools() {
        std::env::set_var("ZEPTOCLAW_AGENTS_DEFAULTS_COMPACT_TOOLS", "true");
        let mut config = Config::default();
        config.apply_env_overrides();
        assert!(config.agents.defaults.compact_tools);
        std::env::remove_var("ZEPTOCLAW_AGENTS_DEFAULTS_COMPACT_TOOLS");
    }
}
