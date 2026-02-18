//! Channel Plugin Adapter for ZeptoClaw
//!
//! This module provides a `ChannelPluginAdapter` that implements the `Channel`
//! trait by spawning an external binary and communicating via JSON-RPC 2.0 over
//! stdin/stdout. Unlike tool binary plugins (which spawn per-call), channel
//! plugins are long-running processes that persist for the lifetime of the
//! channel.
//!
//! # Plugin Directory Layout
//!
//! ```text
//! ~/.zeptoclaw/channels/
//! +-- my-channel/
//! |   +-- manifest.json
//! |   +-- my-channel-binary
//! +-- another-channel/
//!     +-- manifest.json
//!     +-- another-binary
//! ```
//!
//! # Manifest Format
//!
//! ```json
//! {
//!     "name": "my-channel",
//!     "version": "0.1.0",
//!     "description": "A custom channel plugin",
//!     "binary": "my-channel-binary",
//!     "env": {},
//!     "timeout_secs": 30
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::bus::OutboundMessage;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

// ---- JSON-RPC 2.0 types (channel-specific, not coupled to MCP or tool plugins) ----

#[derive(Serialize)]
struct ChannelJsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct ChannelJsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<ChannelJsonRpcError>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct ChannelJsonRpcError {
    code: i64,
    message: String,
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

// ---- Channel Plugin Manifest ----

/// Manifest describing a channel plugin binary.
///
/// Parsed from `manifest.json` in each plugin subdirectory under the
/// channel plugins directory (`~/.zeptoclaw/channels/` by default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPluginManifest {
    /// Unique name for this channel plugin.
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Relative path to the binary within the plugin directory.
    pub binary: String,
    /// Environment variables to set when spawning the binary.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Timeout in seconds for JSON-RPC send operations.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    30
}

// ---- Channel Plugin Adapter ----

/// Adapter that implements `Channel` by spawning an external binary plugin.
///
/// The binary communicates via JSON-RPC 2.0 over stdin/stdout. On `start()`,
/// the binary is spawned as a long-running child process. On `send()`, a
/// JSON-RPC request is written to the process's stdin. On `stop()`, the child
/// process is terminated.
pub struct ChannelPluginAdapter {
    /// The parsed manifest for this plugin.
    manifest: ChannelPluginManifest,
    /// Absolute path to the plugin directory (contains the binary).
    plugin_dir: PathBuf,
    /// Base channel configuration (name, allowlist).
    base_config: BaseChannelConfig,
    /// Atomic flag indicating if the channel is currently running.
    running: Arc<AtomicBool>,
    /// Handle to the child process stdin (for sending JSON-RPC requests).
    child_stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    /// Handle to the child process (for killing on stop).
    child_handle: Arc<Mutex<Option<tokio::process::Child>>>,
    /// Monotonically increasing JSON-RPC request ID.
    request_id: std::sync::atomic::AtomicU64,
}

