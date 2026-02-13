//! ZeptoClaw - Ultra-lightweight personal AI assistant framework

pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod error;
pub mod gateway;
pub mod heartbeat;
pub mod memory;
pub mod providers;
pub mod runtime;
pub mod security;
pub mod session;
pub mod skills;
pub mod tools;
pub mod utils;

pub use agent::{AgentLoop, ContextBuilder};
pub use bus::{InboundMessage, MediaAttachment, MediaType, MessageBus, OutboundMessage};
pub use channels::{BaseChannelConfig, Channel, ChannelManager, SlackChannel, TelegramChannel};
pub use config::Config;
pub use cron::{CronJob, CronPayload, CronSchedule, CronService};
pub use error::{Result, ZeptoError};
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

#[cfg(target_os = "macos")]
pub use runtime::AppleContainerRuntime;
pub use security::{
    validate_extra_mounts, validate_path_in_workspace, SafePath, ShellSecurityConfig,
};
pub use session::{Message, Role, Session, SessionManager, ToolCall};
pub use tools::{
    cron::CronTool, spawn::SpawnTool, EchoTool, GoogleSheetsTool, MemoryGetTool, MemorySearchTool,
    MessageTool, R8rTool, Tool, ToolContext, ToolRegistry, WebFetchTool, WebSearchTool,
    WhatsAppTool,
};
