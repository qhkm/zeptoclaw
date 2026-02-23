//! Security module for ZeptoClaw
//!
//! This module provides security utilities including path validation
//! and command filtering to prevent malicious tool execution.

pub mod agent_mode;
pub mod encryption;
pub mod mount;
pub mod pairing;
pub mod path;
pub mod shell;

pub use agent_mode::{AgentMode, AgentModeConfig, CategoryPermission, ModePolicy};
pub use encryption::{is_secret_field, resolve_master_key, SecretEncryption};
pub use mount::{validate_extra_mounts, validate_mount_not_blocked, DEFAULT_BLOCKED_PATTERNS};
pub use pairing::{DeviceInfo, PairedDevice, PairingManager};
pub use path::{validate_path_in_workspace, SafePath};
pub use shell::{ShellAllowlistMode, ShellSecurityConfig};
