//! Plugin types for ZeptoClaw
//!
//! This module defines all types used by the plugin system, including
//! manifest structures for parsing `plugin.json` files, plugin configuration,
//! and the runtime plugin representation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// Default execution mode for plugins.
fn default_execution() -> String {
    "command".to_string()
}

/// Default protocol for binary plugins.
fn default_protocol() -> String {
    "jsonrpc".to_string()
}

/// The manifest loaded from a plugin's `plugin.json` file.
///
/// Each plugin directory must contain a `plugin.json` file that conforms
/// to this structure. The manifest declares the plugin's identity and the
/// tools it provides.
///
/// # Example
///
/// ```json
/// {
///   "name": "git-tools",
///   "version": "1.0.0",
///   "description": "Git integration tools",
///   "author": "ZeptoClaw",
///   "tools": [
///     {
///       "name": "git_status",
///       "description": "Get the git status of the workspace",
///       "parameters": {
///         "type": "object",
///         "properties": {
///           "path": { "type": "string", "description": "Repository path" }
///         },
///         "required": ["path"]
///       },
///       "command": "git -C {{path}} status --porcelain",
///       "timeout_secs": 10
///     }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name. Must be unique, alphanumeric characters and hyphens only,
    /// between 1 and 64 characters.
    pub name: String,

    /// Semantic version string (e.g., "1.0.0").
    pub version: String,

    /// Human-readable description of what the plugin provides.
    pub description: String,

    /// Optional author name or identifier.
    #[serde(default)]
    pub author: Option<String>,

    /// List of tool definitions provided by this plugin.
    pub tools: Vec<PluginToolDef>,

    /// Execution mode: "command" (default) or "binary" (JSON-RPC stdin/stdout).
    #[serde(default = "default_execution")]
    pub execution: String,

    /// Binary plugin configuration. Required when execution is "binary".
    #[serde(default)]
    pub binary: Option<BinaryPluginConfig>,
}

/// Configuration for binary plugin execution.
///
/// Binary plugins are standalone executables that communicate via JSON-RPC 2.0
/// over stdin/stdout. They are spawned on-demand per tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryPluginConfig {
    /// Relative path to binary within plugin directory.
    pub path: String,

    /// Protocol: only "jsonrpc" supported.
    #[serde(default = "default_protocol")]
    pub protocol: String,

    /// Optional timeout override in seconds (default: 30).
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional SHA-256 hex digest for binary integrity verification.
    /// When set, the binary's hash is checked before execution.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A tool definition within a plugin manifest.
///
/// Each tool wraps a shell command template that is executed when the LLM
/// invokes the tool. Parameter interpolation uses `{{param_name}}` syntax
/// within the command string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    /// Tool name as registered with the agent. Must be alphanumeric
    /// characters and underscores only.
    pub name: String,

    /// Tool description sent to the LLM to help it understand
    /// when and how to use the tool.
    pub description: String,

    /// JSON Schema describing the tool's parameters.
    pub parameters: Value,

    /// Shell command template. Uses `{{param_name}}` for parameter interpolation.
    /// Must not contain dangerous shell operators (&&, ||, ;, |, backticks).
    /// Empty for binary plugins.
    #[serde(default)]
    pub command: String,

    /// Optional working directory for command execution.
    #[serde(default)]
    pub working_dir: Option<String>,

    /// Optional timeout in seconds. Defaults to 30 if not specified.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional environment variables to set during command execution.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

impl PluginManifest {
    /// Returns true if this plugin uses binary execution mode.
    pub fn is_binary(&self) -> bool {
        self.execution == "binary"
    }
}

impl PluginToolDef {
    /// Returns the effective timeout in seconds, defaulting to 30.
    pub fn effective_timeout(&self) -> u64 {
        self.timeout_secs.unwrap_or(30)
    }
}

/// A loaded plugin with its manifest, filesystem path, and enabled state.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// The parsed plugin manifest.
    pub manifest: PluginManifest,

    /// The directory path where the plugin was loaded from.
    pub path: PathBuf,

    /// Whether this plugin is currently enabled.
    pub enabled: bool,
}

impl Plugin {
    /// Create a new plugin from a manifest and path, enabled by default.
    pub fn new(manifest: PluginManifest, path: PathBuf) -> Self {
        Self {
            manifest,
            path,
            enabled: true,
        }
    }

