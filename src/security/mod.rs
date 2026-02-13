//! Security module for ZeptoClaw
//!
//! This module provides security utilities including path validation
//! and command filtering to prevent malicious tool execution.

pub mod mount;
pub mod path;
pub mod shell;

pub use mount::{validate_extra_mounts, validate_mount_not_blocked, DEFAULT_BLOCKED_PATTERNS};
pub use path::{validate_path_in_workspace, SafePath};
pub use shell::ShellSecurityConfig;
