//! Gateway module for containerized agent support
//!
//! This module provides the ContainerAgentProxy for running agents in
//! isolated containers (Docker or Apple Container), enabling multi-user
//! scenarios with proper isolation between requests.

pub mod container_agent;
pub mod ipc;

#[cfg(target_os = "macos")]
pub use container_agent::is_apple_container_available;
pub use container_agent::{
    generate_env_file_content, is_docker_available, is_docker_available_with_binary,
    resolve_backend, ContainerAgentProxy, ResolvedBackend,
};
pub use ipc::{parse_marked_response, AgentRequest, AgentResponse, AgentResult};
pub use ipc::{RESPONSE_END_MARKER, RESPONSE_START_MARKER};

pub mod idempotency;
pub mod rate_limit;
pub use idempotency::IdempotencyStore;
pub use rate_limit::{GatewayRateLimiter, SlidingWindowRateLimiter};
