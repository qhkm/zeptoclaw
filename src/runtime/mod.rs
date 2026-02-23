//! Container runtime module for ZeptoClaw
//!
//! This module provides container isolation for shell command execution.
//! It supports multiple runtimes:
//! - Native: Direct execution (no isolation, uses application-level security)
//! - Docker: Docker container isolation (Linux, macOS, Windows)
//! - Apple Container: Apple's native container technology (macOS only)
//! - Landlock: Linux kernel LSM sandbox (Linux only, kernel 5.13+)
//! - Firejail: Linux namespace + seccomp sandbox (Linux only, requires firejail binary)
//! - Bubblewrap: OCI-compatible bwrap sandbox (Linux only, requires bwrap binary)

#[cfg(target_os = "macos")]
pub mod apple;
pub mod bubblewrap;
pub mod docker;
pub mod factory;
pub mod firejail;
pub mod landlock;
pub mod native;
pub mod types;

#[cfg(target_os = "macos")]
pub use apple::AppleContainerRuntime;
pub use bubblewrap::BubblewrapRuntime;
pub use docker::DockerRuntime;
pub use factory::{available_runtimes, create_runtime};
pub use firejail::FirejailRuntime;
pub use landlock::LandlockRuntime;
pub use native::NativeRuntime;
pub use types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult};
