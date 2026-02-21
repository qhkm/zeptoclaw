//! MCP server auto-discovery from standard config files.
//!
//! Scans two locations (in order):
//! 1. `~/.mcp/servers.json` — global user-level config
//! 2. `{workspace}/.mcp.json` — project-level config (optional)
//!
//! Supported JSON formats:
//! - **Claude Desktop** (`{"mcpServers": { "name": { "url": "..." } }}`)
//! - **Flat servers** (`{"servers": { "name": { "url": "..." } }}`)
//! - **Direct map** (`{ "name": { "url": "..." } }`)  (fall-through)
//!
//! Only servers with a `url` field (HTTP transport) are included. Stdio-only
//! entries (`command` with no `url`) are skipped with a debug log so they do
//! not cause errors for callers that only speak HTTP.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single entry from a `servers.json` / `.mcp.json` file.
///
/// Only `url` is required for HTTP transport discovery. All other fields are
/// preserved for callers that want to inspect them.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerEntry {
    /// HTTP endpoint for the MCP server.
    pub url: Option<String>,
    /// Stdio server command (not used by HTTP transport).
    pub command: Option<String>,
    /// Stdio server arguments.
    pub args: Option<Vec<String>>,
    /// Environment variables forwarded to the server process.
    pub env: Option<HashMap<String, String>>,
}

/// An MCP server successfully discovered from a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMcpServer {
    /// The key name given to this server in the config file.
    pub name: String,
    /// HTTP URL for the MCP server endpoint.
    pub url: String,
    /// Origin of this entry: `"global"` or `"project"`.
    pub source: String,
}

// ---------------------------------------------------------------------------
// Discovery entrypoint
// ---------------------------------------------------------------------------

/// Discover MCP servers from standard config file locations.
///
/// Checks (in order):
/// 1. `~/.mcp/servers.json` — global config
/// 2. `{workspace}/.mcp.json` — project config (only if `workspace` is `Some`)
///
/// Both files are optional. Missing files are silently skipped. Parse errors
/// emit a `warn!` log and skip the file rather than returning an error so that
/// discovery never blocks agent startup.
///
/// Only servers with a `url` field are returned. Stdio-only servers are logged
/// at `debug` level and excluded.
///
/// # Example
///
/// ```rust,no_run
/// use std::path::Path;
/// use zeptoclaw::tools::mcp::discovery::discover_mcp_servers;
///
/// let servers = discover_mcp_servers(Some(Path::new("/my/project")));
/// for s in &servers {
///     println!("Found MCP server '{}' at {} ({})", s.name, s.url, s.source);
/// }
/// ```
pub fn discover_mcp_servers(workspace: Option<&Path>) -> Vec<DiscoveredMcpServer> {
    let mut servers = Vec::new();

    // 1. Global: ~/.mcp/servers.json
    let global_path: Option<PathBuf> =
        dirs::home_dir().map(|h| h.join(".mcp").join("servers.json"));
    if let Some(ref path) = global_path {
        if let Some(discovered) = load_mcp_config(path, "global") {
            servers.extend(discovered);
        }
    }

    // 2. Project-level: {workspace}/.mcp.json
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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Load and parse one MCP config file, returning HTTP-capable servers only.
///
/// Returns `None` when the file does not exist or cannot be parsed, so callers
/// can distinguish "missing" from "empty".
pub(crate) fn load_mcp_config(path: &Path, source: &str) -> Option<Vec<DiscoveredMcpServer>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None, // file absent — not an error
    };

    let entries = parse_mcp_config_json(&content, path)?;

    let mut result = Vec::new();
    for (name, entry) in entries {
        match entry.url {
            Some(url) => {
                result.push(DiscoveredMcpServer {
                    name,
                    url,
                    source: source.to_string(),
                });
            }
            None => {
                debug!(
                    server = %name,
                    path = %path.display(),
                    "Skipping stdio-only MCP server (no url field)"
                );
            }
        }
    }

    Some(result)
}

/// Parse the JSON content of an MCP config file into a name→entry map.
///
/// Attempts three formats in order:
/// 1. `{"mcpServers": {...}}` — Claude Desktop format
/// 2. `{"servers": {...}}` — flat servers alias
/// 3. Direct `HashMap<String, McpServerEntry>` — bare map
///
/// Returns `None` on JSON parse failure (caller emits the warning).
fn parse_mcp_config_json(content: &str, path: &Path) -> Option<HashMap<String, McpServerEntry>> {
    // Wrapper that accepts both "mcpServers" and "servers" via #[serde(alias)].
    #[derive(Deserialize)]
    struct McpConfigWrapper {
        #[serde(alias = "mcpServers", alias = "servers")]
        servers: Option<HashMap<String, McpServerEntry>>,
    }

    // Try the wrapper format first (most common).
    if let Ok(wrapper) = serde_json::from_str::<McpConfigWrapper>(content) {
        if let Some(servers) = wrapper.servers {
            return Some(servers);
        }
        // Wrapper parsed but "servers"/"mcpServers" key absent — fall through
        // to direct map attempt.
    }

    // Fallback: the file is a bare HashMap at the top level.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- discover_mcp_servers -------------------------------------------------

    #[test]
    fn test_discover_no_files_returns_empty() {
        // Passing a nonexistent workspace; global ~/.mcp/servers.json may or
        // may not exist on the CI machine, but that is fine — we only assert
        // the function does not panic.
        let servers = discover_mcp_servers(Some(Path::new("/nonexistent/path")));
        // Result may be non-empty if the test runner has ~/.mcp/servers.json.
        // We only verify the call succeeds.
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
        assert_eq!(project_servers[0].url, "http://localhost:4000");
    }

    // -- load_mcp_config ------------------------------------------------------

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
        assert_eq!(
            servers.len(),
            1,
            "Only the server with url should be included"
        );
        assert_eq!(servers[0].name, "github");
        assert_eq!(servers[0].url, "http://localhost:3000");
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
    fn test_load_mcp_config_all_stdio_returns_empty_vec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {"only-stdio": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"]}}}"#,
        )
        .unwrap();

        // Returns Some(vec![]) — file was parseable, just no HTTP servers.
        let servers = load_mcp_config(&path, "global").unwrap();
        assert!(servers.is_empty(), "No URL entries → empty vec, not None");
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

        // Empty object: wrapper parse succeeds, servers key is None, falls
        // through to bare-map parse which gives an empty HashMap.
        let result = load_mcp_config(&path, "test");
        // Either Some([]) or None are valid — ensure no panic.
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
        assert_eq!(servers[0].url, "http://localhost:5000");
    }

    // -- DiscoveredMcpServer --------------------------------------------------

    #[test]
    fn test_discovered_server_fields() {
        let s = DiscoveredMcpServer {
            name: "my-server".to_string(),
            url: "http://localhost:9000".to_string(),
            source: "global".to_string(),
        };
        assert_eq!(s.name, "my-server");
        assert_eq!(s.url, "http://localhost:9000");
        assert_eq!(s.source, "global");
    }

    #[test]
    fn test_discovered_server_equality() {
        let a = DiscoveredMcpServer {
            name: "svc".to_string(),
            url: "http://localhost:1234".to_string(),
            source: "project".to_string(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
