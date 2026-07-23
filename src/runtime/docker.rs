//! Docker runtime implementation
//!
//! Executes commands inside Docker containers for secure isolation.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::warn;
use uuid::Uuid;

use super::process::{configure_environment, passthrough_values, run_command};
use super::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult};

const DOCKER_PROBE_TIMEOUT_SECS: u64 = 10;
const DOCKER_CLEANUP_TIMEOUT_SECS: u64 = 10;

/// Docker runtime that executes commands in isolated containers
#[derive(Debug, Clone)]
pub struct DockerRuntime {
    /// Docker image to use
    image: String,
    /// Memory limit (e.g., "512m")
    memory_limit: Option<String>,
    /// CPU limit (e.g., "1.0")
    cpu_limit: Option<String>,
    /// Network mode
    network: String,
    /// Extra volume mounts from config (host:container or host:container:ro format)
    extra_mounts: Vec<String>,
    /// PID limit (--pids-limit flag)
    pids_limit: Option<u32>,
    /// Stop timeout in seconds (--stop-timeout flag)
    stop_timeout_secs: u64,
    /// Parent environment variables explicitly allowed into the container.
    env_passthrough: Vec<String>,
}

impl DockerRuntime {
    /// Create a new Docker runtime with the specified image
    pub fn new(image: &str) -> Self {
        Self {
            image: image.to_string(),
            memory_limit: Some("512m".to_string()),
            cpu_limit: Some("1.0".to_string()),
            network: "none".to_string(),
            extra_mounts: Vec::new(),
            pids_limit: Some(100),
            stop_timeout_secs: 300,
            env_passthrough: Vec::new(),
        }
    }

    /// Set memory limit
    pub fn with_memory_limit(mut self, limit: &str) -> Self {
        self.memory_limit = Some(limit.to_string());
        self
    }

    /// Set CPU limit
    pub fn with_cpu_limit(mut self, limit: &str) -> Self {
        self.cpu_limit = Some(limit.to_string());
        self
    }

    /// Set network mode
    pub fn with_network(mut self, network: &str) -> Self {
        self.network = network.to_string();
        self
    }

    /// Add extra volume mounts (host:container or host:container:ro format)
    pub fn with_extra_mounts(mut self, mounts: Vec<String>) -> Self {
        self.extra_mounts = mounts;
        self
    }

    /// Set PID limit to prevent fork bombs
    pub fn with_pids_limit(mut self, limit: u32) -> Self {
        self.pids_limit = Some(limit);
        self
    }

    /// Set container stop timeout in seconds
    pub fn with_stop_timeout(mut self, secs: u64) -> Self {
        self.stop_timeout_secs = secs;
        self
    }

    /// Allow named parent environment variables into Docker commands and containers.
    pub fn with_env_passthrough(mut self, names: Vec<String>) -> Self {
        self.env_passthrough = names;
        self
    }

    /// Disable resource limits
    pub fn without_limits(mut self) -> Self {
        self.memory_limit = None;
        self.cpu_limit = None;
        self.pids_limit = None;
        self
    }

    async fn remove_timed_out_container(&self, name: &str) {
        let mut command = Command::new("docker");
        command.args(["rm", "--force", name]);
        configure_environment(&mut command, &[], &self.env_passthrough);

        if let Err(error) = run_command(command, DOCKER_CLEANUP_TIMEOUT_SECS).await {
            warn!(container = name, %error, "failed to remove timed-out Docker container");
        }
    }
}

