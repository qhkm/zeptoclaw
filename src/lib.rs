//! ZeptoClaw - Ultra-lightweight personal AI assistant

pub mod agent;
pub mod audit;
pub mod auth;
pub mod batch;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod deps;
pub mod error;
pub mod gateway;
pub mod health;
pub mod heartbeat;
pub mod hooks;
pub mod memory;
pub mod migrate;
pub mod plugins;
pub mod providers;
pub mod routines;
pub mod runtime;
pub mod safety;
pub use agent::{CompactionStrategy, ContextMonitor};
pub use config::CompactionConfig;
pub use safety::{SafetyConfig, SafetyLayer, SafetyResult};
pub mod security;
pub mod session;
pub mod skills;
pub mod tools;
pub mod tunnel;
pub mod utils;

pub use agent::{AgentLoop, ContextBuilder};
pub use bus::{InboundMessage, MediaAttachment, MediaType, MessageBus, OutboundMessage};
pub use channels::{
    BaseChannelConfig, Channel, ChannelManager, SlackChannel, TelegramChannel, WhatsAppChannel,
    WhatsAppCloudChannel,
};
pub use config::Config;
pub use cron::{CronJob, CronPayload, CronSchedule, CronService, OnMiss};
pub use error::{ProviderError, Result, ZeptoError};
pub use heartbeat::{ensure_heartbeat_file, HeartbeatService, HEARTBEAT_PROMPT};
pub use providers::{
    ChatOptions, ClaudeProvider, LLMProvider, LLMResponse, LLMToolCall, OpenAIProvider,
    ToolDefinition, Usage,
};
pub use runtime::{
    available_runtimes, create_runtime, CommandOutput, ContainerConfig, ContainerRuntime,
    DockerRuntime, NativeRuntime, RuntimeError, RuntimeResult,
};

pub use config::ContainerAgentBackend;
#[cfg(target_os = "macos")]
pub use gateway::is_apple_container_available;
pub use gateway::{
    generate_env_file_content, is_docker_available, is_docker_available_with_binary,
    parse_marked_response, resolve_backend, AgentRequest, AgentResponse, AgentResult,
    ContainerAgentProxy, ResolvedBackend, RESPONSE_END_MARKER, RESPONSE_START_MARKER,
};
pub use health::{health_port, start_health_server, start_periodic_usage_flush, UsageMetrics};

#[cfg(target_os = "macos")]
pub use runtime::AppleContainerRuntime;
pub use security::{
    validate_extra_mounts, validate_path_in_workspace, SafePath, ShellSecurityConfig,
};
pub use session::{Message, Role, Session, SessionManager, ToolCall};
pub use tools::{
    cron::CronTool, custom::CustomTool, delegate::DelegateTool, spawn::SpawnTool, BinaryPluginTool,
    EchoTool, GoogleSheetsTool, MemoryGetTool, MemorySearchTool, MessageTool, R8rTool,
    ReminderTool, Tool, ToolContext, ToolRegistry, WebFetchTool, WebSearchTool, WhatsAppTool,
};
