//! Plugin discovery and loading for ZeptoClaw
//!
//! This module handles discovering plugin directories, loading and parsing
//! `plugin.json` manifests, and validating manifest contents for safety
//! and correctness.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::audit::{log_audit_event, AuditCategory, AuditSeverity};
use crate::error::{Result, ZeptoError};

use super::types::{BinaryPluginConfig, Plugin, PluginManifest};

/// Discover plugins across multiple directories.
///
/// Scans each provided directory for subdirectories containing a `plugin.json`
/// file. Each valid plugin is loaded, validated, and returned. Invalid plugins
/// are logged as warnings but do not cause the overall discovery to fail.
///
/// # Arguments
/// * `dirs` - Slice of directory paths to scan for plugins
///
/// # Returns
/// A vector of successfully loaded and validated plugins.
///
/// # Example
///
/// ```no_run
/// use std::path::PathBuf;
/// use zeptoclaw::plugins::discover_plugins;
///
/// let dirs = vec![PathBuf::from("/home/user/.zeptoclaw/plugins")];
/// let plugins = discover_plugins(&dirs).unwrap();
/// for plugin in &plugins {
///     println!("Found plugin: {} v{}", plugin.name(), plugin.manifest.version);
/// }
/// ```
pub fn discover_plugins(dirs: &[PathBuf]) -> Result<Vec<Plugin>> {
    let mut plugins = Vec::new();

    for dir in dirs {
        if !dir.exists() {
            info!(dir = %dir.display(), "Plugin directory does not exist, skipping");
            continue;
        }

        if !dir.is_dir() {
            warn!(path = %dir.display(), "Plugin path is not a directory, skipping");
            continue;
        }

        let entries = fs::read_dir(dir).map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to read plugin directory {}: {}",
                dir.display(),
                e
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                ZeptoError::Config(format!("Failed to read directory entry: {}", e))
            })?;

            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            let manifest_path = entry_path.join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }

            match load_plugin(&entry_path) {
                Ok(plugin) => {
                    info!(
                        plugin = %plugin.name(),
                        version = %plugin.manifest.version,
                        tools = plugin.tool_count(),
                        "Discovered plugin"
                    );
                    plugins.push(plugin);
                }
                Err(e) => {
                    warn!(
                        dir = %entry_path.display(),
                        error = %e,
                        "Failed to load plugin, skipping"
                    );
                }
            }
        }
    }

    Ok(plugins)
}

/// Load a single plugin from its directory.
///
/// Reads and parses the `plugin.json` file from the given directory,
/// validates the manifest contents, and returns a `Plugin` instance.
///
/// # Arguments
/// * `dir` - Path to the plugin directory containing `plugin.json`
///
/// # Returns
/// A loaded and validated plugin, or an error if the manifest is
/// missing, malformed, or fails validation.
///
/// # Errors
/// - `ZeptoError::Config` if `plugin.json` does not exist
/// - `ZeptoError::Json` if the JSON is malformed
/// - `ZeptoError::Config` if validation fails (see `validate_manifest`)
pub fn load_plugin(dir: &Path) -> Result<Plugin> {
    let manifest_path = dir.join("plugin.json");

    if !manifest_path.exists() {
        return Err(ZeptoError::Config(format!(
            "No plugin.json found in {}",
            dir.display()
        )));
    }

    let content = fs::read_to_string(&manifest_path).map_err(|e| {
        ZeptoError::Config(format!("Failed to read {}: {}", manifest_path.display(), e))
    })?;

    let manifest: PluginManifest = serde_json::from_str(&content)?;

    validate_manifest(&manifest)?;

    Ok(Plugin::new(manifest, dir.to_path_buf()))
}

