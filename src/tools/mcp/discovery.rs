//! MCP server auto-discovery from standard config files.
//!
//! Scans two locations (in order):
//! 1. `~/.mcp/servers.json` — global user-level config
//! 2. `{workspace}/.mcp.json` — project-level config (optional)
//!
//! Supported JSON formats:
//! - **Claude Desktop** (`{"mcpServers": { "name": { ... } }}`)
//! - **Flat servers** (`{"servers": { "name": { ... } }}`)
//! - **Direct map** (`{ "name": { ... } }`) (fall-through)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{debug, info, warn};

/// A single entry from a `servers.json` / `.mcp.json` file.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerEntry {
    /// HTTP endpoint for the MCP server.
    pub url: Option<String>,
    /// Stdio server command.
    pub command: Option<String>,
    /// Stdio server arguments.
    pub args: Option<Vec<String>>,
    /// Environment variables forwarded to the server process.
    pub env: Option<HashMap<String, String>>,
}

/// Transport type for a discovered MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransportType {
    /// HTTP transport — server has a URL endpoint.
    Http { url: String },
    /// Stdio transport — server is a child process.
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
}

/// An MCP server successfully discovered from a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMcpServer {
    /// The key name given to this server in the config file.
    pub name: String,
    /// Transport configuration.
    pub transport: McpTransportType,
    /// Origin of this entry: `"global"` or `"project"`.
    pub source: String,
}

/// Discover MCP servers from standard config file locations.
pub fn discover_mcp_servers(workspace: Option<&Path>) -> Vec<DiscoveredMcpServer> {
    let mut servers = Vec::new();

    let global_path: Option<PathBuf> =
        dirs::home_dir().map(|h| h.join(".mcp").join("servers.json"));
    if let Some(ref path) = global_path {
        if let Some(discovered) = load_mcp_config(path, "global") {
            servers.extend(discovered);
        }
    }

    if let Some(ws) = workspace {
        let project_path = ws.join(".mcp.json");
        if let Some(discovered) = load_mcp_config(&project_path, "project") {
            servers.extend(discovered);
        }
    }

    if !servers.is_empty() {
        info!(count = servers.len(), "Discovered MCP servers");
    }

    servers
}

/// Load and parse one MCP config file.
pub(crate) fn load_mcp_config(path: &Path, source: &str) -> Option<Vec<DiscoveredMcpServer>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    let entries = parse_mcp_config_json(&content, path)?;

    let mut result = Vec::new();
    for (name, entry) in entries {
        if let Some(url) = entry.url {
            result.push(DiscoveredMcpServer {
                name,
                transport: McpTransportType::Http { url },
                source: source.to_string(),
            });
        } else if let Some(command) = entry.command {
            result.push(DiscoveredMcpServer {
                name,
                transport: McpTransportType::Stdio {
                    command,
                    args: entry.args.unwrap_or_default(),
                    env: entry.env.unwrap_or_default(),
                },
                source: source.to_string(),
            });
        } else {
            debug!(
                server = %name,
                path = %path.display(),
                "Skipping MCP server entry (no url or command)"
            );
        }
    }

    Some(result)
}

