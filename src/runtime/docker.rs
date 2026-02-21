//! Docker runtime implementation
//!
//! Executes commands inside Docker containers for secure isolation.

use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use super::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult};

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

    /// Disable resource limits
    pub fn without_limits(mut self) -> Self {
        self.memory_limit = None;
        self.cpu_limit = None;
        self.pids_limit = None;
        self
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
        Command::new("docker")
            .args(["info"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
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

        // Add environment variables
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
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Execute with timeout
        let output = tokio::time::timeout(Duration::from_secs(config.timeout_secs), cmd.output())
            .await
            .map_err(|_| RuntimeError::Timeout(config.timeout_secs))?
            .map_err(|e| RuntimeError::ExecutionFailed(e.to_string()))?;

        Ok(CommandOutput::new(
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status.code(),
        ))
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