/// Validate a plugin manifest for correctness and safety.
///
/// Performs the following checks:
/// - Plugin name must be 1-64 characters, alphanumeric and hyphens only
/// - Version must be non-empty
/// - At least one tool must be defined
/// - Tool names must be alphanumeric and underscores only
/// - Command templates must not contain dangerous shell operators
///   (`&&`, `||`, `;`, `|`, backticks)
///
/// # Arguments
/// * `manifest` - The manifest to validate
///
/// # Returns
/// `Ok(())` if valid, or `ZeptoError::Config` describing the violation.
pub fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    // Validate plugin name: alphanumeric + hyphens, 1-64 chars
    let name_re = Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9\-]{0,63}$").unwrap();
    if !name_re.is_match(&manifest.name) {
        return Err(ZeptoError::Config(format!(
            "Invalid plugin name '{}': must be 1-64 alphanumeric characters and hyphens, starting with alphanumeric",
            manifest.name
        )));
    }

    // Validate version is non-empty
    if manifest.version.trim().is_empty() {
        return Err(ZeptoError::Config(format!(
            "Plugin '{}' has an empty version string",
            manifest.name
        )));
    }

    // Must have at least one tool
    if manifest.tools.is_empty() {
        return Err(ZeptoError::Config(format!(
            "Plugin '{}' must define at least one tool",
            manifest.name
        )));
    }

    // Validate execution mode
    match manifest.execution.as_str() {
        "command" => {}
        "binary" => {
            // Binary plugins must have binary config
            let bin_cfg = manifest.binary.as_ref().ok_or_else(|| {
                ZeptoError::Config(format!(
                    "Plugin '{}' has execution \"binary\" but no binary config",
                    manifest.name
                ))
            })?;

            // Only jsonrpc protocol supported
            if bin_cfg.protocol != "jsonrpc" {
                return Err(ZeptoError::Config(format!(
                    "Plugin '{}' has unsupported binary protocol '{}': only \"jsonrpc\" is supported",
                    manifest.name, bin_cfg.protocol
                )));
            }

            // Path must be non-empty
            if bin_cfg.path.trim().is_empty() {
                return Err(ZeptoError::Config(format!(
                    "Plugin '{}' has empty binary path",
                    manifest.name
                )));
            }

            // Reject path traversal and absolute paths
            if bin_cfg.path.contains("..") {
                return Err(ZeptoError::SecurityViolation(format!(
                    "Plugin '{}' binary path contains '..': path traversal not allowed",
                    manifest.name
                )));
            }

            if Path::new(&bin_cfg.path).is_absolute() {
                return Err(ZeptoError::SecurityViolation(format!(
                    "Plugin '{}' binary path must be relative, not absolute",
                    manifest.name
                )));
            }
        }
        other => {
            return Err(ZeptoError::Config(format!(
                "Plugin '{}' has unknown execution mode '{}': must be \"command\" or \"binary\"",
                manifest.name, other
            )));
        }
    }

    // Validate each tool
    let tool_name_re = Regex::new(r"^[a-zA-Z][a-zA-Z0-9_]{0,63}$").unwrap();
    for tool in &manifest.tools {
        if !tool_name_re.is_match(&tool.name) {
            return Err(ZeptoError::Config(format!(
                "Invalid tool name '{}' in plugin '{}': must be 1-64 alphanumeric characters and underscores, starting with a letter",
                tool.name, manifest.name
            )));
        }

        // Only check command safety for command-mode plugins
        if !manifest.is_binary() {
            validate_command_safety(&tool.command, &tool.name, &manifest.name)?;
        }
    }

    Ok(())
}