impl Default for DockerRuntime {
    fn default() -> Self {
        Self::new("alpine:latest")
    }
}

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    fn name(&self) -> &str {
        "docker"
    }

    async fn is_available(&self) -> bool {
        // Check if docker is installed and running
        let mut command = Command::new("docker");
        command.args(["info"]);
        configure_environment(&mut command, &[], &self.env_passthrough);
        run_command(command, DOCKER_PROBE_TIMEOUT_SECS)
            .await
            .map(|output| output.success())
            .unwrap_or(false)
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        let container_name = format!("zeptoclaw-{}", Uuid::new_v4().simple());
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "--network".to_string(),
            self.network.clone(),
        ];

        // Add resource limits
        if let Some(ref mem) = self.memory_limit {
            args.push("--memory".to_string());
            args.push(mem.clone());
        }
        if let Some(ref cpu) = self.cpu_limit {
            args.push("--cpus".to_string());
            args.push(cpu.clone());
        }
        if let Some(pids) = self.pids_limit {
            args.push("--pids-limit".to_string());
            args.push(pids.to_string());
        }
        args.push("--stop-timeout".to_string());
        args.push(self.stop_timeout_secs.to_string());

        // Add working directory
        if let Some(ref workdir) = config.workdir {
            args.push("-w".to_string());
            args.push(workdir.to_string_lossy().to_string());
        }

        // Add volume mounts from ContainerConfig
        for (host, container, readonly) in &config.mounts {
            let mount_spec = if *readonly {
                format!(
                    "{}:{}:ro",
                    host.to_string_lossy(),
                    container.to_string_lossy()
                )
            } else {
                format!("{}:{}", host.to_string_lossy(), container.to_string_lossy())
            };
            args.push("-v".to_string());
            args.push(mount_spec);
        }

        // Add extra mounts from runtime config (host:container or host:container:ro format)
        for mount in &self.extra_mounts {
            args.push("-v".to_string());
            args.push(mount.clone());
        }

        // Add explicitly allowed inherited environment variables first, then
        // command-scoped values so the latter take precedence.
        for (key, value) in passthrough_values(&self.env_passthrough) {
            args.push("-e".to_string());
            args.push(format!("{}={}", key, value));
        }
        for (key, value) in &config.env {
            args.push("-e".to_string());
            args.push(format!("{}={}", key, value));
        }

        // Add image and command
        args.push(self.image.clone());
        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(command.to_string());

        let mut cmd = Command::new("docker");
        cmd.args(&args);
        configure_environment(&mut cmd, &[], &self.env_passthrough);

        let result = run_command(cmd, config.timeout_secs).await;
        if matches!(result, Err(RuntimeError::Timeout(_))) {
            self.remove_timed_out_container(&container_name).await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_runtime_creation() {
        let runtime = DockerRuntime::new("ubuntu:22.04");
        assert_eq!(runtime.image, "ubuntu:22.04");
        assert_eq!(runtime.name(), "docker");
    }

    #[test]
    fn test_docker_runtime_builder() {
        let runtime = DockerRuntime::new("alpine:latest")
            .with_memory_limit("1g")
            .with_cpu_limit("2.0")
            .with_network("bridge");

        assert_eq!(runtime.memory_limit, Some("1g".to_string()));
        assert_eq!(runtime.cpu_limit, Some("2.0".to_string()));
        assert_eq!(runtime.network, "bridge");
    }

    #[test]
    fn test_docker_runtime_without_limits() {
        let runtime = DockerRuntime::new("alpine:latest").without_limits();
        assert!(runtime.memory_limit.is_none());
        assert!(runtime.cpu_limit.is_none());
    }

    #[test]
    fn test_docker_runtime_default() {
        let runtime = DockerRuntime::default();
        assert_eq!(runtime.image, "alpine:latest");
        assert_eq!(runtime.memory_limit, Some("512m".to_string()));
        assert_eq!(runtime.cpu_limit, Some("1.0".to_string()));
        assert_eq!(runtime.network, "none");
    }

    #[test]
    fn test_docker_runtime_pids_limit() {
        let runtime = DockerRuntime::new("alpine:latest").with_pids_limit(50);
        assert_eq!(runtime.pids_limit, Some(50));
    }

    #[test]
    fn test_docker_runtime_stop_timeout() {
        let runtime = DockerRuntime::new("alpine:latest").with_stop_timeout(120);
        assert_eq!(runtime.stop_timeout_secs, 120);
    }

    #[test]
    fn test_docker_runtime_default_pids_limit() {
        let runtime = DockerRuntime::default();
        assert_eq!(runtime.pids_limit, Some(100));
        assert_eq!(runtime.stop_timeout_secs, 300);
    }

    #[test]
    fn test_docker_runtime_without_limits_clears_pids() {
        let runtime = DockerRuntime::new("alpine:latest").without_limits();
        assert!(runtime.pids_limit.is_none());
        assert!(runtime.memory_limit.is_none());
        assert!(runtime.cpu_limit.is_none());
    }

    // Integration tests (only run if Docker is available)
    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn test_docker_runtime_available() {
        let runtime = DockerRuntime::new("alpine:latest");
        // This will only pass if Docker is installed and running
        assert!(runtime.is_available().await);
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn test_docker_runtime_echo() {
        let runtime = DockerRuntime::new("alpine:latest");
        let config = ContainerConfig::new();

        let output = runtime.execute("echo hello", &config).await.unwrap();
        assert!(output.success());
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn test_docker_runtime_isolation() {
        let runtime = DockerRuntime::new("alpine:latest");
        let config = ContainerConfig::new();

        // This should fail because network is disabled by default
        let output = runtime
            .execute("ping -c 1 google.com", &config)
            .await
            .unwrap();
        assert!(!output.success());
    }
}
