# Container Isolation Implementation Changelog

> Implementation completed: 2026-02-13

## Overview

Added selectable container runtime support to ZeptoClaw, allowing shell commands to be executed in isolated containers (Docker, Apple Container) or natively. This makes container isolation the primary security mechanism while keeping application-level security as defense-in-depth.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         ShellTool                               │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │ SecurityConfig  │  │ ContainerConfig │  │    Runtime      │  │
│  │ (blocklist)     │  │ (image, mounts) │  │ (trait object)  │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
┌───────────────┐       ┌───────────────┐       ┌───────────────┐
│ NativeRuntime │       │ DockerRuntime │       │ AppleRuntime  │
│ sh -c "cmd"   │       │ docker run... │       │ container...  │
└───────────────┘       └───────────────┘       └───────────────┘
```

## Commits

| Commit | Description |
|--------|-------------|
| `45df6e9` | feat(config): add container runtime configuration types |
| `b31887d` | feat(runtime): add container runtime types and trait |
| `c18c1ac` | feat(runtime): implement native runtime |
| `4f38f4d` | feat(runtime): implement Docker runtime |
| `a3ed356` | feat(runtime): implement Apple Container runtime for macOS |
| `1ec0f42` | feat(runtime): add runtime factory for configuration-based creation |
| `2922236` | feat(lib): export complete runtime module public API |
| `b921e08` | feat(tools): update ShellTool to use container runtime abstraction |
| `2781a7b` | feat(main): add runtime configuration and selection to onboard |
| `535046e` | test: add runtime integration tests |
| `1b44a6e` | chore: fix code formatting |
| `3da5dcd` | docs: add container isolation implementation changelog |
| `788926b` | fix: address code review findings |
| `eceeea4` | docs: update changelog with code review fixes |
| `ee274e6` | fix(runtime): add functional validation to Apple runtime availability check |

## Code Review Fixes

### Finding 1 (Medium): extra_mounts was a no-op

**Problem:** `extra_mounts` was defined in `DockerConfig` and `AppleContainerConfig` but never applied when executing commands.

**Fix:**
- Added `extra_mounts: Vec<String>` field to `DockerRuntime` struct
- Added `with_extra_mounts()` builder method to `DockerRuntime`
- Applied extra mounts in `DockerRuntime::execute()` using `-v` flag
- Added same support to `AppleContainerRuntime`
- Updated factory to pass `extra_mounts` from config to runtimes

### Finding 2 (Medium): Apple runtime CLI compatibility risk

**Problem:** Apple Container runtime is based on assumed CLI interface, may fail at runtime even if availability check passes. Original `is_available()` only checked `container --version`.

**Fix (Phase 1 - Warnings):**
- Added prominent warning in module-level documentation
- Added warning in struct documentation
- Added runtime warning log in `execute()` method

**Fix (Phase 2 - Functional Validation):**
- Enhanced `is_available()` with two-step validation:
  1. Check `container --version` succeeds (tool exists)
  2. Check `container run --help` succeeds (CLI syntax compatible)
- If syntax check fails, `is_available()` returns false with warning
- This ensures fail-fast at `create_runtime()` rather than confusing errors at execution time

### Already Implemented: Opt-in native fallback

The following was already implemented in the initial implementation:
- `allow_fallback_to_native` field in `RuntimeConfig` (default: `false`)
- `create_agent()` only falls back when opted in, otherwise errors
- Onboarding prompts for fallback choice on Docker/Apple
- Status command displays fallback mode

## Files Changed

### New Files Created

| File | Purpose |
|------|---------|
| `src/runtime/mod.rs` | Runtime module declaration and exports |
| `src/runtime/types.rs` | ContainerRuntime trait, RuntimeError, CommandOutput, ContainerConfig |
| `src/runtime/native.rs` | NativeRuntime implementation (direct execution) |
| `src/runtime/docker.rs` | DockerRuntime implementation (Docker container isolation) |
| `src/runtime/apple.rs` | AppleContainerRuntime implementation (macOS 15+ only) |
| `src/runtime/factory.rs` | Runtime factory functions (create_runtime, available_runtimes) |
| `docs/plans/2026-02-13-container-isolation.md` | Implementation plan document |

### Modified Files

| File | Changes |
|------|---------|
| `src/config/types.rs` | Added RuntimeType, RuntimeConfig, DockerConfig, AppleContainerConfig |
| `src/lib.rs` | Added runtime module and exports |
| `src/tools/shell.rs` | Updated to use ContainerRuntime abstraction |
| `src/main.rs` | Added runtime configuration and onboarding |
| `tests/integration.rs` | Added runtime integration tests |

## New Types

### Configuration Types (`src/config/types.rs`)

```rust
/// Container runtime type for shell command execution
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeType {
    #[default]
    Native,
    Docker,
    #[serde(rename = "apple")]
    AppleContainer,
}

/// Runtime configuration for shell execution
pub struct RuntimeConfig {
    pub runtime_type: RuntimeType,
    pub docker: DockerConfig,
    pub apple: AppleContainerConfig,
}

/// Docker runtime configuration
pub struct DockerConfig {
    pub image: String,           // default: "alpine:latest"
    pub extra_mounts: Vec<String>,
    pub memory_limit: Option<String>,  // default: "512m"
    pub cpu_limit: Option<String>,     // default: "1.0"
    pub network: String,               // default: "none"
}