/// Validate binary exists, is a file, is executable, and stays within plugin dir.
///
/// Canonicalizes both paths and verifies the binary does not escape the plugin
/// directory (e.g. via symlinks). On Unix, also checks the execute permission bit.
///
/// # Arguments
/// * `plugin_dir` - The plugin's root directory
/// * `binary_config` - The binary plugin configuration containing the relative path
///
/// # Returns
/// The canonicalized absolute path to the binary, or an error.
pub fn validate_binary_path(
    plugin_dir: &Path,
    binary_config: &BinaryPluginConfig,
) -> Result<PathBuf> {
    let binary_path = plugin_dir.join(&binary_config.path);

    if !binary_path.exists() {
        return Err(ZeptoError::Config(format!(
            "Binary not found: {}",
            binary_path.display()
        )));
    }

    if !binary_path.is_file() {
        return Err(ZeptoError::Config(format!(
            "Binary path is not a file: {}",
            binary_path.display()
        )));
    }

    // Canonicalize to resolve symlinks and check containment
    let canonical_dir = plugin_dir.canonicalize().map_err(|e| {
        ZeptoError::Config(format!(
            "Failed to canonicalize plugin dir {}: {}",
            plugin_dir.display(),
            e
        ))
    })?;
    let canonical_bin = binary_path.canonicalize().map_err(|e| {
        ZeptoError::Config(format!(
            "Failed to canonicalize binary path {}: {}",
            binary_path.display(),
            e
        ))
    })?;

    if !canonical_bin.starts_with(&canonical_dir) {
        return Err(ZeptoError::SecurityViolation(format!(
            "Binary escapes plugin directory: {} is outside {}",
            canonical_bin.display(),
            canonical_dir.display()
        )));
    }

    // Check execute bit on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = binary_path.metadata().map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to read permissions for {}: {}",
                binary_path.display(),
                e
            ))
        })?;
        if perms.permissions().mode() & 0o111 == 0 {
            return Err(ZeptoError::Config(format!(
                "Binary is not executable: {}",
                binary_path.display()
            )));
        }
    }

    // SHA-256 integrity verification (when configured).
    if let Some(ref expected_hash) = binary_config.sha256 {
        let file_bytes = fs::read(&canonical_bin).map_err(|e| {
            ZeptoError::Config(format!(
                "Failed to read binary for SHA-256 check {}: {}",
                canonical_bin.display(),
                e
            ))
        })?;
        let actual_hash = hex::encode(Sha256::digest(&file_bytes));
        if !actual_hash.eq_ignore_ascii_case(expected_hash) {
            log_audit_event(
                AuditCategory::PluginIntegrity,
                AuditSeverity::Critical,
                "sha256_mismatch",
                &format!(
                    "Binary {} expected SHA-256 {} but got {}",
                    canonical_bin.display(),
                    expected_hash,
                    actual_hash
                ),
                true,
            );
            return Err(ZeptoError::SecurityViolation(format!(
                "Binary SHA-256 mismatch for {}: expected {} but got {}",
                canonical_bin.display(),
                expected_hash,
                actual_hash
            )));
        }
    }

    Ok(canonical_bin)
}

