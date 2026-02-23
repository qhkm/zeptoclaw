//! Bubblewrap (bwrap) sandbox runtime (Linux only).
//!
//! Wraps shell commands with `bwrap`, a lightweight OCI-compatible sandbox
//! used by Flatpak. Requires the `bwrap` binary on PATH and the
//! `sandbox-bubblewrap` Cargo feature.

use async_trait::async_trait;

use crate::config::BubblewrapConfig;
use crate::runtime::types::{
    CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult,
};

/// Bubblewrap (bwrap) sandbox runtime.
pub struct BubblewrapRuntime {
    config: BubblewrapConfig,
}

impl BubblewrapRuntime {
    pub fn new(config: BubblewrapConfig) -> Self {
        Self { config }
    }

    /// Build the argument list for the `bwrap` invocation.
    ///
    /// `workspace` is an optional host path that is bind-mounted read-write.
    pub fn build_args(&self, command: &str, workspace: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        for path in &self.config.ro_binds {
            args.push("--ro-bind".to_string());
            args.push(path.clone());
            args.push(path.clone());
        }

        if self.config.dev_bind {
            args.push("--dev".to_string());
            args.push("/dev".to_string());
        }

        if self.config.proc_bind {
            args.push("--proc".to_string());
            args.push("/proc".to_string());
        }

        // Workspace gets a writable bind
        if let Some(ws) = workspace {
            args.push("--bind".to_string());
            args.push(ws.to_string());
            args.push(ws.to_string());
        }

        // /tmp always writable
        args.push("--bind".to_string());
        args.push("/tmp".to_string());
        args.push("/tmp".to_string());

        for extra in &self.config.extra_args {
            args.push(extra.clone());
        }

        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(command.to_string());
        args
    }
}

#[async_trait]
impl ContainerRuntime for BubblewrapRuntime {
    fn name(&self) -> &str {
        "bubblewrap"
    }

    async fn is_available(&self) -> bool {
        #[cfg(feature = "sandbox-bubblewrap")]
        {
            use tokio::process::Command;
            Command::new("which")
                .arg("bwrap")
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(feature = "sandbox-bubblewrap"))]
        false
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        #[cfg(not(feature = "sandbox-bubblewrap"))]
        {
            let _ = (command, config);
            return Err(RuntimeError::NotAvailable(
                "Recompile with --features sandbox-bubblewrap to use the Bubblewrap runtime."
                    .to_string(),
            ));
        }

        #[cfg(feature = "sandbox-bubblewrap")]
        {
            use std::process::Stdio;
            use std::time::Duration;
            use tokio::process::Command;

            let workspace = config.workdir.as_ref().and_then(|p| p.to_str());
            let args = self.build_args(command, workspace);

            let mut cmd = Command::new("bwrap");
            for arg in &args {
                cmd.arg(arg);
            }
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            if let Some(ref workdir) = config.workdir {
                cmd.current_dir(workdir);
            }
            for (k, v) in &config.env {
                cmd.env(k, v);
            }

            let output =
                tokio::time::timeout(Duration::from_secs(config.timeout_secs), cmd.output())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BubblewrapConfig;

    #[test]
    fn test_bubblewrap_runtime_name() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        assert_eq!(rt.name(), "bubblewrap");
    }

    #[test]
    fn test_bubblewrap_build_args_contains_ro_binds() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let args = rt.build_args("echo hello", None);
        // Each ro_bind produces three tokens: --ro-bind <path> <path>
        assert!(args.contains(&"--ro-bind".to_string()));
        assert!(args.contains(&"/usr".to_string()));
    }

    #[test]
    fn test_bubblewrap_build_args_dev_proc() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let args = rt.build_args("ls", None);
        assert!(args.contains(&"--dev".to_string()));
        assert!(args.contains(&"--proc".to_string()));
    }

    #[test]
    fn test_bubblewrap_build_args_workspace_bind() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let args = rt.build_args("ls", Some("/workspace"));
        // workspace appears as --bind /workspace /workspace
        let bind_pos = args
            .windows(3)
            .any(|w| w[0] == "--bind" && w[1] == "/workspace" && w[2] == "/workspace");
        assert!(bind_pos, "Expected --bind /workspace /workspace in args");
    }

    #[test]
    fn test_bubblewrap_build_args_tmp_always_writable() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let args = rt.build_args("ls", None);
        let has_tmp_bind = args
            .windows(3)
            .any(|w| w[0] == "--bind" && w[1] == "/tmp" && w[2] == "/tmp");
        assert!(has_tmp_bind, "Expected --bind /tmp /tmp in args");
    }

    #[test]
    fn test_bubblewrap_build_args_command_is_last() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let args = rt.build_args("my-cmd --flag", None);
        assert_eq!(args.last().unwrap(), "my-cmd --flag");
    }

    #[test]
    fn test_bubblewrap_build_args_extra_args() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig {
            extra_args: vec!["--unshare-net".to_string()],
            ..BubblewrapConfig::default()
        });
        let args = rt.build_args("ls", None);
        assert!(args.contains(&"--unshare-net".to_string()));
    }

    #[cfg(not(feature = "sandbox-bubblewrap"))]
    #[tokio::test]
    async fn test_bubblewrap_execute_without_feature_returns_error() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let cfg = ContainerConfig::new();
        let result = rt.execute("echo hi", &cfg).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("sandbox-bubblewrap"),
            "Expected error mentioning sandbox-bubblewrap, got: {msg}"
        );
    }

    #[cfg(not(feature = "sandbox-bubblewrap"))]
    #[tokio::test]
    async fn test_bubblewrap_not_available_without_feature() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        assert!(!rt.is_available().await);
    }
}