/// Apple Container runtime configuration (macOS only)
pub struct AppleContainerConfig {
    pub image: String,
    pub extra_mounts: Vec<String>,
}
```

### Runtime Types (`src/runtime/types.rs`)

```rust
/// Errors that can occur during runtime operations
#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Runtime not available: {0}")]
    NotAvailable(String),
    #[error("Failed to start container: {0}")]
    StartFailed(String),
    #[error("Command execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Command timed out after {0} seconds")]
    Timeout(u64),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Output from a command execution
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Configuration for a container execution
pub struct ContainerConfig {
    pub workdir: Option<PathBuf>,
    pub mounts: Vec<(PathBuf, PathBuf, bool)>,  // (host, container, readonly)
    pub env: Vec<(String, String)>,
    pub timeout_secs: u64,
}

/// Trait for container runtimes
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    fn name(&self) -> &str;
    async fn is_available(&self) -> bool;
    async fn execute(&self, command: &str, config: &ContainerConfig) -> RuntimeResult<CommandOutput>;
}
```

## New Public API

### From `zeptoclaw` crate

```rust
// Runtime types
pub use runtime::{
    available_runtimes,
    create_runtime,
    CommandOutput,
    ContainerConfig,
    ContainerRuntime,
    DockerRuntime,
    NativeRuntime,
    RuntimeError,
    RuntimeResult,
};

#[cfg(target_os = "macos")]
pub use runtime::AppleContainerRuntime;

// Config types
pub use config::{RuntimeConfig, RuntimeType, DockerConfig, AppleContainerConfig};
```

### ShellTool New Methods

```rust
impl ShellTool {
    /// Create with a specific container runtime
    pub fn with_runtime(runtime: Arc<dyn ContainerRuntime>) -> Self;

    /// Create with both custom security and runtime
    pub fn with_security_and_runtime(
        security_config: ShellSecurityConfig,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self;

    /// Get the runtime name
    pub fn runtime_name(&self) -> &str;
}
```

## Configuration Examples

### Native Runtime (Default)

```json
{
  "runtime": {
    "runtime_type": "native"
  }
}
```

### Docker Runtime

```json
{
  "runtime": {
    "runtime_type": "docker",
    "docker": {
      "image": "alpine:latest",
      "memory_limit": "512m",
      "cpu_limit": "1.0",
      "network": "none"
    }
  }
}
```

### Apple Container Runtime (macOS 15+)

```json
{
  "runtime": {
    "runtime_type": "apple",
    "apple": {
      "image": "/path/to/container/bundle"
    }
  }
}
```

## CLI Changes

### Onboarding (`zeptoclaw onboard`)

Now includes runtime selection:

```
=== Runtime Configuration ===
Choose container runtime for shell command isolation:
  1. Native (no container, uses application-level security)
  2. Docker (requires Docker installed)
  3. Apple Container (macOS 15+ only)  [macOS only]

Enter choice [1]:
```

### Status (`zeptoclaw status`)

Now shows runtime information:

```
Runtime
-------
  Type: Native
  Available: native, docker
```

## Test Coverage

### New Unit Tests

| Module | Tests |
|--------|-------|
| `runtime::types` | 8 tests (CommandOutput, ContainerConfig) |
| `runtime::native` | 8 tests (execution, timeout, workdir, env) |
| `runtime::docker` | 4 unit + 3 integration tests |
| `runtime::apple` | 3 unit + 2 conditional tests |
| `runtime::factory` | 2 tests |
| `tools::shell` | 5 new tests for runtime support |

### New Integration Tests

| Test | Purpose |
|------|---------|
| `test_runtime_factory_native` | Factory creates native runtime |
| `test_available_runtimes_includes_native` | Native always available |
| `test_shell_tool_with_native_runtime` | ShellTool with runtime injection |
| `test_shell_tool_runtime_with_workspace` | ShellTool + runtime + workspace |
| `test_config_runtime_serialization` | RuntimeConfig serialization |

## Security Model

### Defense in Depth

1. **Primary: Container Isolation** (Docker/Apple Container)
   - Process isolation
   - Filesystem isolation (only mounted paths accessible)
   - Network isolation (default: none)
   - Resource limits (memory, CPU)

2. **Secondary: Application-Level Security** (ShellSecurityConfig)
   - Command blocklist (rm -rf, etc.)
   - Pattern-based blocking
   - Enabled even when using container runtimes

### Runtime Availability Fallback

If configured runtime is unavailable, falls back to native with warning:

```rust
let runtime = match create_runtime(&config.runtime).await {
    Ok(r) => r,
    Err(e) => {
        warn!("Failed to create configured runtime: {}. Falling back to native.", e);
        Arc::new(NativeRuntime::new())
    }
};
```

## Backward Compatibility

- All existing API preserved
- `ShellTool::new()` continues to work (uses native runtime)
- `ShellTool::permissive()` continues to work
- Default configuration uses native runtime
- No breaking changes to configuration format

## Future Improvements

1. Add resource limit configuration for Apple Container
2. Add container image pull policy for Docker
3. Add timeout on availability checks
4. Add container cleanup on timeout (docker kill)
5. Validate Apple Container CLI interface when documentation available