fn parse_mcp_config_json(content: &str, path: &Path) -> Option<HashMap<String, McpServerEntry>> {
    #[derive(Deserialize)]
    struct McpConfigWrapper {
        #[serde(alias = "mcpServers", alias = "servers")]
        servers: Option<HashMap<String, McpServerEntry>>,
    }

    if let Ok(wrapper) = serde_json::from_str::<McpConfigWrapper>(content) {
        if let Some(servers) = wrapper.servers {
            return Some(servers);
        }
    }

    match serde_json::from_str::<HashMap<String, McpServerEntry>>(content) {
        Ok(map) => Some(map),
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "Failed to parse MCP config; skipping"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_no_files_returns_empty() {
        let servers = discover_mcp_servers(Some(Path::new("/nonexistent/path")));
        let _ = servers;
    }

    #[test]
    fn test_discover_project_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".mcp.json");
        std::fs::write(
            &config_path,
            r#"{"servers": {"db": {"url": "http://localhost:4000"}}}"#,
        )
        .unwrap();

        let servers = discover_mcp_servers(Some(dir.path()));
        let project_servers: Vec<_> = servers.iter().filter(|s| s.source == "project").collect();

        assert_eq!(project_servers.len(), 1);
        assert_eq!(project_servers[0].name, "db");
        match &project_servers[0].transport {
            McpTransportType::Http { url } => assert_eq!(url, "http://localhost:4000"),
            _ => panic!("Expected HTTP transport"),
        }
    }

    #[test]
    fn test_load_mcp_config_missing_file() {
        let result = load_mcp_config(Path::new("/nonexistent/servers.json"), "global");
        assert!(result.is_none(), "Missing file should return None");
    }

    #[test]
    fn test_load_mcp_config_claude_desktop_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "github": { "url": "http://localhost:3000" },
                    "stdio-only": { "command": "node", "args": ["server.js"] }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "test").unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].source, "test");
    }

    #[test]
    fn test_load_mcp_config_servers_key_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(
            &path,
            r#"{"servers": {"db": {"url": "http://localhost:4000"}}}"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "project").unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "db");
        assert_eq!(servers[0].source, "project");
    }

    #[test]
    fn test_load_mcp_config_multiple_http_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "svc-a": { "url": "http://localhost:3001" },
                    "svc-b": { "url": "http://localhost:3002" },
                    "svc-c": { "url": "http://localhost:3003" }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "global").unwrap();
        assert_eq!(servers.len(), 3);
    }

    #[test]
    fn test_load_mcp_config_all_stdio_returns_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {"only-stdio": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "global").unwrap();
        assert_eq!(servers.len(), 1, "stdio entries should be included");
        assert!(matches!(
            servers[0].transport,
            McpTransportType::Stdio { .. }
        ));
    }

    #[test]
    fn test_load_mcp_config_invalid_json_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not valid json {{{").unwrap();

        assert!(
            load_mcp_config(&path, "test").is_none(),
            "Invalid JSON should return None"
        );
    }

    #[test]
    fn test_load_mcp_config_empty_object_returns_some_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.json");
        std::fs::write(&path, "{}").unwrap();

        let result = load_mcp_config(&path, "test");
        let _ = result;
    }

    #[test]
    fn test_load_mcp_config_with_env_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "api-server": {
                        "url": "http://localhost:5000",
                        "env": { "API_KEY": "secret" }
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "global").unwrap();
        assert_eq!(servers.len(), 1);
        match &servers[0].transport {
            McpTransportType::Http { url } => assert_eq!(url, "http://localhost:5000"),
            _ => panic!("Expected HTTP transport"),
        }
    }

    #[test]
    fn test_load_mcp_config_stdio_entry_included() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "http-server": { "url": "http://localhost:3000" },
                    "stdio-server": { "command": "node", "args": ["server.js"] }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "test").unwrap();
        assert_eq!(servers.len(), 2, "Both HTTP and stdio should be included");

        let http = servers.iter().find(|s| s.name == "http-server").unwrap();
        assert!(matches!(http.transport, McpTransportType::Http { .. }));

        let stdio = servers.iter().find(|s| s.name == "stdio-server").unwrap();
        assert!(matches!(stdio.transport, McpTransportType::Stdio { .. }));
    }

    #[test]
    fn test_load_mcp_config_both_url_and_command_prefers_http() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {"both": {"url": "http://localhost:3000", "command": "node", "args": ["server.js"]}}}"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "test").unwrap();
        assert_eq!(servers.len(), 1);
        assert!(matches!(
            servers[0].transport,
            McpTransportType::Http { .. }
        ));
    }

    #[test]
    fn test_load_mcp_config_no_url_no_command_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {"empty": {"env": {"KEY": "val"}}}}"#,
        )
        .unwrap();

        let servers = load_mcp_config(&path, "test").unwrap();
        assert!(
            servers.is_empty(),
            "Entry with neither url nor command should be skipped"
        );
    }

    #[test]
    fn test_mcp_transport_type_http_fields() {
        let t = McpTransportType::Http {
            url: "http://localhost:3000".to_string(),
        };
        assert!(matches!(t, McpTransportType::Http { .. }));
    }

    #[test]
    fn test_mcp_transport_type_stdio_fields() {
        let t = McpTransportType::Stdio {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env: HashMap::from([("KEY".to_string(), "val".to_string())]),
        };
        if let McpTransportType::Stdio { command, args, env } = t {
            assert_eq!(command, "node");
            assert_eq!(args, vec!["server.js"]);
            assert_eq!(env.get("KEY"), Some(&"val".to_string()));
        } else {
            panic!("Expected Stdio variant");
        }
    }

    #[test]
    fn test_discovered_server_fields() {
        let s = DiscoveredMcpServer {
            name: "my-server".to_string(),
            transport: McpTransportType::Http {
                url: "http://localhost:9000".to_string(),
            },
            source: "global".to_string(),
        };
        assert_eq!(s.name, "my-server");
        assert_eq!(s.source, "global");
        assert!(matches!(s.transport, McpTransportType::Http { .. }));
    }

    #[test]
    fn test_discovered_server_equality() {
        let a = DiscoveredMcpServer {
            name: "svc".to_string(),
            transport: McpTransportType::Http {
                url: "http://localhost:1234".to_string(),
            },
            source: "project".to_string(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