    /// Get the plugin name from its manifest.
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// Get the number of tools defined by this plugin.
    pub fn tool_count(&self) -> usize {
        self.manifest.tools.len()
    }
}

/// Plugin system configuration, typically stored within the main config.json.
///
/// Controls whether the plugin system is active, which directories are
/// scanned for plugins, and which plugins are allowed or blocked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Whether the plugin system is enabled. Defaults to false.
    #[serde(default)]
    pub enabled: bool,

    /// Directories to scan for plugin subdirectories.
    /// Defaults to `["~/.zeptoclaw/plugins"]`.
    #[serde(default = "default_plugin_dirs")]
    pub plugin_dirs: Vec<String>,

    /// Allowlist of plugin names. If empty, all discovered plugins are allowed.
    #[serde(default)]
    pub allowed_plugins: Vec<String>,

    /// Blocklist of plugin names. If empty, no plugins are blocked.
    /// Blocklist takes precedence over allowlist.
    #[serde(default)]
    pub blocked_plugins: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            plugin_dirs: default_plugin_dirs(),
            allowed_plugins: Vec::new(),
            blocked_plugins: Vec::new(),
        }
    }
}

impl PluginConfig {
    /// Check whether a plugin name is permitted by the allow/block lists.
    ///
    /// A plugin is permitted if:
    /// - It is not in the blocked list, AND
    /// - The allowed list is empty (all plugins allowed) OR the plugin is in the allowed list.
    pub fn is_plugin_permitted(&self, name: &str) -> bool {
        if self.blocked_plugins.contains(&name.to_string()) {
            return false;
        }
        if self.allowed_plugins.is_empty() {
            return true;
        }
        self.allowed_plugins.contains(&name.to_string())
    }
}

