//! Apple Container runtime implementation (macOS only)
//!
//! Executes commands inside Apple's lightweight containers on macOS.
//! Uses the `container` tool from Apple's Containerization framework.
//!
//! # Warning: Experimental Implementation
//!
//! This runtime is based on the **expected** CLI interface for Apple's container tool.
//! The actual Apple Container API may differ. This implementation:
//!
//! - **Has not been validated** against official Apple documentation
//! - **May fail at runtime** even if `is_available()` returns true
//! - Should be tested thoroughly before production use
//!
//! The availability check only validates that `container --version` succeeds,
//! not that the `container run` syntax matches this implementation.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::warn;

use super::process::{configure_environment, passthrough_values, run_command};
use super::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeResult};

const CONTAINER_PROBE_TIMEOUT_SECS: u64 = 10;

/// Apple Container runtime for macOS
///
/// This runtime uses Apple's native container technology available on macOS 15+
/// (Sequoia). It provides lightweight isolation optimized for Apple Silicon.
///
/// # Warning
///
/// This is an **experimental** implementation based on expected CLI interface.
/// The actual Apple Container tool API may differ. Test thoroughly before use.
#[derive(Debug, Clone, Default)]
pub struct AppleContainerRuntime {
    /// Container image/bundle path (optional, uses default if not set)
    image: Option<String>,
    /// Extra directory mounts from config
    extra_mounts: Vec<String>,
    /// Parent environment variables explicitly allowed into the container.
    env_passthrough: Vec<String>,
}

impl AppleContainerRuntime {
    /// Create a new Apple Container runtime
    pub fn new() -> Self {
        Self::default()
    }

    /// Create runtime with a specific container image
    pub fn with_image(image: &str) -> Self {
        Self {
            image: Some(image.to_string()),
            extra_mounts: Vec::new(),
            env_passthrough: Vec::new(),
        }
    }

    /// Add extra mounts from configuration
    pub fn with_extra_mounts(mut self, mounts: Vec<String>) -> Self {
        self.extra_mounts = mounts;
        self
    }

    /// Allow named parent environment variables into container commands.
    pub fn with_env_passthrough(mut self, names: Vec<String>) -> Self {
        self.env_passthrough = names;
        self
    }
}

#[async_trait]
impl ContainerRuntime for AppleContainerRuntime {
    fn name(&self) -> &str {
        "apple"
    }

    async fn is_available(&self) -> bool {
        // Check if we're on macOS
        if !cfg!(target_os = "macos") {
            return false;
        }

        // Step 1: Check if container tool exists
        let mut version_command = Command::new("container");
        version_command.arg("--version");
        configure_environment(&mut version_command, &[], &self.env_passthrough);
        let version_check = run_command(version_command, CONTAINER_PROBE_TIMEOUT_SECS)
            .await
            .map(|output| output.success())
            .unwrap_or(false);

        if !version_check {
            return false;
        }

        // Step 2: Validate CLI syntax compatibility by attempting a simple command
        // This catches cases where the tool exists but uses different syntax
        // We use --help on the run subcommand to validate syntax without executing
        let mut syntax_command = Command::new("container");
        syntax_command.args(["run", "--help"]);
        configure_environment(&mut syntax_command, &[], &self.env_passthrough);
        let syntax_check = run_command(syntax_command, CONTAINER_PROBE_TIMEOUT_SECS)
            .await
            .map(|output| output.success())
            .unwrap_or(false);

        if !syntax_check {
            warn!(
                "Apple Container tool found but 'container run --help' failed. \
                CLI syntax may be incompatible with this implementation."
            );
            return false;
        }

        true
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        // WARNING: This is an experimental implementation based on expected CLI interface.
        // The actual Apple Container tool API may differ significantly.
        warn!(
            "Apple Container runtime is EXPERIMENTAL. \
            CLI interface may not match actual Apple Container tool. \
            Test thoroughly before production use."
        );

        let mut args = vec!["run".to_string()];

        // Add image if specified
        if let Some(ref image) = self.image {
            args.push("--image".to_string());
            args.push(image.clone());
        }

        // Add working directory
        if let Some(ref workdir) = config.workdir {
            args.push("--workdir".to_string());
            args.push(workdir.to_string_lossy().to_string());
        }

        // Add volume mounts from ContainerConfig
        for (host, container, readonly) in &config.mounts {
            args.push("--mount".to_string());
            let mount_spec = if *readonly {
                format!(
                    "type=bind,source={},target={},readonly",
                    host.to_string_lossy(),
                    container.to_string_lossy()
                )
            } else {
                format!(
                    "type=bind,source={},target={}",
                    host.to_string_lossy(),
                    container.to_string_lossy()
                )
            };
            args.push(mount_spec);
        }

        // Add extra mounts from runtime config
        for mount in &self.extra_mounts {
            args.push("--mount".to_string());
            // Assume format: source:target or source:target:ro
            let parts: Vec<&str> = mount.split(':').collect();
            let mount_spec = match parts.len() {
                2 => format!("type=bind,source={},target={}", parts[0], parts[1]),
                3 if parts[2] == "ro" => {
                    format!("type=bind,source={},target={},readonly", parts[0], parts[1])
                }
                _ => {
                    warn!("Invalid mount format '{}', skipping", mount);
                    continue;
                }
            };
            args.push(mount_spec);
        }

        // Add explicitly allowed inherited environment variables first, then
        // command-scoped values so the latter take precedence.
        for (key, value) in passthrough_values(&self.env_passthrough) {
            args.push("--env".to_string());
            args.push(format!("{}={}", key, value));
        }
        for (key, value) in &config.env {
            args.push("--env".to_string());
            args.push(format!("{}={}", key, value));
        }

        // Add the command
        args.push("--".to_string());
        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(command.to_string());

        let mut cmd = Command::new("container");
        cmd.args(&args);
        configure_environment(&mut cmd, &[], &self.env_passthrough);
        run_command(cmd, config.timeout_secs).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apple_runtime_creation() {
        let runtime = AppleContainerRuntime::new();
        assert_eq!(runtime.name(), "apple");
        assert!(runtime.image.is_none());
    }

    #[test]
    fn test_apple_runtime_with_image() {
        let runtime = AppleContainerRuntime::with_image("/path/to/image");
        assert_eq!(runtime.image, Some("/path/to/image".to_string()));
    }

    #[test]
    fn test_apple_runtime_default() {
        let runtime = AppleContainerRuntime::default();
        assert!(runtime.image.is_none());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires Apple Container framework"]
    async fn test_apple_runtime_available() {
        let runtime = AppleContainerRuntime::new();
        // This checks if the container tool is available
        println!(
            "Apple Container available: {}",
            runtime.is_available().await
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn test_apple_runtime_not_available_on_non_macos() {
        let runtime = AppleContainerRuntime::new();
        assert!(!runtime.is_available().await);
    }
}
