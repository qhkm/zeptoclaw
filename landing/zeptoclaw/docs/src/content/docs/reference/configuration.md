---
title: Configuration
description: ZeptoClaw configuration file reference
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 3
---

ZeptoClaw is configured via `~/.zeptoclaw/config.json`. All fields have sensible defaults.

## File location

```
~/.zeptoclaw/
├── config.json        # Main configuration
├── memory/
│   └── longterm.json  # Long-term memory store
├── sessions/          # Conversation history
├── skills/            # Custom skills (markdown)
└── plugins/           # Custom plugins (JSON manifests)
```

## Full example

```json
{
  "providers": {
    "default": "anthropic",
    "anthropic": {
      "api_key": "sk-ant-...",
      "model": "claude-sonnet-4-5-20250929"
    },
    "openai": {
      "api_key": "sk-...",
      "model": "gpt-5.1"
    },
    "retry": {
      "enabled": true,
      "max_retries": 3,
      "base_delay_ms": 1000,
      "max_delay_ms": 30000
    },
    "fallback": {
      "enabled": true,
      "provider": "openai"
    }
  },
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "123456:ABC..."
    },
    "slack": {
      "enabled": false,
      "bot_token": "xoxb-..."
    },
    "discord": {
      "enabled": false,
      "bot_token": "MTIz...",
      "guild_id": "123456789"
    },
    "webhook": {
      "enabled": false,
      "bind": "0.0.0.0",
      "port": 8080,
      "auth_token": "my-secret"
    },
    "whatsapp_cloud": {
      "enabled": false,
      "phone_number_id": "...",
      "access_token": "..."
    },
    "lark": {
      "enabled": false,
      "app_id": "...",
      "app_secret": "..."
    }
  },
  "agents": {
    "defaults": {
      "agent_timeout_secs": 300,
      "message_queue_mode": "collect",
      "token_budget": 0,
      "streaming": false
    }
  },
  "approval": {
    "enabled": false,
    "require_approval": ["shell", "write_file"],
    "auto_approve": ["read_file", "memory"]
  },
  "plugins": {
    "enabled": true,
    "directories": ["~/.zeptoclaw/plugins"]
  },
  "telemetry": {
    "enabled": false,
    "format": "prometheus"
  },
  "cost": {
    "enabled": false,
    "warn_threshold_usd": 1.0
  },
  "hooks": {
    "before_tool": [],
    "after_tool": [],
    "on_error": []
  }
}
```

## Providers section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `providers.default` | string | `"anthropic"` | Default provider name |
| `providers.anthropic.api_key` | string | — | Anthropic API key |
| `providers.anthropic.model` | string | `"claude-sonnet-4-5-20250929"` | Claude model |
| `providers.openai.api_key` | string | — | OpenAI API key |
| `providers.openai.model` | string | `"gpt-5.1"` | OpenAI model |
| `providers.retry.enabled` | bool | `false` | Enable retry wrapper |
| `providers.retry.max_retries` | int | `3` | Max retry attempts |
| `providers.fallback.enabled` | bool | `false` | Enable fallback provider |
| `providers.fallback.provider` | string | — | Fallback provider name |

## Agents section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agents.defaults.agent_timeout_secs` | int | `300` | Wall-clock timeout in seconds |
| `agents.defaults.message_queue_mode` | string | `"collect"` | Queue mode: collect or followup |
| `agents.defaults.token_budget` | int | `0` | Per-session token budget (0 = unlimited) |
| `agents.defaults.streaming` | bool | `false` | Enable streaming by default |

## Approval section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `approval.enabled` | bool | `false` | Enable approval gate |
| `approval.require_approval` | array | `[]` | Tools requiring approval |
| `approval.auto_approve` | array | `[]` | Tools auto-approved |

## Safety section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `safety.enabled` | bool | `true` | Enable safety layer |
| `safety.leak_detection_enabled` | bool | `true` | Enable secret leak detection |

## Runtime section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `runtime.runtime_type` | string | `"native"` | Shell runtime: native, Docker, Apple Container, Landlock, Firejail, or Bubblewrap |
| `runtime.allow_fallback_to_native` | bool | `false` | Fall back to native execution when the configured runtime is unavailable |
| `runtime.env_passthrough` | array | `[]` | Parent environment variable names explicitly allowed into runtime commands |

Runtime subprocesses scrub secret-like inherited environment variables by default. Add only variables required by trusted workflows to `env_passthrough`, or set the comma-separated `ZEPTOCLAW_RUNTIME_ENV_PASSTHROUGH` override.

## Compaction section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `compaction.enabled` | bool | `false` | Enable context compaction |
| `compaction.context_limit` | int | `100000` | Max tokens before compaction |
| `compaction.threshold` | float | `0.80` | Compaction trigger threshold |

## Routines section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `routines.enabled` | bool | `false` | Enable routines engine |
| `routines.cron_interval_secs` | int | `60` | Cron tick interval |
| `routines.max_concurrent` | int | `3` | Max concurrent routine executions |

## Memory section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `memory.backend` | string | `"builtin"` | Search backend: builtin, bm25, embedding, hnsw |

## Config validation

Run `zeptoclaw config check` to validate your configuration. It reports:

- Unknown field names
- Type mismatches
- Missing required values
- Invalid enum values
