//! Native runtime implementation
//!
//! Executes commands directly on the host system without container isolation.
//! This is the fallback when no container runtime is configured.

use async_trait::async_trait;
use tokio::process::Command;

use super::process::{configure_environment, run_command};
use super::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeResult};

/// Native runtime that executes commands directly on the host
#[derive(Debug, Clone, Default)]
pub struct NativeRuntime {
    env_passthrough: Vec<String>,
}

impl NativeRuntime {
    /// Create a new native runtime
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow named parent environment variables to reach shell commands.
    pub fn with_env_passthrough(mut self, names: Vec<String>) -> Self {
        self.env_passthrough = names;
        self
    }
}

#[async_trait]
impl ContainerRuntime for NativeRuntime {
    fn name(&self) -> &str {
        "native"
    }

    async fn is_available(&self) -> bool {
        // Native runtime is always available
        true
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);

        // Set working directory if specified
        if let Some(ref workdir) = config.workdir {
            cmd.current_dir(workdir);
        }

        configure_environment(&mut cmd, &config.env, &self.env_passthrough);
        run_command(cmd, config.timeout_secs).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeError;

    #[tokio::test]
    async fn test_native_runtime_available() {
        let runtime = NativeRuntime::new();
        assert!(runtime.is_available().await);
    }

    #[tokio::test]
    async fn test_native_runtime_name() {
        let runtime = NativeRuntime::new();
        assert_eq!(runtime.name(), "native");
    }

    #[tokio::test]
    async fn test_native_runtime_echo() {
        let runtime = NativeRuntime::new();
        let config = ContainerConfig::new();

        let output = runtime.execute("echo hello", &config).await.unwrap();
        assert!(output.success());
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn test_native_runtime_with_workdir() {
        let runtime = NativeRuntime::new();
        let config = ContainerConfig::new().with_workdir(std::path::PathBuf::from("/tmp"));

        let output = runtime.execute("pwd", &config).await.unwrap();
        assert!(output.success());
        // On macOS /tmp is symlinked to /private/tmp
        assert!(output.stdout.contains("tmp"));
    }

    #[tokio::test]
    async fn test_native_runtime_with_env() {
        let runtime = NativeRuntime::new();
        let config = ContainerConfig::new().with_env("TEST_VAR", "test_value");

        let output = runtime.execute("echo $TEST_VAR", &config).await.unwrap();
        assert!(output.success());
        assert_eq!(output.stdout.trim(), "test_value");
    }

    #[tokio::test]
    async fn test_native_runtime_stderr() {
        let runtime = NativeRuntime::new();
        let config = ContainerConfig::new();

        let output = runtime.execute("echo error >&2", &config).await.unwrap();
        assert!(output.success());
        assert!(output.stderr.contains("error"));
    }

    #[tokio::test]
    async fn test_native_runtime_exit_code() {
        let runtime = NativeRuntime::new();
        let config = ContainerConfig::new();

        let output = runtime.execute("exit 42", &config).await.unwrap();
        assert!(!output.success());
        assert_eq!(output.exit_code, Some(42));
    }

    #[tokio::test]
    async fn test_native_runtime_timeout() {
        let runtime = NativeRuntime::new();
        let config = ContainerConfig::new().with_timeout(1);

        let result = runtime.execute("sleep 10", &config).await;
        assert!(matches!(result, Err(RuntimeError::Timeout(1))));
    }
}
