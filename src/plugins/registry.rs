//! Plugin registry for ZeptoClaw
//!
//! This module provides the `PluginRegistry` struct for managing loaded plugins
//! and mapping tool names back to their originating plugins. It ensures tool
//! name uniqueness across all registered plugins and provides lookup methods
//! for both plugins and individual tool definitions.

use std::collections::HashMap;

use tracing::info;

use crate::error::{Result, ZeptoError};

use super::types::{Plugin, PluginToolDef};

/// A registry that holds loaded plugins and indexes their tools.
///
/// The registry maintains two mappings:
/// - Plugin name to plugin instance
/// - Tool name to plugin name (for reverse lookup)
///
/// This ensures that tool names are globally unique across all registered
/// plugins and provides efficient lookup in both directions.
///
/// # Example
///
/// ```rust
/// use zeptoclaw::plugins::{PluginRegistry, Plugin, PluginManifest, PluginToolDef};
/// use std::path::PathBuf;
/// use serde_json::json;
///
/// let mut registry = PluginRegistry::new();
///
/// let manifest = PluginManifest {
///     name: "example".to_string(),
///     version: "1.0.0".to_string(),
///     description: "Example plugin".to_string(),
///     author: None,
///     tools: vec![PluginToolDef {
///         name: "example_tool".to_string(),
///         description: "An example tool".to_string(),
///         parameters: json!({}),
///         command: "echo hello".to_string(),
///         working_dir: None,
///         timeout_secs: None,
///         env: None,
///         category: None,
///     }],
///     execution: "command".to_string(),
///     binary: None,
/// };
///
/// let plugin = Plugin::new(manifest, PathBuf::from("/tmp/example"));
/// registry.register(plugin).unwrap();
///
/// assert_eq!(registry.plugin_count(), 1);
/// assert_eq!(registry.tool_count(), 1);
/// assert!(registry.is_tool_from_plugin("example_tool"));
/// ```
pub struct PluginRegistry {
    /// Map from plugin name to plugin instance.
    plugins: HashMap<String, Plugin>,

    /// Map from tool name to the name of the plugin that provides it.
    tool_to_plugin: HashMap<String, String>,
}

