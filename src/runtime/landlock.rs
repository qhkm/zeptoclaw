//! Landlock LSM sandbox runtime (Linux only).
//!
//! Uses the `landlock` crate to apply kernel-level filesystem access rules
//! before spawning shell commands. Requires Linux kernel 5.13+.
//! Degrades gracefully on older kernels via ABI negotiation.
//!
//! # Architecture
//!
//! Landlock restrictions are applied in the **child process** via `pre_exec`,
//! so the parent (ZeptoClaw) process is never restricted. The child inherits
//! the Landlock ruleset after fork but before exec.
//!
//! When the `sandbox-landlock` feature is not enabled, `execute()` returns
//! `Err(RuntimeError::NotAvailable(...))` with a clear message.

use async_trait::async_trait;

use crate::config::LandlockConfig;
use crate::runtime::types::{
    CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult,
};

/// Landlock LSM sandbox runtime.
///
/// Applies kernel-level filesystem access restrictions using the Linux Landlock LSM.
/// Requires Linux 5.13+; gracefully degrades on older kernels.
pub struct LandlockRuntime {
    config: LandlockConfig,
}

impl LandlockRuntime {
    /// Create a new Landlock runtime with the given configuration.
    pub fn new(config: LandlockConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ContainerRuntime for LandlockRuntime {
    fn name(&self) -> &str {
        "landlock"
    }

    /// Always returns true -- availability is checked at exec time via kernel ABI negotiation.
    async fn is_available(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        command: &str,
        config: &ContainerConfig,
    ) -> RuntimeResult<CommandOutput> {
        let config_clone = config.clone();
        let ll_config = self.config.clone();
        let command = command.to_string();

        tokio::task::spawn_blocking(move || {
            execute_with_landlock(&command, &config_clone, &ll_config)
        })
        .await
        .map_err(|e| RuntimeError::ExecutionFailed(format!("spawn_blocking join error: {e}")))?
    }
}

/// Execute a shell command inside a Landlock-restricted child process.
///
/// When the `sandbox-landlock` feature is disabled, returns `NotAvailable`.
/// When enabled, applies Landlock rules in the child via `pre_exec` so the
/// parent process is never restricted.
#[cfg(not(feature = "sandbox-landlock"))]
fn execute_with_landlock(
    _command: &str,
    _config: &ContainerConfig,
    _ll_config: &LandlockConfig,
) -> RuntimeResult<CommandOutput> {
    Err(RuntimeError::NotAvailable(
        "Recompile with --features sandbox-landlock to use the Landlock runtime.".to_string(),
    ))
}

#[cfg(feature = "sandbox-landlock")]
fn execute_with_landlock(
    command: &str,
    config: &ContainerConfig,
    ll_config: &LandlockConfig,
) -> RuntimeResult<CommandOutput> {
    execute_with_landlock_inner(command, config, ll_config)
}

/// Inner implementation, only compiled when the feature is enabled.
#[cfg(feature = "sandbox-landlock")]
fn execute_with_landlock_inner(
    command: &str,
    config: &ContainerConfig,
    ll_config: &LandlockConfig,
) -> RuntimeResult<CommandOutput> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::Duration;

    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    if let Some(ref workdir) = config.workdir {
        cmd.current_dir(workdir);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    // Apply Landlock in the child process (after fork, before exec).
    // This ensures the parent ZeptoClaw process is never restricted.
    let ll_config_clone = ll_config.clone();
    // SAFETY: We only call async-signal-safe operations in the pre_exec closure.
    // `landlock::Ruleset` operations use only synchronous syscalls (landlock_create_ruleset,
    // landlock_add_rule, landlock_restrict_self, prctl) which are async-signal-safe.
    // PathFd::new calls open() which is also async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            apply_landlock_rules_in_child(&ll_config_clone).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
            })
        });
    }

    // Spawn and wait with timeout via thread + channel.
    let timeout = Duration::from_secs(config.timeout_secs);
    let child = cmd
        .spawn()
        .map_err(|e| RuntimeError::ExecutionFailed(format!("Failed to spawn command: {e}")))?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut child = child;
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(CommandOutput::new(
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status.code(),
        )),
        Ok(Err(e)) => Err(RuntimeError::ExecutionFailed(format!(
            "Command wait failed: {e}"
        ))),
        Err(_) => Err(RuntimeError::Timeout(config.timeout_secs)),
    }
}

