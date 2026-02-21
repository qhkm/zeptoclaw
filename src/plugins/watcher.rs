//! Plugin directory change detection via polling.
//!
//! No `notify` crate — uses filesystem metadata polling for zero new deps.
//! Scan plugin directories on a schedule, detect new or modified `plugin.json`
//! manifests, and report changed paths for the caller to re-load.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{info, warn};

/// Tracks modification times of plugin manifests for change detection.
///
/// Call [`PluginWatcher::scan`] periodically to discover new or changed plugins
/// without watching the filesystem in real-time (no inotify/kqueue dependency).
///
/// # Example
///
/// ```rust,no_run
/// use std::path::PathBuf;
/// use zeptoclaw::plugins::PluginWatcher;
///
/// let plugin_dirs = vec![PathBuf::from("/home/user/.zeptoclaw/plugins")];
/// let mut watcher = PluginWatcher::new();
///
/// // First scan discovers all plugins.
/// let changed = watcher.scan(&plugin_dirs);
/// println!("Found {} plugin(s) on first scan", changed.len());
///
/// // Subsequent scans only return newly added or modified plugins.
/// let changed_again = watcher.scan(&plugin_dirs);
/// println!("{} plugin(s) changed since last scan", changed_again.len());
/// ```
#[derive(Debug)]
pub struct PluginWatcher {
    /// Map of plugin directory path → last known mtime of `plugin.json`.
    known_mtimes: HashMap<PathBuf, SystemTime>,
}

impl PluginWatcher {
    /// Create a new, empty watcher.
    pub fn new() -> Self {
        Self {
            known_mtimes: HashMap::new(),
        }
    }

    /// Scan plugin directories and return paths of changed or new plugins.
    ///
    /// A plugin is considered changed when:
    /// - Its `plugin.json` mtime is newer than last observed, or
    /// - It is newly discovered (not in the tracking map).
    ///
    /// Plugins whose directories have been removed are silently evicted from
    /// the tracking map so they do not reappear on future scans.
    ///
    /// Each element in `plugin_dirs` is expected to be a parent directory
    /// containing one subdirectory per plugin. Non-existent directories are
    /// skipped with a warning and do not cause an error.
    pub fn scan(&mut self, plugin_dirs: &[PathBuf]) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        let mut current_paths: HashMap<PathBuf, SystemTime> = HashMap::new();

        for dir in plugin_dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(err) => {
                    warn!(
                        dir = %dir.display(),
                        error = %err,
                        "Could not read plugin directory; skipping"
                    );
                    continue;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let manifest = path.join("plugin.json");
                if !manifest.exists() {
                    continue;
                }

                let mtime = match std::fs::metadata(&manifest).and_then(|m| m.modified()) {
                    Ok(t) => t,
                    Err(err) => {
                        warn!(
                            manifest = %manifest.display(),
                            error = %err,
                            "Could not read plugin manifest metadata; skipping"
                        );
                        continue;
                    }
                };

                current_paths.insert(path.clone(), mtime);

                match self.known_mtimes.get(&path) {
                    None => {
                        info!(plugin = %path.display(), "New plugin detected");
                        changed.push(path);
                    }
                    Some(old_mtime) if *old_mtime != mtime => {
                        info!(plugin = %path.display(), "Plugin changed (manifest mtime updated)");
                        changed.push(path);
                    }
                    _ => {} // unchanged
                }
            }
        }

        // Replace tracking map — entries for removed plugins are dropped here.
        self.known_mtimes = current_paths;

        changed
    }

    /// Number of plugin directories currently being tracked.
    pub fn tracked_count(&self) -> usize {
        self.known_mtimes.len()
    }
}

impl Default for PluginWatcher {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Plugin binary health check
// ---------------------------------------------------------------------------

/// Check whether a binary plugin is present and executable.
///
/// This is a lightweight, synchronous check — it does not spawn a process or
/// send a JSON-RPC ping. Use it to gate tool registration so dead binaries are
/// not offered to the LLM.
///
/// Returns `true` when the file exists **and** is executable (Unix: any exec
/// bit set). On non-Unix platforms the exec-bit check is skipped.
///
/// # Example
///
/// ```rust,no_run
/// use std::path::Path;
/// use zeptoclaw::plugins::check_binary_health;
///
/// let healthy = check_binary_health(Path::new("/usr/local/bin/my-plugin"));
/// assert!(healthy, "plugin binary should be available");
/// ```
pub fn check_binary_health(binary_path: &Path) -> bool {
    binary_path.exists() && binary_path.is_file() && is_executable(binary_path)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // On Windows, any file with an executable extension is assumed runnable.
    // We cannot check exec bits, so we just confirm the file exists.
    path.exists()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_watcher_new_is_empty() {
        let watcher = PluginWatcher::new();
        assert_eq!(watcher.tracked_count(), 0);
    }

    #[test]
    fn test_watcher_detects_new_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.json"), r#"{"name":"test"}"#).unwrap();

        let mut watcher = PluginWatcher::new();
        let changed = watcher.scan(&[dir.path().to_path_buf()]);

        assert_eq!(changed.len(), 1);
        assert_eq!(watcher.tracked_count(), 1);
    }