/// Returns the default plugin directories.
fn default_plugin_dirs() -> Vec<String> {
    vec!["~/.zeptoclaw/plugins".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_plugin_manifest_serialization_roundtrip() {
        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            author: Some("Tester".to_string()),
            tools: vec![PluginToolDef {
                name: "test_tool".to_string(),
                description: "A test tool".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    },
                    "required": ["input"]
                }),
                command: "echo {{input}}".to_string(),
                working_dir: None,
                timeout_secs: Some(15),
                env: None,
            }],
            execution: "command".to_string(),
            binary: None,
        };

        let json_str = serde_json::to_string(&manifest).unwrap();
        let deserialized: PluginManifest = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.name, "test-plugin");
        assert_eq!(deserialized.version, "1.0.0");
        assert_eq!(deserialized.description, "A test plugin");
        assert_eq!(deserialized.author, Some("Tester".to_string()));
        assert_eq!(deserialized.tools.len(), 1);
        assert_eq!(deserialized.tools[0].name, "test_tool");
        assert_eq!(deserialized.tools[0].timeout_secs, Some(15));
    }

    #[test]
    fn test_plugin_manifest_deserialization_from_json() {
        let json_str = r#"{
            "name": "git-tools",
            "version": "1.0.0",
            "description": "Git integration tools",
            "author": "ZeptoClaw",
            "tools": [
                {
                    "name": "git_status",
                    "description": "Get git status",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" }
                        },
                        "required": ["path"]
                    },
                    "command": "git -C {{path}} status --porcelain",
                    "timeout_secs": 10
                }
            ]
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json_str).unwrap();
        assert_eq!(manifest.name, "git-tools");
        assert_eq!(
            manifest.tools[0].command,
            "git -C {{path}} status --porcelain"
        );
    }

    #[test]
    fn test_plugin_tool_def_defaults() {
        let json_str = r#"{
            "name": "simple_tool",
            "description": "A simple tool",
            "parameters": { "type": "object", "properties": {} },
            "command": "echo hello"
        }"#;

        let tool_def: PluginToolDef = serde_json::from_str(json_str).unwrap();
        assert_eq!(tool_def.name, "simple_tool");
        assert!(tool_def.working_dir.is_none());
        assert!(tool_def.timeout_secs.is_none());
        assert!(tool_def.env.is_none());
        assert_eq!(tool_def.effective_timeout(), 30);
    }

    #[test]
    fn test_plugin_tool_def_effective_timeout() {
        let tool = PluginToolDef {
            name: "t".to_string(),
            description: "d".to_string(),
            parameters: json!({}),
            command: "echo".to_string(),
            working_dir: None,
            timeout_secs: Some(60),
            env: None,
        };
        assert_eq!(tool.effective_timeout(), 60);

        let tool_default = PluginToolDef {
            name: "t".to_string(),
            description: "d".to_string(),
            parameters: json!({}),
            command: "echo".to_string(),
            working_dir: None,
            timeout_secs: None,
            env: None,
        };
        assert_eq!(tool_default.effective_timeout(), 30);
    }

    #[test]
    fn test_plugin_tool_def_with_env() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("BAZ".to_string(), "qux".to_string());

        let tool = PluginToolDef {
            name: "env_tool".to_string(),
            description: "Tool with env".to_string(),
            parameters: json!({}),
            command: "printenv FOO".to_string(),
            working_dir: Some("/tmp".to_string()),
            timeout_secs: Some(5),
            env: Some(env),
        };

        assert_eq!(tool.env.as_ref().unwrap().get("FOO").unwrap(), "bar");
        assert_eq!(tool.working_dir.as_deref(), Some("/tmp"));
    }

    #[test]
    fn test_plugin_config_defaults() {
        let config = PluginConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.plugin_dirs, vec!["~/.zeptoclaw/plugins"]);
        assert!(config.allowed_plugins.is_empty());
        assert!(config.blocked_plugins.is_empty());
    }

    #[test]
    fn test_plugin_config_deserialization_defaults() {
        let json_str = r#"{}"#;
        let config: PluginConfig = serde_json::from_str(json_str).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.plugin_dirs, vec!["~/.zeptoclaw/plugins"]);
        assert!(config.allowed_plugins.is_empty());
        assert!(config.blocked_plugins.is_empty());
    }

    #[test]
    fn test_plugin_config_is_plugin_permitted_all_allowed() {
        let config = PluginConfig::default();
        assert!(config.is_plugin_permitted("any-plugin"));
        assert!(config.is_plugin_permitted("another-plugin"));
    }

    #[test]
    fn test_plugin_config_is_plugin_permitted_allowlist() {
        let config = PluginConfig {
            enabled: true,
            plugin_dirs: vec![],
            allowed_plugins: vec!["good-plugin".to_string()],
            blocked_plugins: vec![],
        };
        assert!(config.is_plugin_permitted("good-plugin"));
        assert!(!config.is_plugin_permitted("other-plugin"));
    }

    #[test]
    fn test_plugin_config_is_plugin_permitted_blocklist() {
        let config = PluginConfig {
            enabled: true,
            plugin_dirs: vec![],
            allowed_plugins: vec![],
            blocked_plugins: vec!["bad-plugin".to_string()],
        };
        assert!(!config.is_plugin_permitted("bad-plugin"));
        assert!(config.is_plugin_permitted("good-plugin"));
    }

    #[test]
    fn test_plugin_config_blocklist_overrides_allowlist() {
        let config = PluginConfig {
            enabled: true,
            plugin_dirs: vec![],
            allowed_plugins: vec!["my-plugin".to_string()],
            blocked_plugins: vec!["my-plugin".to_string()],
        };
        // Blocklist takes precedence
        assert!(!config.is_plugin_permitted("my-plugin"));
    }

    #[test]
    fn test_plugin_struct_construction() {
        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "Test".to_string(),
            author: None,
            tools: vec![PluginToolDef {
                name: "t".to_string(),
                description: "d".to_string(),
                parameters: json!({}),
                command: "echo".to_string(),
                working_dir: None,
                timeout_secs: None,
                env: None,
            }],
            execution: "command".to_string(),
            binary: None,
        };

        let plugin = Plugin::new(manifest, PathBuf::from("/tmp/test-plugin"));
        assert_eq!(plugin.name(), "test-plugin");
        assert!(plugin.enabled);
        assert_eq!(plugin.path, PathBuf::from("/tmp/test-plugin"));
        assert_eq!(plugin.tool_count(), 1);
    }

    #[test]
    fn test_plugin_manifest_without_author() {
        let json_str = r#"{
            "name": "minimal",
            "version": "1.0.0",
            "description": "Minimal plugin",
            "tools": [
                {
                    "name": "noop",
                    "description": "Does nothing",
                    "parameters": { "type": "object", "properties": {} },
                    "command": "true"
                }
            ]
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json_str).unwrap();
        assert!(manifest.author.is_none());
        assert_eq!(manifest.tools.len(), 1);
    }

    #[test]
    fn test_plugin_tool_parameter_schema() {
        let tool = PluginToolDef {
            name: "search".to_string(),
            description: "Search files".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum results",
                        "default": 10
                    }
                },
                "required": ["query"]
            }),
            command: "grep -r {{query}}".to_string(),
            working_dir: None,
            timeout_secs: None,
            env: None,
        };

        let params = &tool.parameters;
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["query"].is_object());
        assert!(params["properties"]["max_results"].is_object());
        assert_eq!(params["required"][0], "query");
    }

    // ---- Binary plugin types tests ----

    #[test]
    fn test_manifest_default_execution_is_command() {
        let json_str = r#"{
            "name": "basic-plugin",
            "version": "1.0.0",
            "description": "Basic",
            "tools": [{
                "name": "t",
                "description": "d",
                "parameters": {},
                "command": "echo"
            }]
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json_str).unwrap();
        assert_eq!(manifest.execution, "command");
        assert!(manifest.binary.is_none());
        assert!(!manifest.is_binary());
    }

    #[test]
    fn test_manifest_binary_deserialization() {
        let json_str = r#"{
            "name": "bin-plugin",
            "version": "1.0.0",
            "description": "Binary plugin",
            "execution": "binary",
            "binary": {
                "path": "bin/my-plugin",
                "protocol": "jsonrpc",
                "timeout_secs": 60
            },
            "tools": [{
                "name": "my_tool",
                "description": "Does stuff",
                "parameters": {"type": "object", "properties": {}}
            }]
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json_str).unwrap();
        assert_eq!(manifest.execution, "binary");
        assert!(manifest.is_binary());
        let bin = manifest.binary.unwrap();
        assert_eq!(bin.path, "bin/my-plugin");
        assert_eq!(bin.protocol, "jsonrpc");
        assert_eq!(bin.timeout_secs, Some(60));
        // command defaults to empty for binary plugins
        assert_eq!(manifest.tools[0].command, "");
    }

    #[test]
    fn test_manifest_binary_config_defaults() {
        let json_str = r#"{
            "name": "bin-plugin",
            "version": "1.0.0",
            "description": "Minimal binary",
            "execution": "binary",
            "binary": { "path": "plugin" },
            "tools": [{
                "name": "t",
                "description": "d",
                "parameters": {}
            }]
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json_str).unwrap();
        let bin = manifest.binary.unwrap();
        assert_eq!(bin.protocol, "jsonrpc");
        assert!(bin.timeout_secs.is_none());
    }

    #[test]
    fn test_manifest_is_binary() {
        let command_manifest = PluginManifest {
            name: "cmd".to_string(),
            version: "1.0.0".to_string(),
            description: "Command".to_string(),
            author: None,
            tools: vec![],
            execution: "command".to_string(),
            binary: None,
        };
        assert!(!command_manifest.is_binary());

        let binary_manifest = PluginManifest {
            name: "bin".to_string(),
            version: "1.0.0".to_string(),
            description: "Binary".to_string(),
            author: None,
            tools: vec![],
            execution: "binary".to_string(),
            binary: Some(BinaryPluginConfig {
                path: "plugin".to_string(),
                protocol: "jsonrpc".to_string(),
                timeout_secs: None,
                sha256: None,
            }),
        };
        assert!(binary_manifest.is_binary());
    }

    #[test]
    fn test_manifest_backward_compat_no_execution_field() {
        // Old-style manifests without execution/binary fields should still work
        let json_str = r#"{
            "name": "old-plugin",
            "version": "1.0.0",
            "description": "Old style",
            "tools": [{
                "name": "old_tool",
                "description": "Old tool",
                "parameters": {},
                "command": "echo hello"
            }]
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json_str).unwrap();
        assert_eq!(manifest.execution, "command");
        assert!(manifest.binary.is_none());
        assert!(!manifest.is_binary());
        assert_eq!(manifest.tools[0].command, "echo hello");
    }
}
