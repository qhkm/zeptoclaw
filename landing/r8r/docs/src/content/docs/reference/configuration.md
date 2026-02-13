---
title: Configuration
description: r8r configuration options
---

r8r can be configured via `r8r.toml` or environment variables.

## r8r.toml

```toml
[server]
host = "0.0.0.0"
port = 3000
request_timeout = "30s"

[workflows]
directory = "./workflows"
hot_reload = true

[logging]
level = "info"
format = "json"  # json | pretty

[nodes]
# Custom node directories
custom = ["./nodes"]

[integrations]
[integrations.slack]
token = "${SLACK_TOKEN}"

[integrations.postgres]
url = "${DATABASE_URL}"

[integrations.redis]
url = "redis://localhost:6379"
```

## Server options

| Option | Default | Description |
|--------|---------|-------------|
| `host` | `"0.0.0.0"` | Bind address |
| `port` | `3000` | HTTP port |
| `request_timeout` | `"30s"` | Request timeout |
| `max_body_size` | `"10MB"` | Max request body size |
| `workers` | `num_cpus` | Worker threads |

## Workflow options

| Option | Default | Description |
|--------|---------|-------------|
| `directory` | `"./workflows"` | Workflows directory |
| `hot_reload` | `true` | Auto-reload on change |
| `extensions` | `["yaml", "yml"]` | Valid file extensions |

## Logging options

| Option | Default | Description |
|--------|---------|-------------|
| `level` | `"info"` | Log level (trace/debug/info/warn/error) |
| `format` | `"pretty"` | Output format |
| `output` | `"stdout"` | Log destination |

## Environment variables

All config options can be set via environment:

```bash
R8R_SERVER_PORT=8080
R8R_WORKFLOWS_HOT_RELOAD=false
R8R_LOGGING_LEVEL=debug
```

Use `${VAR}` syntax in config:

```toml
[integrations.slack]
token = "${SLACK_TOKEN}"
```

## Profile-specific config

Override settings per environment:

```toml
# Default settings
[server]
port = 3000

[profile.production]
[profile.production.server]
port = 8080
host = "127.0.0.1"

[profile.production.logging]
level = "warn"
format = "json"
```

Activate profile:

```bash
R8R_PROFILE=production r8r run
```
