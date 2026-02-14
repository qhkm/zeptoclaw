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
      "bot_token": "123456:ABC..."
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

## Config validation

Run `zeptoclaw config check` to validate your configuration. It reports:

- Unknown field names
- Type mismatches
- Missing required values
- Invalid enum values