/// Apply Landlock filesystem rules to the current process.
///
/// Called inside the child process via `pre_exec`. Restricts filesystem access
/// based on the configured read/write directory allowlists.
#[cfg(feature = "sandbox-landlock")]
fn apply_landlock_rules_in_child(config: &LandlockConfig) -> Result<(), RuntimeError> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, ABI,
    };

    let abi = ABI::V3;

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_read(abi))
        .map_err(|e| RuntimeError::ExecutionFailed(format!("Landlock ruleset read error: {e}")))?
        .handle_access(AccessFs::from_write(abi))
        .map_err(|e| RuntimeError::ExecutionFailed(format!("Landlock ruleset write error: {e}")))?
        .create()
        .map_err(|e| RuntimeError::ExecutionFailed(format!("Landlock create error: {e}")))?;

    // Grant read access to configured directories.
    for dir in &config.fs_read_dirs {
        if let Ok(fd) = PathFd::new(dir) {
            let _ = ruleset.add_rule(PathBeneath::new(fd, AccessFs::from_read(abi)));
        }
    }

    // Grant full access (read + write) to configured write directories.
    for dir in &config.fs_write_dirs {
        if let Ok(fd) = PathFd::new(dir) {
            let _ = ruleset.add_rule(PathBeneath::new(fd, AccessFs::from_all(abi)));
        }
    }

    match ruleset.restrict_self() {
        Ok(status) => {
            if status.ruleset == RulesetStatus::NotEnforced {
                // Kernel too old for Landlock -- degrade gracefully.
                // In pre_exec we cannot use tracing, so this is a silent degradation.
                // The parent process logs a warning if needed.
            }
            Ok(())
        }
        Err(e) => Err(RuntimeError::ExecutionFailed(format!(
            "Landlock restrict_self failed: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LandlockConfig;
    use crate::runtime::types::ContainerConfig;

    #[test]
    fn test_landlock_runtime_name() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        assert_eq!(rt.name(), "landlock");
    }

    #[tokio::test]
    async fn test_landlock_runtime_is_always_available() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        assert!(rt.is_available().await);
    }

    #[test]
    fn test_landlock_runtime_config_stored() {
        let mut config = LandlockConfig::default();
        config.fs_write_dirs.push("/home".to_string());
        let rt = LandlockRuntime::new(config.clone());
        assert_eq!(rt.config.fs_write_dirs, config.fs_write_dirs);
    }

    /// When compiled WITHOUT the sandbox-landlock feature, execute() returns a clear error.
    #[cfg(not(feature = "sandbox-landlock"))]
    #[tokio::test]
    async fn test_landlock_execute_without_feature_returns_error() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new();
        let result = rt.execute("echo hi", &cfg).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("sandbox-landlock"),
            "Expected error mentioning sandbox-landlock, got: {msg}"
        );
    }

    /// Only run echo test on Linux with the feature enabled (Landlock is Linux-only).
    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    #[tokio::test]
    async fn test_landlock_runtime_echo() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new();
        let out = rt.execute("echo hello", &cfg).await.unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.trim(), "hello");
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    #[tokio::test]
    async fn test_landlock_runtime_timeout() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new().with_timeout(1);
        let result = rt.execute("sleep 10", &cfg).await;
        assert!(matches!(result, Err(RuntimeError::Timeout(1))));
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    #[tokio::test]
    async fn test_landlock_runtime_with_env() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new().with_env("MY_VAR", "hello_landlock");
        let out = rt.execute("echo $MY_VAR", &cfg).await.unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.trim(), "hello_landlock");
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    #[tokio::test]
    async fn test_landlock_runtime_with_workdir() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new().with_workdir(std::path::PathBuf::from("/tmp"));
        let out = rt.execute("pwd", &cfg).await.unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("tmp"));
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    #[tokio::test]
    async fn test_landlock_runtime_exit_code() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new();
        let out = rt.execute("exit 42", &cfg).await.unwrap();
        assert!(!out.success());
        assert_eq!(out.exit_code, Some(42));
    }
}