    #[test]
    fn test_watcher_no_change_second_scan() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.json"), r#"{"name":"test"}"#).unwrap();

        let mut watcher = PluginWatcher::new();
        let _ = watcher.scan(&[dir.path().to_path_buf()]);
        let changed = watcher.scan(&[dir.path().to_path_buf()]);

        assert_eq!(changed.len(), 0, "Second scan should find no changes");
    }

    #[test]
    fn test_watcher_detects_modified_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        let manifest = plugin_dir.join("plugin.json");
        fs::write(&manifest, r#"{"name":"test","version":"1.0"}"#).unwrap();

        let mut watcher = PluginWatcher::new();
        let _ = watcher.scan(&[dir.path().to_path_buf()]);

        // Wait long enough for the filesystem clock to tick (HFS+/APFS have
        // 1-second mtime resolution on some configurations).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&manifest, r#"{"name":"test","version":"2.0"}"#).unwrap();

        let changed = watcher.scan(&[dir.path().to_path_buf()]);
        assert_eq!(changed.len(), 1, "Should detect modified plugin");
    }

    #[test]
    fn test_watcher_ignores_non_plugin_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Directory without plugin.json
        let non_plugin = dir.path().join("not-a-plugin");
        fs::create_dir(&non_plugin).unwrap();
        fs::write(non_plugin.join("README.md"), "not a plugin").unwrap();

        let mut watcher = PluginWatcher::new();
        let changed = watcher.scan(&[dir.path().to_path_buf()]);

        assert_eq!(changed.len(), 0);
        assert_eq!(watcher.tracked_count(), 0);
    }

    #[test]
    fn test_watcher_handles_missing_dir() {
        let mut watcher = PluginWatcher::new();
        let changed = watcher.scan(&[PathBuf::from("/nonexistent/plugins")]);
        assert_eq!(changed.len(), 0);
    }

    #[test]
    fn test_watcher_evicts_removed_plugins() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.json"), r#"{"name":"test"}"#).unwrap();

        let mut watcher = PluginWatcher::new();
        let _ = watcher.scan(&[dir.path().to_path_buf()]);
        assert_eq!(watcher.tracked_count(), 1);

        // Remove the plugin directory.
        fs::remove_dir_all(&plugin_dir).unwrap();
        let _ = watcher.scan(&[dir.path().to_path_buf()]);

        assert_eq!(watcher.tracked_count(), 0, "Removed plugin should be evicted");
    }

    #[test]
    fn test_check_binary_health_missing() {
        assert!(!check_binary_health(Path::new("/nonexistent/binary")));
    }

    #[test]
    fn test_check_binary_health_directory() {
        let dir = tempfile::tempdir().unwrap();
        // Directories are not files — should return false.
        assert!(!check_binary_health(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn test_check_binary_health_non_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-exec");
        fs::write(&file, "data").unwrap();
        // Clear all executable bits.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();

        assert!(!check_binary_health(&file), "Non-executable file should fail health check");
    }

    #[cfg(unix)]
    #[test]
    fn test_check_binary_health_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("exec-bin");
        fs::write(&file, "#!/bin/sh\necho ok").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(check_binary_health(&file), "Executable file should pass health check");
    }

    #[test]
    fn test_watcher_default_same_as_new() {
        let w1 = PluginWatcher::new();
        let w2 = PluginWatcher::default();
        assert_eq!(w1.tracked_count(), w2.tracked_count());
    }

    #[test]
    fn test_watcher_multiple_dirs() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        // Plugin in dir1
        let p1 = dir1.path().join("plugin-a");
        fs::create_dir(&p1).unwrap();
        fs::write(p1.join("plugin.json"), r#"{"name":"a"}"#).unwrap();

        // Plugin in dir2
        let p2 = dir2.path().join("plugin-b");
        fs::create_dir(&p2).unwrap();
        fs::write(p2.join("plugin.json"), r#"{"name":"b"}"#).unwrap();

        let mut watcher = PluginWatcher::new();
        let changed = watcher.scan(&[dir1.path().to_path_buf(), dir2.path().to_path_buf()]);

        assert_eq!(changed.len(), 2, "Should detect plugins in both directories");
        assert_eq!(watcher.tracked_count(), 2);
    }
}