impl ChannelPluginAdapter {
    /// Creates a new channel plugin adapter.
    ///
    /// # Arguments
    ///
    /// * `manifest` - The parsed plugin manifest
    /// * `plugin_dir` - Absolute path to the plugin directory
    /// * `base_config` - Base channel configuration (name, allowlist)
    pub fn new(
        manifest: ChannelPluginManifest,
        plugin_dir: PathBuf,
        base_config: BaseChannelConfig,
    ) -> Self {
        Self {
            manifest,
            plugin_dir,
            base_config,
            running: Arc::new(AtomicBool::new(false)),
            child_stdin: Arc::new(Mutex::new(None)),
            child_handle: Arc::new(Mutex::new(None)),
            request_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Returns the absolute path to the plugin binary.
    pub fn binary_path(&self) -> PathBuf {
        self.plugin_dir.join(&self.manifest.binary)
    }

    /// Returns a reference to the manifest.
    pub fn manifest(&self) -> &ChannelPluginManifest {
        &self.manifest
    }

    /// Returns the next JSON-RPC request ID.
    fn next_id(&self) -> u64 {
        self.request_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl std::fmt::Debug for ChannelPluginAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelPluginAdapter")
            .field("name", &self.manifest.name)
            .field("version", &self.manifest.version)
            .field("binary", &self.binary_path())
            .field("running", &self.running.load(Ordering::SeqCst))
            .finish()
    }
}

#[async_trait]
impl Channel for ChannelPluginAdapter {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    async fn start(&mut self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            info!("Channel plugin '{}' already running", self.manifest.name);
            return Ok(());
        }

        let binary_path = self.binary_path();

        if !binary_path.exists() {
            self.running.store(false, Ordering::SeqCst);
            return Err(ZeptoError::Channel(format!(
                "Channel plugin binary not found: {}",
                binary_path.display()
            )));
        }

        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&self.plugin_dir);

        // Set environment variables from manifest
        for (key, value) in &self.manifest.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            self.running.store(false, Ordering::SeqCst);
            ZeptoError::Channel(format!(
                "Failed to spawn channel plugin '{}' ({}): {}",
                self.manifest.name,
                binary_path.display(),
                e
            ))
        })?;

        // Take ownership of stdin for sending JSON-RPC requests
        let stdin = child.stdin.take();

        {
            let mut stdin_lock = self.child_stdin.lock().await;
            *stdin_lock = stdin;
        }
        {
            let mut child_lock = self.child_handle.lock().await;
            *child_lock = Some(child);
        }

        info!(
            "Channel plugin '{}' v{} started (pid: running)",
            self.manifest.name, self.manifest.version
        );

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            info!("Channel plugin '{}' already stopped", self.manifest.name);
            return Ok(());
        }

        info!("Stopping channel plugin '{}'", self.manifest.name);

        // Drop stdin to signal EOF to the child
        {
            let mut stdin_lock = self.child_stdin.lock().await;
            *stdin_lock = None;
        }

        // Kill the child process
        {
            let mut child_lock = self.child_handle.lock().await;
            if let Some(ref mut child) = *child_lock {
                if let Err(e) = child.kill().await {
                    warn!(
                        "Failed to kill channel plugin '{}': {}",
                        self.manifest.name, e
                    );
                }
            }
            *child_lock = None;
        }

        info!("Channel plugin '{}' stopped", self.manifest.name);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ZeptoError::Channel(format!(
                "Channel plugin '{}' not running",
                self.manifest.name
            )));
        }

        let request = ChannelJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "send".to_string(),
            params: serde_json::json!({
                "channel": msg.channel,
                "chat_id": msg.chat_id,
                "content": msg.content,
            }),
        };

        let mut request_json = serde_json::to_string(&request).map_err(|e| {
            ZeptoError::Channel(format!(
                "Failed to serialize JSON-RPC request for channel plugin '{}': {}",
                self.manifest.name, e
            ))
        })?;
        request_json.push('\n');

        let mut stdin_lock = self.child_stdin.lock().await;
        match stdin_lock.as_mut() {
            Some(stdin) => {
                use tokio::io::AsyncWriteExt;
                stdin
                    .write_all(request_json.as_bytes())
                    .await
                    .map_err(|e| {
                        ZeptoError::Channel(format!(
                            "Failed to write to channel plugin '{}' stdin: {}",
                            self.manifest.name, e
                        ))
                    })?;
                stdin.flush().await.map_err(|e| {
                    ZeptoError::Channel(format!(
                        "Failed to flush channel plugin '{}' stdin: {}",
                        self.manifest.name, e
                    ))
                })?;
                Ok(())
            }
            None => Err(ZeptoError::Channel(format!(
                "Channel plugin '{}' stdin not available",
                self.manifest.name
            ))),
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
    }
}

// ---- Plugin Discovery ----

/// Discovers channel plugins in the given directory.
///
/// Scans subdirectories for `manifest.json` files, parses them, and returns
/// a list of valid manifests along with their directory paths. Invalid
/// manifests are logged as warnings and skipped.
///
/// # Arguments
///
/// * `plugin_dir` - The directory to scan (e.g., `~/.zeptoclaw/channels/`)
///
/// # Returns
///
/// A vector of `(manifest, plugin_directory_path)` tuples.
pub fn discover_channel_plugins(plugin_dir: &Path) -> Vec<(ChannelPluginManifest, PathBuf)> {
    let mut plugins = Vec::new();

    let entries = match std::fs::read_dir(plugin_dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "Failed to read channel plugins directory {}: {}",
                    plugin_dir.display(),
                    e
                );
            }
            return plugins;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read directory entry: {}", e);
                continue;
            }
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            warn!(
                "Channel plugin directory '{}' missing manifest.json, skipping",
                path.display()
            );
            continue;
        }

        let manifest_content = match std::fs::read_to_string(&manifest_path) {
            Ok(content) => content,
            Err(e) => {
                warn!(
                    "Failed to read manifest at {}: {}",
                    manifest_path.display(),
                    e
                );
                continue;
            }
        };

        let manifest: ChannelPluginManifest = match serde_json::from_str(&manifest_content) {
            Ok(m) => m,
            Err(e) => {
                warn!("Invalid manifest at {}: {}", manifest_path.display(), e);
                continue;
            }
        };

        // Validate that the binary path does not escape the plugin directory.
        // Reject any binary field containing ".." components — Path::starts_with()
        // is component-based and does NOT resolve traversals, so join("../x")
        // would pass starts_with() despite escaping the directory.
        if Path::new(&manifest.binary)
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            warn!(
                "Channel plugin '{}' binary path contains traversal, skipping",
                manifest.name
            );
            continue;
        }
        let binary_path = path.join(&manifest.binary);
        if !binary_path.starts_with(&path) {
            warn!(
                "Channel plugin '{}' binary path escapes plugin directory, skipping",
                manifest.name
            );
            continue;
        }

        info!(
            "Discovered channel plugin: {} v{} ({})",
            manifest.name,
            manifest.version,
            path.display()
        );
        plugins.push((manifest, path));
    }

    plugins
}

