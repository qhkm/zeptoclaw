//! Plugin system for ZeptoClaw
//!
//! This module provides a plugin system that allows loading tools from external
//! JSON-defined command plugins. Each plugin is a directory containing a
//! `plugin.json` manifest and optional scripts. Plugins wrap shell commands
//! with parameter interpolation, making it easy to extend ZeptoClaw's tool
//! set without writing Rust code.
//!
//! # Architecture
//!
//! - **types**: Core data structures (`PluginManifest`, `PluginToolDef`, `Plugin`, `PluginConfig`)
//! - **loader**: Plugin discovery, loading, and manifest validation
//! - **registry**: Plugin and tool registration with conflict detection
//!
//! # Plugin Directory Structure
//!
//! ```text
//! ~/.zeptoclaw/plugins/
//! ├── git-tools/
//! │   └── plugin.json
//! ├── docker-tools/
//! │   ├── plugin.json
//! │   └── scripts/
//! │       └── helper.sh
//! └── custom-tool/
//!     └── plugin.json
//! ```
//!
//! # Example plugin.json
//!
//! ```json
//! {
//!   "name": "git-tools",
//!   "version": "1.0.0",
//!   "description": "Git integration tools",
//!   "author": "ZeptoClaw",
//!   "tools": [
//!     {
//!       "name": "git_status",
//!       "description": "Get the git status of the workspace",
//!       "parameters": {
//!         "type": "object",
//!         "properties": {
//!           "path": { "type": "string", "description": "Repository path" }
//!         },
//!         "required": ["path"]
//!       },
//!       "command": "git -C {{path}} status --porcelain",
//!       "timeout_secs": 10
//!     }
//!   ]
//! }
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use std::path::PathBuf;
//! use zeptoclaw::plugins::{discover_plugins, PluginRegistry};
//!
//! let dirs = vec![PathBuf::from("/home/user/.zeptoclaw/plugins")];
//! let plugins = discover_plugins(&dirs).unwrap();
//!
//! let mut registry = PluginRegistry::new();
//! for plugin in plugins {
//!     registry.register(plugin).unwrap();
//! }
//!
//! println!("Loaded {} plugins with {} tools", registry.plugin_count(), registry.tool_count());
//! ```

mod loader;
pub mod registry;
pub mod types;
pub mod watcher;

pub use loader::{discover_plugins, load_plugin, validate_binary_path, validate_manifest};
pub use registry::PluginRegistry;
pub use types::{BinaryPluginConfig, Plugin, PluginConfig, PluginManifest, PluginToolDef};
pub use watcher::{check_binary_health, PluginWatcher};
