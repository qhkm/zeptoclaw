//! Firejail sandbox runtime (Linux only).
//!
//! Wraps shell commands with `firejail` using Linux namespaces and seccomp.
//! Requires the `firejail` binary on PATH and the `sandbox-firejail` Cargo feature.

use async_trait::async_trait;

use crate::config::FirejailConfig;
use crate::runtime::types::{
    CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult,
};

/// Firejail sandbox runtime.
///
/// Wraps commands as: `firejail [--profile=<path> | --noprofile] [extra_args...] -- sh -c <command>`
///
/// When the `sandbox-firejail` feature is not enabled, [`execute`](ContainerRuntime::execute)
/// returns `Err(RuntimeError::NotAvailable(...))` with a clear recompilation message.
pub struct FirejailRuntime {
    config: FirejailConfig,
}

impl FirejailRuntime {
    /// Create a new Firejail runtime with the given configuration.
    pub fn new(config: FirejailConfig) -> Self {
        Self { config }
    }

    /// Build the argument list for the `firejail` invocation.
    ///
    /// The returned args are passed directly to `Command::new("firejail")`.
    ///
    /// # Argument order
    ///
    /// 1. `--profile=<path>` or `--noprofile` (when no profile is configured)
    /// 2. Any `extra_args` from the configuration
    /// 3. `--` separator
    /// 4. `sh -c <command>`
    pub fn build_args(&self, command: &str) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        // Profile selection: explicit path or --noprofile
        match &self.config.profile {
            Some(p) => args.push(format!("--profile={p}")),
            None => args.push("--noprofile".to_string()),
        }

        // Extra arguments from configuration
        for extra in &self.config.extra_args {
            args.push(extra.clone());
        }

        // Separator before the shell command
        args.push("--".to_string());
        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(command.to_string());

        args
    }
}

#[async_trait]
impl ContainerRuntime for FirejailRuntime {
    fn name(&self) -> &str {
        "firejail"
    }

    async fn is_available(&self) -> bool {
        #[cfg(feature = "sandbox-firejail")]
        {
            use std::process::Stdio;
            use tokio::process::Command;

            Command::new("which")
                .arg("firejail")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(not(feature = "sandbox-firejail"))]
        false
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        #[cfg(not(feature = "sandbox-firejail"))]
        {
            let _ = (command, config);
            return Err(RuntimeError::NotAvailable(
                "Recompile with --features sandbox-firejail to use the Firejail runtime."
                    .to_string(),
            ));
        }

        #[cfg(feature = "sandbox-firejail")]
        {
            use std::process::Stdio;
            use std::time::Duration;
            use tokio::process::Command;

            let args = self.build_args(command);
            let mut cmd = Command::new("firejail");
            cmd.args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

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
    use crate::config::FirejailConfig;

    #[test]
    fn test_firejail_runtime_name() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        assert_eq!(rt.name(), "firejail");
    }

    #[test]
    fn test_firejail_build_args_no_profile() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let args = rt.build_args("echo hello");
        assert!(args.contains(&"--noprofile".to_string()));
        assert!(args.contains(&"echo hello".to_string()));
        assert!(args.contains(&"sh".to_string()));
        assert!(args.contains(&"-c".to_string()));
        // No --profile= should be present
        assert!(!args.iter().any(|a| a.starts_with("--profile=")));
    }

    #[test]
    fn test_firejail_build_args_with_profile() {
        let rt = FirejailRuntime::new(FirejailConfig {
            profile: Some("/etc/firejail/default.profile".to_string()),
            extra_args: vec![],
        });
        let args = rt.build_args("ls");
        assert!(args
            .iter()
            .any(|a| a == "--profile=/etc/firejail/default.profile"));
        assert!(!args.contains(&"--noprofile".to_string()));
    }

    #[test]
    fn test_firejail_build_args_extra_args() {
        let rt = FirejailRuntime::new(FirejailConfig {
            profile: None,
            extra_args: vec!["--net=none".to_string(), "--memory=256m".to_string()],
        });
        let args = rt.build_args("ls");
        assert!(args.contains(&"--net=none".to_string()));
        assert!(args.contains(&"--memory=256m".to_string()));
    }

    #[test]
    fn test_firejail_build_args_command_after_separator() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let args = rt.build_args("my-cmd --flag");
        // "--" must appear before "sh"
        let sep_pos = args.iter().position(|a| a == "--").unwrap();
        let sh_pos = args.iter().position(|a| a == "sh").unwrap();
        assert!(sep_pos < sh_pos);
        // command is the last element
        assert_eq!(args.last().unwrap(), "my-cmd --flag");
    }

    #[test]
    fn test_firejail_build_args_ordering() {
        let rt = FirejailRuntime::new(FirejailConfig {
            profile: Some("/custom.profile".to_string()),
            extra_args: vec!["--net=none".to_string()],
        });
        let args = rt.build_args("whoami");
        // Expected order: --profile=..., --net=none, --, sh, -c, whoami
        assert_eq!(args[0], "--profile=/custom.profile");
        assert_eq!(args[1], "--net=none");
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "sh");
        assert_eq!(args[4], "-c");
        assert_eq!(args[5], "whoami");
    }

    #[cfg(not(feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_execute_without_feature_returns_error() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let cfg = ContainerConfig::new();
        let result = rt.execute("echo hi", &cfg).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("sandbox-firejail"),
            "Expected error mentioning sandbox-firejail, got: {msg}"
        );
    }

    #[cfg(not(feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_not_available_without_feature() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        assert!(!rt.is_available().await);
    }

    /// Only run echo test on Linux with the feature enabled (firejail is Linux-only).
    #[cfg(all(target_os = "linux", feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_runtime_echo() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let cfg = ContainerConfig::new();
        let out = rt.execute("echo hello", &cfg).await.unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("hello"));
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_runtime_timeout() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let cfg = ContainerConfig::new().with_timeout(1);
        let result = rt.execute("sleep 10", &cfg).await;
        assert!(matches!(result, Err(RuntimeError::Timeout(1))));
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_runtime_with_env() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let cfg = ContainerConfig::new().with_env("MY_VAR", "hello_firejail");
        let out = rt.execute("echo $MY_VAR", &cfg).await.unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("hello_firejail"));
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_runtime_exit_code() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let cfg = ContainerConfig::new();
        let out = rt.execute("exit 42", &cfg).await.unwrap();
        assert!(!out.success());
    }
}
