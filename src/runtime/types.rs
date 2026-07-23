//! Runtime type definitions

use async_trait::async_trait;
use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during runtime operations
#[derive(Error, Debug)]
pub enum RuntimeError {
    /// Container runtime not available
    #[error("Runtime not available: {0}")]
    NotAvailable(String),

    /// Failed to start container
    #[error("Failed to start container: {0}")]
    StartFailed(String),

    /// Command execution failed
    #[error("Command execution failed: {0}")]
    ExecutionFailed(String),

    /// Timeout exceeded
    #[error("Command timed out after {0} seconds")]
    Timeout(u64),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for runtime operations
pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

/// Output from a command execution
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code (None if killed by signal)
    pub exit_code: Option<i32>,
}

impl CommandOutput {
    /// Create a new CommandOutput
    pub fn new(stdout: String, stderr: String, exit_code: Option<i32>) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
        }
    }

    /// Check if the command succeeded (exit code 0)
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Format output similar to current shell tool behavior
    pub fn format(&self) -> String {
        let mut result = String::new();

        if !self.stdout.is_empty() {
            result.push_str(&self.stdout);
        }

        if !self.stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n--- stderr ---\n");
            }
            result.push_str(&self.stderr);
        }

        if let Some(code) = self.exit_code {
            if code != 0 {
                result.push_str(&format!("\n[Exit code: {}]", code));
            }
        }

        result
    }
}

/// Configuration for a container execution
#[derive(Debug, Clone, Default)]
pub struct ContainerConfig {
    /// Working directory inside container
    pub workdir: Option<PathBuf>,
    /// Directories to mount (host_path, container_path, readonly)
    pub mounts: Vec<(PathBuf, PathBuf, bool)>,
    /// Environment variables
    pub env: Vec<(String, String)>,
    /// Command timeout in seconds
    pub timeout_secs: u64,
}

impl ContainerConfig {
    /// Create a new container config
    pub fn new() -> Self {
        Self {
            timeout_secs: 60,
            ..Default::default()
        }
    }

    /// Set working directory
    pub fn with_workdir(mut self, workdir: PathBuf) -> Self {
        self.workdir = Some(workdir);
        self
    }

    /// Add a mount point
    pub fn with_mount(mut self, host: PathBuf, container: PathBuf, readonly: bool) -> Self {
        self.mounts.push((host, container, readonly));
        self
    }

    /// Add an environment variable
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// Trait for container runtimes
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Get the runtime name
    fn name(&self) -> &str;

    /// Check if this runtime is available on the system
    async fn is_available(&self) -> bool;

    /// Execute a command in the container
    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_output_success() {
        let output = CommandOutput::new("hello".to_string(), "".to_string(), Some(0));
        assert!(output.success());
    }

    #[test]
    fn test_command_output_failure() {
        let output = CommandOutput::new("".to_string(), "error".to_string(), Some(1));
        assert!(!output.success());
    }

    #[test]
    fn test_command_output_format_stdout_only() {
        let output = CommandOutput::new("output".to_string(), "".to_string(), Some(0));
        assert_eq!(output.format(), "output");
    }

    #[test]
    fn test_command_output_format_with_stderr() {
        let output = CommandOutput::new("stdout".to_string(), "stderr".to_string(), Some(0));
        let formatted = output.format();
        assert!(formatted.contains("stdout"));
        assert!(formatted.contains("--- stderr ---"));
        assert!(formatted.contains("stderr"));
    }

    #[test]
    fn test_command_output_format_with_exit_code() {
        let output = CommandOutput::new("".to_string(), "".to_string(), Some(1));
        let formatted = output.format();
        assert!(formatted.contains("[Exit code: 1]"));
    }

    #[test]
    fn test_container_config_builder() {
        let config = ContainerConfig::new()
            .with_workdir(PathBuf::from("/workspace"))
            .with_mount(PathBuf::from("/host"), PathBuf::from("/container"), true)
            .with_env("FOO", "bar")
            .with_timeout(120);

        assert_eq!(config.workdir, Some(PathBuf::from("/workspace")));
        assert_eq!(config.mounts.len(), 1);
        assert_eq!(config.env.len(), 1);
        assert_eq!(config.env[0], ("FOO".to_string(), "bar".to_string()));
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_container_config_default_timeout() {
        let config = ContainerConfig::new();
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn test_runtime_error_display() {
        let err = RuntimeError::Timeout(30);
        assert_eq!(err.to_string(), "Command timed out after 30 seconds");
    }

    #[test]
    fn test_runtime_config_env_passthrough_defaults_empty() {
        let config = crate::config::RuntimeConfig::default();
        assert!(config.env_passthrough.is_empty());
    }

    #[test]
    fn test_runtime_config_env_passthrough_deserializes() {
        let config: crate::config::RuntimeConfig =
            serde_json::from_str(r#"{"env_passthrough":["GH_TOKEN"]}"#).unwrap();
        assert_eq!(config.env_passthrough, vec!["GH_TOKEN"]);
    }
}