/// Check a command template for dangerous shell operators.
///
/// Rejects commands containing `&&`, `||`, `;`, `|`, or backticks
/// to prevent shell injection through plugin definitions.
fn validate_command_safety(command: &str, tool_name: &str, plugin_name: &str) -> Result<()> {
    let dangerous_patterns: &[(&str, &str)] = &[
        ("&&", "command chaining (&&)"),
        ("||", "conditional chaining (||)"),
        (";", "command separator (;)"),
        ("`", "backtick execution"),
    ];

    for (pattern, description) in dangerous_patterns {
        if command.contains(pattern) {
            return Err(ZeptoError::SecurityViolation(format!(
                "Tool '{}' in plugin '{}' contains dangerous pattern: {}",
                tool_name, plugin_name, description
            )));
        }
    }

    // Check for pipe operator. The `||` pattern is already caught above,
    // so we check for any remaining single `|` characters.
    if command.contains('|') {
        return Err(ZeptoError::SecurityViolation(format!(
            "Tool '{}' in plugin '{}' contains dangerous pattern: pipe operator (|)",
            tool_name, plugin_name
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::PluginToolDef;
    use serde_json::json;
    use tempfile::TempDir;

    /// Helper to create a valid minimal manifest for testing.
    fn valid_manifest() -> PluginManifest {
        PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            author: None,
            tools: vec![PluginToolDef {
                name: "test_tool".to_string(),
                description: "A test tool".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                command: "echo hello".to_string(),
                working_dir: None,
                timeout_secs: None,
                env: None,
            }],
            execution: "command".to_string(),
            binary: None,
        }
    }

    /// Helper to write a plugin.json file into a directory.
    fn write_plugin_json(dir: &Path, manifest: &PluginManifest) {
        let content = serde_json::to_string_pretty(manifest).unwrap();
        fs::write(dir.join("plugin.json"), content).unwrap();
    }

    // ---- discover_plugins tests ----

    #[test]
    fn test_discover_plugins_with_valid_plugins() {
        let tmp = TempDir::new().unwrap();

        // Create two plugin directories
        let plugin1_dir = tmp.path().join("git-tools");
        fs::create_dir(&plugin1_dir).unwrap();
        let mut manifest1 = valid_manifest();
        manifest1.name = "git-tools".to_string();
        write_plugin_json(&plugin1_dir, &manifest1);

        let plugin2_dir = tmp.path().join("docker-tools");
        fs::create_dir(&plugin2_dir).unwrap();
        let mut manifest2 = valid_manifest();
        manifest2.name = "docker-tools".to_string();
        manifest2.tools[0].name = "docker_ps".to_string();
        manifest2.tools[0].command = "docker ps".to_string();
        write_plugin_json(&plugin2_dir, &manifest2);

        let plugins = discover_plugins(&[tmp.path().to_path_buf()]).unwrap();
        assert_eq!(plugins.len(), 2);

        let names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();
        assert!(names.contains(&"git-tools"));
        assert!(names.contains(&"docker-tools"));
    }

    #[test]
    fn test_discover_plugins_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let plugins = discover_plugins(&[tmp.path().to_path_buf()]).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_plugins_nonexistent_directory() {
        let plugins = discover_plugins(&[PathBuf::from("/nonexistent/path/plugins")]).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_plugins_skips_files() {
        let tmp = TempDir::new().unwrap();

        // Create a regular file (not a directory)
        fs::write(tmp.path().join("not-a-dir.txt"), "hello").unwrap();

        let plugins = discover_plugins(&[tmp.path().to_path_buf()]).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_plugins_skips_dirs_without_manifest() {
        let tmp = TempDir::new().unwrap();

        // Create a directory without plugin.json
        fs::create_dir(tmp.path().join("empty-dir")).unwrap();

        let plugins = discover_plugins(&[tmp.path().to_path_buf()]).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_plugins_skips_invalid_plugins() {
        let tmp = TempDir::new().unwrap();

        // Create a valid plugin
        let valid_dir = tmp.path().join("valid-plugin");
        fs::create_dir(&valid_dir).unwrap();
        write_plugin_json(&valid_dir, &valid_manifest());

        // Create an invalid plugin (malformed JSON)
        let invalid_dir = tmp.path().join("invalid-plugin");
        fs::create_dir(&invalid_dir).unwrap();
        fs::write(invalid_dir.join("plugin.json"), "{ broken json").unwrap();

        let plugins = discover_plugins(&[tmp.path().to_path_buf()]).unwrap();
        // Should still return the valid one
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name(), "test-plugin");
    }

    #[test]
    fn test_discover_plugins_multiple_directories() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();

        let plugin_dir1 = tmp1.path().join("plugin-a");
        fs::create_dir(&plugin_dir1).unwrap();
        let mut m1 = valid_manifest();
        m1.name = "plugin-a".to_string();
        write_plugin_json(&plugin_dir1, &m1);

        let plugin_dir2 = tmp2.path().join("plugin-b");
        fs::create_dir(&plugin_dir2).unwrap();
        let mut m2 = valid_manifest();
        m2.name = "plugin-b".to_string();
        m2.tools[0].name = "other_tool".to_string();
        write_plugin_json(&plugin_dir2, &m2);

        let plugins =
            discover_plugins(&[tmp1.path().to_path_buf(), tmp2.path().to_path_buf()]).unwrap();
        assert_eq!(plugins.len(), 2);
    }

    // ---- load_plugin tests ----

    #[test]
    fn test_load_plugin_valid_manifest() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();

        let manifest = valid_manifest();
        write_plugin_json(&plugin_dir, &manifest);

        let plugin = load_plugin(&plugin_dir).unwrap();
        assert_eq!(plugin.name(), "test-plugin");
        assert_eq!(plugin.manifest.version, "1.0.0");
        assert!(plugin.enabled);
        assert_eq!(plugin.path, plugin_dir);
        assert_eq!(plugin.tool_count(), 1);
    }

    #[test]
    fn test_load_plugin_missing_plugin_json() {
        let tmp = TempDir::new().unwrap();
        let result = load_plugin(tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No plugin.json found"));
    }

    #[test]
    fn test_load_plugin_malformed_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("plugin.json"), "{ not valid json }").unwrap();

        let result = load_plugin(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_plugin_missing_required_fields() {
        let tmp = TempDir::new().unwrap();
        // Missing "tools" field
        fs::write(
            tmp.path().join("plugin.json"),
            r#"{"name": "incomplete", "version": "1.0.0", "description": "test"}"#,
        )
        .unwrap();

        let result = load_plugin(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_plugin_with_full_manifest() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("full-plugin");
        fs::create_dir(&plugin_dir).unwrap();

        let json_content = r#"{
            "name": "full-plugin",
            "version": "2.0.0",
            "description": "A fully specified plugin",
            "author": "Test Author",
            "tools": [
                {
                    "name": "tool_one",
                    "description": "First tool",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" }
                        },
                        "required": ["path"]
                    },
                    "command": "ls {{path}}",
                    "working_dir": "/tmp",
                    "timeout_secs": 5,
                    "env": { "LANG": "en_US.UTF-8" }
                },
                {
                    "name": "tool_two",
                    "description": "Second tool",
                    "parameters": { "type": "object", "properties": {} },
                    "command": "date"
                }
            ]
        }"#;

        fs::write(plugin_dir.join("plugin.json"), json_content).unwrap();

        let plugin = load_plugin(&plugin_dir).unwrap();
        assert_eq!(plugin.name(), "full-plugin");
        assert_eq!(plugin.manifest.version, "2.0.0");
        assert_eq!(plugin.manifest.author, Some("Test Author".to_string()));
        assert_eq!(plugin.tool_count(), 2);
        assert_eq!(
            plugin.manifest.tools[0].working_dir,
            Some("/tmp".to_string())
        );
        assert_eq!(plugin.manifest.tools[0].timeout_secs, Some(5));
        assert!(plugin.manifest.tools[0].env.is_some());
    }

    // ---- validate_manifest tests ----

    #[test]
    fn test_validate_manifest_valid() {
        let manifest = valid_manifest();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_manifest_empty_name() {
        let mut manifest = valid_manifest();
        manifest.name = "".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid plugin name"));
    }

    #[test]
    fn test_validate_manifest_name_with_spaces() {
        let mut manifest = valid_manifest();
        manifest.name = "bad name".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid plugin name"));
    }

    #[test]
    fn test_validate_manifest_name_with_special_chars() {
        let mut manifest = valid_manifest();
        manifest.name = "bad@name!".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_name_starting_with_hyphen() {
        let mut manifest = valid_manifest();
        manifest.name = "-bad-start".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_name_too_long() {
        let mut manifest = valid_manifest();
        manifest.name = "a".repeat(65);
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_valid_name_with_hyphens() {
        let mut manifest = valid_manifest();
        manifest.name = "my-cool-plugin-123".to_string();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_manifest_empty_version() {
        let mut manifest = valid_manifest();
        manifest.version = "  ".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty version"));
    }

    #[test]
    fn test_validate_manifest_no_tools() {
        let mut manifest = valid_manifest();
        manifest.tools = vec![];
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("at least one tool"));
    }

    #[test]
    fn test_validate_manifest_invalid_tool_name() {
        let mut manifest = valid_manifest();
        manifest.tools[0].name = "bad-tool-name".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid tool name"));
    }

    #[test]
    fn test_validate_manifest_tool_name_starting_with_number() {
        let mut manifest = valid_manifest();
        manifest.tools[0].name = "123tool".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_valid_tool_name_with_underscores() {
        let mut manifest = valid_manifest();
        manifest.tools[0].name = "my_cool_tool_v2".to_string();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_manifest_dangerous_command_and_and() {
        let mut manifest = valid_manifest();
        manifest.tools[0].command = "echo hello && rm -rf /".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("dangerous pattern"));
    }

    #[test]
    fn test_validate_manifest_dangerous_command_or_or() {
        let mut manifest = valid_manifest();
        manifest.tools[0].command = "echo hello || echo fallback".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_dangerous_command_semicolon() {
        let mut manifest = valid_manifest();
        manifest.tools[0].command = "echo hello; rm -rf /".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_dangerous_command_pipe() {
        let mut manifest = valid_manifest();
        manifest.tools[0].command = "cat file | grep secret".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_dangerous_command_backtick() {
        let mut manifest = valid_manifest();
        manifest.tools[0].command = "echo `whoami`".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_manifest_safe_command_with_template() {
        let mut manifest = valid_manifest();
        manifest.tools[0].command = "git -C {{path}} status --porcelain".to_string();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_manifest_multiple_tools() {
        let mut manifest = valid_manifest();
        manifest.tools.push(PluginToolDef {
            name: "second_tool".to_string(),
            description: "Second tool".to_string(),
            parameters: json!({}),
            command: "date".to_string(),
            working_dir: None,
            timeout_secs: None,
            env: None,
        });
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_manifest_second_tool_invalid() {
        let mut manifest = valid_manifest();
        manifest.tools.push(PluginToolDef {
            name: "valid_tool".to_string(),
            description: "Valid".to_string(),
            parameters: json!({}),
            command: "echo ok && echo bad".to_string(),
            working_dir: None,
            timeout_secs: None,
            env: None,
        });
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
    }

    // ---- binary plugin validation tests ----

    fn binary_manifest() -> PluginManifest {
        PluginManifest {
            name: "bin-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "Binary plugin".to_string(),
            author: None,
            tools: vec![PluginToolDef {
                name: "bin_tool".to_string(),
                description: "A binary tool".to_string(),
                parameters: json!({"type": "object", "properties": {}}),
                command: String::new(),
                working_dir: None,
                timeout_secs: None,
                env: None,
            }],
            execution: "binary".to_string(),
            binary: Some(BinaryPluginConfig {
                path: "bin/plugin".to_string(),
                protocol: "jsonrpc".to_string(),
                timeout_secs: None,
                sha256: None,
            }),
        }
    }

    #[test]
    fn test_validate_binary_mode_valid() {
        let manifest = binary_manifest();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_binary_missing_config() {
        let mut manifest = binary_manifest();
        manifest.binary = None;
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no binary config"));
    }

    #[test]
    fn test_validate_binary_unsupported_protocol() {
        let mut manifest = binary_manifest();
        manifest.binary.as_mut().unwrap().protocol = "grpc".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unsupported binary protocol"));
    }

    #[test]
    fn test_validate_binary_empty_path() {
        let mut manifest = binary_manifest();
        manifest.binary.as_mut().unwrap().path = "  ".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("empty binary path"));
    }

    #[test]
    fn test_validate_binary_path_traversal_dotdot() {
        let mut manifest = binary_manifest();
        manifest.binary.as_mut().unwrap().path = "../escape/bin".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn test_validate_binary_absolute_path_rejected() {
        let mut manifest = binary_manifest();
        manifest.binary.as_mut().unwrap().path = "/usr/bin/evil".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be relative"));
    }

    #[test]
    fn test_validate_invalid_execution_mode() {
        let mut manifest = valid_manifest();
        manifest.execution = "unknown".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown execution mode"));
    }

    #[test]
    fn test_validate_command_mode_still_checks_commands() {
        let mut manifest = valid_manifest();
        manifest.execution = "command".to_string();
        manifest.tools[0].command = "echo hello && rm -rf /".to_string();
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("dangerous pattern"));
    }

    #[test]
    fn test_validate_binary_path_real_file() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir(&bin_dir).unwrap();
        let bin_path = bin_dir.join("plugin");
        fs::write(&bin_path, "#!/bin/sh\necho ok").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = BinaryPluginConfig {
            path: "bin/plugin".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: None,
        };

        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_ok());
        let canonical = result.unwrap();
        assert!(canonical.ends_with("bin/plugin"));
    }

    #[test]
    fn test_validate_binary_path_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let config = BinaryPluginConfig {
            path: "bin/missing".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: None,
        };
        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Binary not found"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_binary_path_not_executable() {
        let tmp = TempDir::new().unwrap();
        let bin_path = tmp.path().join("plugin");
        fs::write(&bin_path, "#!/bin/sh\necho ok").unwrap();
        // Set to read-only (no execute)
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let config = BinaryPluginConfig {
            path: "plugin".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: None,
        };
        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not executable"));
    }

    // -----------------------------------------------------------------------
    // SHA-256 verification tests
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_sha256_match_succeeds() {
        let tmp = TempDir::new().unwrap();
        let bin_path = tmp.path().join("plugin");
        let content = b"#!/bin/sh\necho ok";
        fs::write(&bin_path, content).unwrap();

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let expected = hex::encode(Sha256::digest(content));
        let config = BinaryPluginConfig {
            path: "plugin".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: Some(expected),
        };
        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_sha256_mismatch_rejected() {
        let tmp = TempDir::new().unwrap();
        let bin_path = tmp.path().join("plugin");
        fs::write(&bin_path, b"#!/bin/sh\necho ok").unwrap();

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let config = BinaryPluginConfig {
            path: "plugin".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: Some(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ),
        };
        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("SHA-256 mismatch"));
    }

    #[cfg(unix)]
    #[test]
    fn test_sha256_none_skips_check() {
        let tmp = TempDir::new().unwrap();
        let bin_path = tmp.path().join("plugin");
        fs::write(&bin_path, b"#!/bin/sh\necho ok").unwrap();

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let config = BinaryPluginConfig {
            path: "plugin".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: None,
        };
        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_sha256_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        let bin_path = tmp.path().join("plugin");
        let content = b"#!/bin/sh\necho ok";
        fs::write(&bin_path, content).unwrap();

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let expected = hex::encode(Sha256::digest(content)).to_uppercase();
        let config = BinaryPluginConfig {
            path: "plugin".to_string(),
            protocol: "jsonrpc".to_string(),
            timeout_secs: None,
            sha256: Some(expected),
        };
        let result = validate_binary_path(tmp.path(), &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_binary_plugin_config_sha256_default_none() {
        let json = r#"{"path": "bin/plugin"}"#;
        let config: BinaryPluginConfig = serde_json::from_str(json).expect("should parse");
        assert!(config.sha256.is_none());
    }

    #[test]
    fn test_binary_plugin_config_sha256_deserialize() {
        let json = r#"{"path": "bin/plugin", "sha256": "abc123"}"#;
        let config: BinaryPluginConfig = serde_json::from_str(json).expect("should parse");
        assert_eq!(config.sha256.as_deref(), Some("abc123"));
    }
}
