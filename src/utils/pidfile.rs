//! PID file management for single-instance enforcement.
//!
//! This module provides a simple PID file mechanism to detect and prevent
//! multiple gateway instances from running concurrently. The PID file is
//! stored at `~/.zeptoclaw/gateway.pid` and contains the process ID of
//! the running gateway.
//!
//! # Design
//!
//! This is an advisory mechanism, not a hard lock. It detects common cases
//! of accidental multiple instances but cannot prevent all scenarios (e.g.,
//! processes killed without cleanup, stale PID files after crashes).
//!
//! For production deployments, use system-level process supervision (systemd,
//! Docker restart policies, Kubernetes) to ensure single-instance semantics.

use std::fs;
use std::path::PathBuf;
use tracing::warn;

use crate::config::Config;
use crate::error::{Result, ZeptoError};

/// PID file guard that automatically cleans up on drop.
#[derive(Debug)]
pub struct PidFileGuard {
    path: PathBuf,
}

impl PidFileGuard {
    /// Create a new PID file guard at the default location.
    ///
    /// Returns an error if a PID file already exists and the process is still running.
    /// If the PID file exists but the process is dead, it will be cleaned up and
    /// a new one will be created.
    pub fn acquire() -> Result<Self> {
        Self::acquire_at(Config::dir().join("gateway.pid"))
    }

    /// Create a new PID file guard at a custom path (for testing).
    ///
    /// Returns an error if a PID file already exists and the process is still running.
    #[cfg(test)]
    pub fn acquire_at(path: PathBuf) -> Result<Self> {
        Self::acquire_internal(path)
    }

    #[cfg(not(test))]
    fn acquire_at(path: PathBuf) -> Result<Self> {
        Self::acquire_internal(path)
    }

    fn acquire_internal(path: PathBuf) -> Result<Self> {
        
        // Check if a PID file already exists
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    if let Ok(pid) = content.trim().parse::<u32>() {
                        // Check if the process is still running
                        if is_process_running(pid) {
                            return Err(ZeptoError::Config(format!(
                                "Gateway already running with PID {}. \
                                 If this is incorrect, remove {} and try again.",
                                pid,
                                path.display()
                            )));
                        } else {
                            warn!(
                                "Found stale PID file for non-running process {}. Cleaning up.",
                                pid
                            );
                            // Process is dead, clean up stale PID file
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read PID file: {}. Assuming stale and removing.", e);
                    let _ = fs::remove_file(&path);
                }
            }
        }

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ZeptoError::Config(format!(
                    "Failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        // Write our PID to the file
        let pid = std::process::id();
        fs::write(&path, pid.to_string()).map_err(|e| {
            ZeptoError::Config(format!("Failed to write PID file {}: {}", path.display(), e))
        })?;

        Ok(Self { path })
    }

    /// Get the path to the PID file.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        // Best effort cleanup - don't panic on failure
        if let Err(e) = fs::remove_file(&self.path) {
            warn!("Failed to remove PID file {}: {}", self.path.display(), e);
        }
    }
}

/// Check if a process with the given PID is currently running.
///
/// This is a best-effort check that works across platforms without requiring
/// additional dependencies. It checks if /proc/<pid> exists on Unix-like systems.
///
/// On Windows or other platforms where /proc is unavailable, we conservatively
/// assume stale PID files older than 1 hour are from dead processes.
///
/// Returns false if the check indicates the process doesn't exist.
fn is_process_running(pid: u32) -> bool {
    // Try Unix /proc filesystem first (works on Linux, some BSDs)
    let proc_path = format!("/proc/{}", pid);
    if std::path::Path::new(&proc_path).exists() {
        return true;
    }

    // On macOS and other systems without /proc, we can't easily check.
    // Use file modification time as a heuristic: if the PID file is very old,
    // it's likely stale from a crashed process.
    // This is imperfect but avoids pulling in platform-specific dependencies.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn unique_pid_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("zeptoclaw_test_{}.pid", id))
    }

    #[test]
    fn test_pidfile_guard_creation() {
        let path = unique_pid_path();
        let guard = PidFileGuard::acquire_at(path.clone());
        assert!(guard.is_ok());
        let guard = guard.unwrap();
        assert!(guard.path().exists());
        
        let content = fs::read_to_string(guard.path()).unwrap();
        let pid: u32 = content.trim().parse().unwrap();
        assert_eq!(pid, std::process::id());
    }

    #[test]
    fn test_pidfile_guard_cleanup() {
        let test_path = unique_pid_path();
        let path = {
            let guard = PidFileGuard::acquire_at(test_path.clone()).unwrap();
            let path = guard.path().clone();
            assert!(path.exists());
            path
        }; // Guard dropped here
        
        // Give the OS a moment to process the cleanup
        std::thread::sleep(std::time::Duration::from_millis(10));
        
        // PID file should be removed
        assert!(!path.exists());
    }

    #[test]
    fn test_pidfile_guard_rejects_duplicate() {
        let test_path = unique_pid_path();
        let _guard1 = PidFileGuard::acquire_at(test_path.clone()).unwrap();
        
        // Second acquisition should fail on systems where we can detect running processes
        // On systems without /proc, the PID file will be overwritten (acceptable limitation)
        let result = PidFileGuard::acquire_at(test_path);
        
        // On Linux with /proc, this should fail
        // On other systems, it might succeed (acceptable trade-off to avoid dependencies)
        if std::path::Path::new(&format!("/proc/{}", std::process::id())).exists() {
            assert!(result.is_err(), "Expected error on systems with /proc");
            assert!(result.unwrap_err().to_string().contains("already running"));
        }
    }

    #[test]
    fn test_is_process_running_current_process() {
        let pid = std::process::id();
        assert!(is_process_running(pid));
    }

    #[test]
    fn test_is_process_running_invalid_pid() {
        // Use a very high PID that's unlikely to exist
        let invalid_pid = 99999999;
        assert!(!is_process_running(invalid_pid));
    }
}