impl PluginRegistry {
    /// Create a new empty plugin registry.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            tool_to_plugin: HashMap::new(),
        }
    }

    /// Register a plugin in the registry.
    ///
    /// Validates that none of the plugin's tool names conflict with tools
    /// already registered by other plugins. If a conflict is detected,
    /// the registration fails and the registry is unchanged.
    ///
    /// Registering a plugin with the same name as an existing plugin
    /// will replace the old plugin and update tool mappings accordingly.
    ///
    /// # Arguments
    /// * `plugin` - The plugin to register
    ///
    /// # Returns
    /// `Ok(())` on success, or `ZeptoError::Config` if a tool name conflict
    /// is detected with a different plugin.
    pub fn register(&mut self, plugin: Plugin) -> Result<()> {
        let plugin_name = plugin.name().to_string();

        // Check for tool name conflicts with OTHER plugins
        for tool in &plugin.manifest.tools {
            if let Some(existing_plugin) = self.tool_to_plugin.get(&tool.name) {
                if existing_plugin != &plugin_name {
                    return Err(ZeptoError::Config(format!(
                        "Tool name '{}' from plugin '{}' conflicts with existing tool from plugin '{}'",
                        tool.name, plugin_name, existing_plugin
                    )));
                }
            }
        }

        // If re-registering, remove old tool mappings first
        if self.plugins.contains_key(&plugin_name) {
            self.tool_to_plugin.retain(|_, pname| pname != &plugin_name);
        }

        // Register all tool mappings
        for tool in &plugin.manifest.tools {
            self.tool_to_plugin
                .insert(tool.name.clone(), plugin_name.clone());
        }

        info!(
            plugin = %plugin_name,
            tools = plugin.tool_count(),
            "Registered plugin"
        );

        self.plugins.insert(plugin_name, plugin);

        Ok(())
    }

    /// Get a plugin by name.
    ///
    /// # Arguments
    /// * `name` - The plugin name to look up
    ///
    /// # Returns
    /// A reference to the plugin if found, or `None`.
    pub fn get_plugin(&self, name: &str) -> Option<&Plugin> {
        self.plugins.get(name)
    }

    /// Look up which plugin provides a given tool, and return both the
    /// plugin and the tool definition.
    ///
    /// # Arguments
    /// * `tool_name` - The tool name to look up
    ///
    /// # Returns
    /// A tuple of `(Plugin, PluginToolDef)` if the tool is from a plugin,
    /// or `None` if the tool is not registered in any plugin.
    pub fn get_tool_plugin(&self, tool_name: &str) -> Option<(&Plugin, &PluginToolDef)> {
        let plugin_name = self.tool_to_plugin.get(tool_name)?;
        let plugin = self.plugins.get(plugin_name)?;
        let tool_def = plugin.manifest.tools.iter().find(|t| t.name == tool_name)?;
        Some((plugin, tool_def))
    }

    /// Get the number of registered plugins.
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Get the number of registered tools across all plugins.
    pub fn tool_count(&self) -> usize {
        self.tool_to_plugin.len()
    }

    /// Get a list of all registered plugins.
    pub fn list_plugins(&self) -> Vec<&Plugin> {
        self.plugins.values().collect()
    }

    /// Get a list of all registered tool names and their plugin names.
    ///
    /// # Returns
    /// A vector of `(tool_name, plugin_name)` pairs.
    pub fn list_tools(&self) -> Vec<(&str, &str)> {
        self.tool_to_plugin
            .iter()
            .map(|(tool, plugin)| (tool.as_str(), plugin.as_str()))
            .collect()
    }

    /// Check whether a tool name belongs to a plugin-provided tool.
    ///
    /// # Arguments
    /// * `tool_name` - The tool name to check
    ///
    /// # Returns
    /// `true` if the tool is registered in the plugin registry.
    pub fn is_tool_from_plugin(&self, tool_name: &str) -> bool {
        self.tool_to_plugin.contains_key(tool_name)
    }

    /// Get all tool definitions across all registered plugins.
    pub fn all_tool_defs(&self) -> Vec<&PluginToolDef> {
        self.plugins
            .values()
            .flat_map(|p| p.manifest.tools.iter())
            .collect()
    }

    /// Get the plugin name that provides a given tool.
    pub fn plugin_for_tool(&self, tool_name: &str) -> Option<&str> {
        self.tool_to_plugin.get(tool_name).map(|s| s.as_str())
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::{PluginManifest, PluginToolDef};
    use serde_json::json;
    use std::path::PathBuf;

    /// Helper to create a plugin with the given name and tool names.
    fn make_plugin(name: &str, tool_names: &[&str]) -> Plugin {
        let tools: Vec<PluginToolDef> = tool_names
            .iter()
            .map(|tn| PluginToolDef {
                name: tn.to_string(),
                description: format!("Tool {}", tn),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                command: format!("echo {}", tn),
                working_dir: None,
                timeout_secs: None,
                env: None,
                category: None,
            })
            .collect();

        let manifest = PluginManifest {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("Plugin {}", name),
            author: None,
            tools,
            execution: "command".to_string(),
            binary: None,
        };

        Plugin::new(manifest, PathBuf::from(format!("/tmp/{}", name)))
    }

    #[test]
    fn test_registry_new_is_empty() {
        let registry = PluginRegistry::new();
        assert_eq!(registry.plugin_count(), 0);
        assert_eq!(registry.tool_count(), 0);
        assert!(registry.list_plugins().is_empty());
        assert!(registry.list_tools().is_empty());
    }

    #[test]
    fn test_registry_default_is_empty() {
        let registry = PluginRegistry::default();
        assert_eq!(registry.plugin_count(), 0);
        assert_eq!(registry.tool_count(), 0);
    }

    #[test]
    fn test_register_and_lookup_plugin() {
        let mut registry = PluginRegistry::new();
        let plugin = make_plugin("git-tools", &["git_status", "git_log"]);

        registry.register(plugin).unwrap();

        let found = registry.get_plugin("git-tools");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "git-tools");
        assert_eq!(found.unwrap().tool_count(), 2);
    }

    #[test]
    fn test_register_plugin_not_found() {
        let registry = PluginRegistry::new();
        assert!(registry.get_plugin("nonexistent").is_none());
    }

    #[test]
    fn test_tool_name_conflict_detection() {
        let mut registry = PluginRegistry::new();

        let plugin1 = make_plugin("plugin-a", &["shared_tool"]);
        let plugin2 = make_plugin("plugin-b", &["shared_tool"]);

        registry.register(plugin1).unwrap();
        let result = registry.register(plugin2);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("conflicts"));
        assert!(err_msg.contains("shared_tool"));
        assert!(err_msg.contains("plugin-a"));
        assert!(err_msg.contains("plugin-b"));
    }

    #[test]
    fn test_get_tool_by_name_returns_correct_plugin() {
        let mut registry = PluginRegistry::new();

        let plugin1 = make_plugin("plugin-a", &["tool_alpha"]);
        let plugin2 = make_plugin("plugin-b", &["tool_beta"]);

        registry.register(plugin1).unwrap();
        registry.register(plugin2).unwrap();

        let (plugin, tool_def) = registry.get_tool_plugin("tool_alpha").unwrap();
        assert_eq!(plugin.name(), "plugin-a");
        assert_eq!(tool_def.name, "tool_alpha");

        let (plugin, tool_def) = registry.get_tool_plugin("tool_beta").unwrap();
        assert_eq!(plugin.name(), "plugin-b");
        assert_eq!(tool_def.name, "tool_beta");
    }

    #[test]
    fn test_get_tool_plugin_not_found() {
        let registry = PluginRegistry::new();
        assert!(registry.get_tool_plugin("nonexistent_tool").is_none());
    }

    #[test]
    fn test_list_plugins() {
        let mut registry = PluginRegistry::new();

        registry
            .register(make_plugin("alpha", &["tool_a"]))
            .unwrap();
        registry.register(make_plugin("beta", &["tool_b"])).unwrap();
        registry
            .register(make_plugin("gamma", &["tool_c"]))
            .unwrap();

        let plugins = registry.list_plugins();
        assert_eq!(plugins.len(), 3);

        let names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }

    #[test]
    fn test_list_tools() {
        let mut registry = PluginRegistry::new();

        registry
            .register(make_plugin("my-plugin", &["tool_x", "tool_y"]))
            .unwrap();

        let tools = registry.list_tools();
        assert_eq!(tools.len(), 2);

        let tool_names: Vec<&str> = tools.iter().map(|(name, _)| *name).collect();
        assert!(tool_names.contains(&"tool_x"));
        assert!(tool_names.contains(&"tool_y"));

        // All tools should map to the same plugin
        for (_, plugin_name) in &tools {
            assert_eq!(*plugin_name, "my-plugin");
        }
    }

    #[test]
    fn test_plugin_count_and_tool_count() {
        let mut registry = PluginRegistry::new();

        registry.register(make_plugin("p1", &["t1", "t2"])).unwrap();
        registry.register(make_plugin("p2", &["t3"])).unwrap();

        assert_eq!(registry.plugin_count(), 2);
        assert_eq!(registry.tool_count(), 3);
    }

    #[test]
    fn test_is_tool_from_plugin() {
        let mut registry = PluginRegistry::new();

        registry
            .register(make_plugin("my-plugin", &["plugin_tool"]))
            .unwrap();

        assert!(registry.is_tool_from_plugin("plugin_tool"));
        assert!(!registry.is_tool_from_plugin("builtin_tool"));
        assert!(!registry.is_tool_from_plugin("nonexistent"));
    }

    #[test]
    fn test_re_register_same_plugin_replaces() {
        let mut registry = PluginRegistry::new();

        let plugin_v1 = make_plugin("evolving", &["old_tool"]);
        registry.register(plugin_v1).unwrap();
        assert!(registry.is_tool_from_plugin("old_tool"));
        assert_eq!(registry.tool_count(), 1);

        let plugin_v2 = make_plugin("evolving", &["new_tool"]);
        registry.register(plugin_v2).unwrap();

        // Old tool should be gone, new tool should be present
        assert!(!registry.is_tool_from_plugin("old_tool"));
        assert!(registry.is_tool_from_plugin("new_tool"));
        assert_eq!(registry.plugin_count(), 1);
        assert_eq!(registry.tool_count(), 1);
    }

    #[test]
    fn test_multiple_plugins_no_conflict() {
        let mut registry = PluginRegistry::new();

        registry
            .register(make_plugin("git-tools", &["git_status", "git_log"]))
            .unwrap();
        registry
            .register(make_plugin("docker-tools", &["docker_ps", "docker_build"]))
            .unwrap();
        registry
            .register(make_plugin("k8s-tools", &["kubectl_get"]))
            .unwrap();

        assert_eq!(registry.plugin_count(), 3);
        assert_eq!(registry.tool_count(), 5);

        // Verify each tool maps to the correct plugin
        assert_eq!(
            registry.get_tool_plugin("git_status").unwrap().0.name(),
            "git-tools"
        );
        assert_eq!(
            registry.get_tool_plugin("docker_ps").unwrap().0.name(),
            "docker-tools"
        );
        assert_eq!(
            registry.get_tool_plugin("kubectl_get").unwrap().0.name(),
            "k8s-tools"
        );
    }

    #[test]
    fn test_get_tool_plugin_returns_correct_tool_def() {
        let mut registry = PluginRegistry::new();

        let mut plugin = make_plugin("multi-tool", &["tool_a", "tool_b"]);
        plugin.manifest.tools[0].command = "command_a".to_string();
        plugin.manifest.tools[1].command = "command_b".to_string();

        registry.register(plugin).unwrap();

        let (_, tool_a) = registry.get_tool_plugin("tool_a").unwrap();
        assert_eq!(tool_a.command, "command_a");

        let (_, tool_b) = registry.get_tool_plugin("tool_b").unwrap();
        assert_eq!(tool_b.command, "command_b");
    }
}
