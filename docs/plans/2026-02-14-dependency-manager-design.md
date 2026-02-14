# Dependency Manager Design

**Date:** 2026-02-14
**Status:** Approved

## Problem

ZeptoClaw channels, tools, and skills increasingly depend on external binaries/services (WhatsApp bridge, MCP servers, etc.). Currently users must manually install and run these. This creates friction and breaks the seamless UX.

## Solution

A dependency manager (`src/deps/`) that handles install + lifecycle of external dependencies. Components declare what they need via a `HasDependencies` trait. A central `DepManager` handles download, install, start, health check, and stop.

## Core Abstractions

### Dependency Declaration

```rust
enum DepKind {
    Binary { repo, asset_pattern, version },
    DockerImage { image, tag, ports },
    NpmPackage { package, version, entry_point },
    PipPackage { package, version, entry_point },
}

struct Dependency {
    name: String,
    kind: DepKind,
    health_check: HealthCheck,
    env: HashMap<String, String>,
    args: Vec<String>,
}

enum HealthCheck {
    WebSocket(String),
    Http(String),
    TcpPort(u16),
    Command(String),
    None,
}

trait HasDependencies {
    fn dependencies(&self) -> Vec<Dependency> { vec![] }
}
```

### DepManager

```rust
impl DepManager {
    async fn ensure_installed(&self, dep: &Dependency) -> Result<()>;
    async fn start(&self, dep: &Dependency) -> Result<ManagedProcess>;
    async fn stop(&self, name: &str) -> Result<()>;
    async fn stop_all(&self) -> Result<()>;
    async fn wait_healthy(&self, dep: &Dependency, timeout: Duration) -> Result<()>;
    fn is_installed(&self, name: &str) -> bool;
    fn is_running(&self, name: &str) -> bool;
}
```

### File Layout

```
~/.zeptoclaw/deps/
├── bin/                      # Downloaded binaries
├── node_modules/             # npm installs
├── venvs/                    # pip virtualenvs
├── registry.json             # Installed state tracking
└── logs/                     # Process stdout/stderr
```

### Registry

```json
{
  "whatsmeow-bridge": {
    "kind": "binary",
    "version": "v0.1.0",
    "installed_at": "2026-02-14T10:00:00Z",
    "path": "~/.zeptoclaw/deps/bin/whatsmeow-bridge"
  }
}
```

## Integration Points

1. **Gateway startup** — collect deps from enabled channels/tools, ensure_installed, start, wait_healthy, then start channels. stop_all on shutdown.
2. **channel setup** — ensure_installed, start temporarily for pairing/config, stop, save config.
3. **channel test** — ensure_installed, start, wait_healthy, report status, stop.
4. **WhatsAppChannel** — implements HasDependencies returning Binary dep for whatsmeow-bridge.

## Config Override

`bridge_managed: false` on WhatsAppConfig (and similar for other components) skips DepManager entirely. Default is true.

## Install Strategies

- **Binary:** GitHub Releases API → download asset matching `{os}-{arch}` → chmod +x → `deps/bin/`
- **DockerImage:** `docker pull` → track in registry
- **NpmPackage:** `npm install --prefix ~/.zeptoclaw/deps` → entry_point via `node`
- **PipPackage:** `python -m venv` → `pip install` → entry_point in venv

## Error Handling

- GitHub rate limit: retry with backoff, suggest GITHUB_TOKEN
- No matching platform asset: clear error with manual override instructions
- Network offline: use cached binary from registry
- npm/pip not found: error with install instructions
- Process crash: detect via try_wait, log to deps/logs/
- Port conflict: report in health check failure
- Unclean shutdown: check registry for stale running entries on startup

## Security

- Downloaded binaries not executed from temp dirs (same blocklist as Docker binary)
- SHA256 checksum verification when available
- npm/pip installs in isolated directories, never global

## Testing

- **Unit (~25):** Dependency construction, HealthCheck variants, HasDependencies default, registry serde, platform detection, bridge_managed config
- **Integration (~10):** Mock GitHub API for binary download, start/stop/health lifecycle, gateway dep collection, bridge_managed=false skip
- **DepFetcher trait** for testability — mock replaces real network calls

## Module Structure

| File | Purpose |
|------|---------|
| `src/deps/types.rs` | Dependency, DepKind, HealthCheck, HasDependencies |
| `src/deps/manager.rs` | DepManager — install, start, stop, health |
| `src/deps/registry.rs` | JSON registry CRUD |
| `src/deps/fetcher.rs` | DepFetcher trait + GitHub/npm/pip/docker impls |
| `src/deps/mod.rs` | Module exports |

No new crate dependencies.