/// Returns the default channel plugins directory (`~/.zeptoclaw/channels/`).
pub fn default_channel_plugins_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".zeptoclaw").join("channels"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    // ---- Manifest parsing tests ----

    #[test]
    fn test_manifest_parse_valid() {
        let json = r#"{
            "name": "test-channel",
            "version": "1.0.0",
            "description": "A test channel plugin",
            "binary": "test-binary",
            "env": {"API_KEY": "secret"},
            "timeout_secs": 60
        }"#;
        let manifest: ChannelPluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "test-channel");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.description, "A test channel plugin");
        assert_eq!(manifest.binary, "test-binary");
        assert_eq!(manifest.env.get("API_KEY").unwrap(), "secret");
        assert_eq!(manifest.timeout_secs, 60);
    }

    #[test]
    fn test_manifest_parse_minimal() {
        let json = r#"{
            "name": "minimal",
            "version": "0.1.0",
            "description": "Minimal plugin",
            "binary": "plugin"
        }"#;
        let manifest: ChannelPluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "minimal");
        assert!(manifest.env.is_empty());
        assert_eq!(manifest.timeout_secs, 30); // default
    }

    #[test]
    fn test_manifest_parse_missing_required_field() {
        let json = r#"{
            "name": "missing-binary",
            "version": "0.1.0",
            "description": "Missing binary field"
        }"#;
        let result: std::result::Result<ChannelPluginManifest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_parse_extra_fields_ignored() {
        let json = r#"{
            "name": "extra",
            "version": "0.1.0",
            "description": "Has extra fields",
            "binary": "plugin",
            "unknown_field": true,
            "another": 42
        }"#;
        let manifest: ChannelPluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "extra");
    }

    #[test]
    fn test_manifest_serialization_roundtrip() {
        let manifest = ChannelPluginManifest {
            name: "roundtrip".to_string(),
            version: "1.2.3".to_string(),
            description: "Roundtrip test".to_string(),
            binary: "my-binary".to_string(),
            env: {
                let mut m = HashMap::new();
                m.insert("KEY".to_string(), "VALUE".to_string());
                m
            },
            timeout_secs: 45,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ChannelPluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "roundtrip");
        assert_eq!(parsed.timeout_secs, 45);
        assert_eq!(parsed.env.get("KEY").unwrap(), "VALUE");
    }

    // ---- Adapter construction and properties ----

    fn test_manifest() -> ChannelPluginManifest {
        ChannelPluginManifest {
            name: "test-plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "Test plugin".to_string(),
            binary: "test-binary".to_string(),
            env: HashMap::new(),
            timeout_secs: 30,
        }
    }

    #[test]
    fn test_adapter_name() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp/plugins/test"),
            BaseChannelConfig::new("test-plugin"),
        );
        assert_eq!(adapter.name(), "test-plugin");
    }

    #[test]
    fn test_adapter_binary_path() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp/plugins/test"),
            BaseChannelConfig::new("test-plugin"),
        );
        assert_eq!(
            adapter.binary_path(),
            PathBuf::from("/tmp/plugins/test/test-binary")
        );
    }

    #[test]
    fn test_adapter_not_running_initially() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp/plugins/test"),
            BaseChannelConfig::new("test-plugin"),
        );
        assert!(!adapter.is_running());
    }

    #[test]
    fn test_adapter_manifest_accessor() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp/plugins/test"),
            BaseChannelConfig::new("test-plugin"),
        );
        assert_eq!(adapter.manifest().version, "0.1.0");
        assert_eq!(adapter.manifest().description, "Test plugin");
    }

    // ---- is_allowed delegation ----

    #[test]
    fn test_adapter_is_allowed_empty_allowlist() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp"),
            BaseChannelConfig::new("test"),
        );
        assert!(adapter.is_allowed("anyone"));
        assert!(adapter.is_allowed("user123"));
    }

    #[test]
    fn test_adapter_is_allowed_with_allowlist() {
        let base = BaseChannelConfig::with_allowlist(
            "test",
            vec!["user1".to_string(), "user2".to_string()],
        );
        let adapter = ChannelPluginAdapter::new(test_manifest(), PathBuf::from("/tmp"), base);
        assert!(adapter.is_allowed("user1"));
        assert!(adapter.is_allowed("user2"));
        assert!(!adapter.is_allowed("user3"));
    }

    #[test]
    fn test_adapter_is_allowed_deny_by_default() {
        let base = BaseChannelConfig {
            name: "test".to_string(),
            allowlist: vec![],
            deny_by_default: true,
        };
        let adapter = ChannelPluginAdapter::new(test_manifest(), PathBuf::from("/tmp"), base);
        assert!(!adapter.is_allowed("anyone"));
    }

    // ---- Send JSON-RPC serialization ----

    #[test]
    fn test_send_jsonrpc_request_structure() {
        let request = ChannelJsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "send".to_string(),
            params: json!({
                "channel": "my-channel",
                "chat_id": "chat123",
                "content": "Hello world",
            }),
        };
        let json_val = serde_json::to_value(&request).unwrap();
        assert_eq!(json_val["jsonrpc"], "2.0");
        assert_eq!(json_val["id"], 1);
        assert_eq!(json_val["method"], "send");
        assert_eq!(json_val["params"]["channel"], "my-channel");
        assert_eq!(json_val["params"]["chat_id"], "chat123");
        assert_eq!(json_val["params"]["content"], "Hello world");
    }

    #[test]
    fn test_jsonrpc_response_success() {
        let json_str = r#"{"jsonrpc":"2.0","result":{"status":"sent"},"id":1}"#;
        let resp: ChannelJsonRpcResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_jsonrpc_response_error() {
        let json_str =
            r#"{"jsonrpc":"2.0","error":{"code":-1,"message":"delivery failed"},"id":1}"#;
        let resp: ChannelJsonRpcResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -1);
        assert_eq!(err.message, "delivery failed");
    }

    // ---- Send when not running ----

    #[tokio::test]
    async fn test_send_when_not_running() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp"),
            BaseChannelConfig::new("test"),
        );
        let msg = OutboundMessage::new("test", "chat1", "Hello");
        let result = adapter.send(msg).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not running"), "err was: {}", err);
    }

    // ---- Start with invalid binary ----

    #[tokio::test]
    async fn test_start_binary_not_found() {
        let mut adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/nonexistent/path"),
            BaseChannelConfig::new("test"),
        );
        let result = adapter.start().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found") || err.contains("Failed to spawn"),
            "err was: {}",
            err
        );
        assert!(!adapter.is_running());
    }

    // ---- Discovery tests ----

    #[test]
    fn test_discover_empty_directory() {
        let dir = TempDir::new().unwrap();
        let plugins = discover_channel_plugins(dir.path());
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_nonexistent_directory() {
        let plugins = discover_channel_plugins(Path::new("/nonexistent/dir/xyz"));
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_valid_plugin() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("my-channel");
        std::fs::create_dir(&plugin_dir).unwrap();

        let manifest = json!({
            "name": "my-channel",
            "version": "0.1.0",
            "description": "Test channel",
            "binary": "my-binary"
        });
        std::fs::write(
            plugin_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        // Create a dummy binary file so path validation works
        std::fs::write(plugin_dir.join("my-binary"), "#!/bin/sh\n").unwrap();

        let plugins = discover_channel_plugins(dir.path());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].0.name, "my-channel");
        assert_eq!(plugins[0].1, plugin_dir);
    }

    #[test]
    fn test_discover_skips_invalid_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("bad-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        // Write invalid JSON
        std::fs::write(plugin_dir.join("manifest.json"), "{ invalid json }").unwrap();

        let plugins = discover_channel_plugins(dir.path());
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_skips_missing_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("no-manifest");
        std::fs::create_dir(&plugin_dir).unwrap();
        // No manifest.json created

        let plugins = discover_channel_plugins(dir.path());
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_skips_files_only_processes_dirs() {
        let dir = TempDir::new().unwrap();
        // Create a file, not a directory
        std::fs::write(dir.path().join("not-a-dir.txt"), "hello").unwrap();

        let plugins = discover_channel_plugins(dir.path());
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_multiple_plugins() {
        let dir = TempDir::new().unwrap();

        for name in &["plugin-a", "plugin-b", "plugin-c"] {
            let plugin_dir = dir.path().join(name);
            std::fs::create_dir(&plugin_dir).unwrap();
            let manifest = json!({
                "name": name,
                "version": "0.1.0",
                "description": format!("{} plugin", name),
                "binary": "binary"
            });
            std::fs::write(
                plugin_dir.join("manifest.json"),
                serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();
        }

        let plugins = discover_channel_plugins(dir.path());
        assert_eq!(plugins.len(), 3);
    }

    #[test]
    fn test_discover_skips_path_traversal_binary() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("evil-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        let manifest = json!({
            "name": "evil-plugin",
            "version": "0.1.0",
            "description": "Tries to escape",
            "binary": "../../../etc/cron.d/evil"
        });
        std::fs::write(
            plugin_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let plugins = discover_channel_plugins(dir.path());
        assert!(plugins.is_empty(), "path traversal binary should be rejected");
    }

    // ---- Debug formatting ----

    #[test]
    fn test_adapter_debug_format() {
        let adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp/plugins/test"),
            BaseChannelConfig::new("test-plugin"),
        );
        let debug = format!("{:?}", adapter);
        assert!(debug.contains("test-plugin"));
        assert!(debug.contains("0.1.0"));
    }

    // ---- Default channel plugins dir ----

    #[test]
    fn test_default_channel_plugins_dir() {
        let dir = default_channel_plugins_dir();
        // Should return Some on systems with a home directory
        if let Some(d) = dir {
            assert!(d.ends_with("channels"));
            assert!(d.to_string_lossy().contains(".zeptoclaw"));
        }
    }

    // ---- Double-start idempotency ----

    #[tokio::test]
    async fn test_double_start_idempotent() {
        // With a nonexistent binary, first start fails.
        // But if we manually set running to true, second start should be a no-op.
        let mut adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp"),
            BaseChannelConfig::new("test"),
        );
        adapter.running.store(true, Ordering::SeqCst);
        let result = adapter.start().await;
        assert!(result.is_ok()); // idempotent, already running
        assert!(adapter.is_running());
    }

    // ---- Stop when not running ----

    #[tokio::test]
    async fn test_stop_when_not_running() {
        let mut adapter = ChannelPluginAdapter::new(
            test_manifest(),
            PathBuf::from("/tmp"),
            BaseChannelConfig::new("test"),
        );
        let result = adapter.stop().await;
        assert!(result.is_ok());
        assert!(!adapter.is_running());
    }

    // ---- Start and stop with real binary ----

    #[cfg(unix)]
    #[tokio::test]
    async fn test_start_and_stop_real_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let binary_path = dir.path().join("channel-plugin");
        // Write a simple script that reads stdin and stays alive
        std::fs::write(
            &binary_path,
            "#!/bin/sh\nwhile read line; do echo '{\"jsonrpc\":\"2.0\",\"result\":{\"status\":\"ok\"},\"id\":1}'; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let manifest = ChannelPluginManifest {
            name: "real-test".to_string(),
            version: "0.1.0".to_string(),
            description: "Real test".to_string(),
            binary: "channel-plugin".to_string(),
            env: HashMap::new(),
            timeout_secs: 30,
        };

        let mut adapter = ChannelPluginAdapter::new(
            manifest,
            dir.path().to_path_buf(),
            BaseChannelConfig::new("real-test"),
        );

        // Start
        let result = adapter.start().await;
        assert!(result.is_ok(), "start failed: {:?}", result);
        assert!(adapter.is_running());

        // Send a message
        let msg = OutboundMessage::new("real-test", "chat1", "Hello from test");
        let send_result = adapter.send(msg).await;
        assert!(send_result.is_ok(), "send failed: {:?}", send_result);

        // Stop
        let stop_result = adapter.stop().await;
        assert!(stop_result.is_ok(), "stop failed: {:?}", stop_result);
        assert!(!adapter.is_running());
    }
}
