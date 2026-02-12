//! ZeptoClaw - Ultra-lightweight personal AI assistant framework

pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod error;
pub mod providers;
pub mod runtime;
pub mod security;
pub mod session;
pub mod skills;
pub mod tools;
pub mod utils;

pub use agent::{AgentLoop, ContextBuilder};
pub use bus::{InboundMessage, MediaAttachment, MediaType, MessageBus, OutboundMessage};
pub use channels::{BaseChannelConfig, Channel, ChannelManager, TelegramChannel};
pub use config::Config;
pub use cron::{CronJob, CronPayload, CronSchedule, CronService};
pub use error::{PicoError, Result};
pub use providers::{
    ChatOptions, ClaudeProvider, LLMProvider, LLMResponse, LLMToolCall, OpenAIProvider,
    ToolDefinition, Usage,
};
pub use runtime::{
    available_runtimes, create_runtime, CommandOutput, ContainerConfig, ContainerRuntime,
    DockerRuntime, NativeRuntime, RuntimeError, RuntimeResult,
};

#[cfg(target_os = "macos")]
pub use runtime::AppleContainerRuntime;
pub use security::{
    validate_extra_mounts, validate_path_in_workspace, SafePath, ShellSecurityConfig,
};
pub use session::{Message, Role, Session, SessionManager, ToolCall};
pub use tools::{cron::CronTool, spawn::SpawnTool, EchoTool, Tool, ToolContext, ToolRegistry};
