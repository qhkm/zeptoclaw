---
title: Environment Variables
description: All environment variable overrides for ZeptoClaw
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 3
---

Every config field can be overridden with an environment variable. The naming convention is `ZEPTOCLAW_` followed by the JSON path with underscores.

## Provider keys

| Variable | Description |
|----------|-------------|
| `ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY` | Anthropic Claude API key |
| `ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY` | OpenAI API key |

## Channel tokens

| Variable | Description |
|----------|-------------|
| `ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN` | Telegram bot token |
| `ZEPTOCLAW_CHANNELS_SLACK_BOT_TOKEN` | Slack bot token |
| `ZEPTOCLAW_CHANNELS_DISCORD_BOT_TOKEN` | Discord bot token |

## Agent settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS` | `300` | Wall-clock timeout for agent runs |
| `ZEPTOCLAW_AGENTS_DEFAULTS_MESSAGE_QUEUE_MODE` | `"collect"` | Queue mode: collect or followup |
| `ZEPTOCLAW_AGENTS_DEFAULTS_TOKEN_BUDGET` | `0` | Per-session token budget (0 = unlimited) |

## Retry settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_PROVIDERS_RETRY_ENABLED` | `false` | Enable retry wrapper |
| `ZEPTOCLAW_PROVIDERS_RETRY_MAX_RETRIES` | `3` | Max retry attempts |
| `ZEPTOCLAW_PROVIDERS_RETRY_BASE_DELAY_MS` | `1000` | Initial retry delay (ms) |
| `ZEPTOCLAW_PROVIDERS_RETRY_MAX_DELAY_MS` | `30000` | Max retry delay (ms) |

## Fallback settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_PROVIDERS_FALLBACK_ENABLED` | `false` | Enable fallback provider |
| `ZEPTOCLAW_PROVIDERS_FALLBACK_PROVIDER` | — | Fallback provider name |

## Safety settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_SAFETY_ENABLED` | `true` | Enable safety layer |
| `ZEPTOCLAW_SAFETY_LEAK_DETECTION_ENABLED` | `true` | Enable secret leak detection |

## Compaction settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_COMPACTION_ENABLED` | `false` | Enable context compaction |
| `ZEPTOCLAW_COMPACTION_CONTEXT_LIMIT` | `100000` | Max tokens before compaction |
| `ZEPTOCLAW_COMPACTION_THRESHOLD` | `0.80` | Compaction trigger threshold |

## Routines settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_ROUTINES_ENABLED` | `false` | Enable routines engine |
| `ZEPTOCLAW_ROUTINES_CRON_INTERVAL_SECS` | `60` | Cron tick interval |
| `ZEPTOCLAW_ROUTINES_MAX_CONCURRENT` | `3` | Max concurrent routine executions |

## Memory settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_MEMORY_BACKEND` | `"builtin"` | Search backend: builtin, bm25, embedding, hnsw |

## Tool settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_TOOLS_CODING_TOOLS` | `false` | Enable coding tools (grep, find). Auto-enabled by the `coder` template. |
| `ZEPTOCLAW_TOOLS_WEB_SEARCH_PROVIDER` | auto | Search provider: `brave`, `searxng`, `ddg` |
| `ZEPTOCLAW_TOOLS_WEB_SEARCH_API_URL` | — | SearXNG instance URL (required when provider is `searxng`) |

## Tunnel settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_TUNNEL_PROVIDER` | — | Tunnel provider (cloudflare, ngrok, tailscale, auto) |

## Encryption settings

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_MASTER_KEY` | — | Hex-encoded 32-byte master key for secret encryption |

## Compile-time defaults

These are set at build time, not runtime:

| Variable | Default | Description |
|----------|---------|-------------|
| `ZEPTOCLAW_DEFAULT_MODEL` | `claude-sonnet-4-5-20250929` | Default model for agent |
| `ZEPTOCLAW_CLAUDE_DEFAULT_MODEL` | `claude-sonnet-4-5-20250929` | Default Claude model |
| `ZEPTOCLAW_OPENAI_DEFAULT_MODEL` | `gpt-5.1` | Default OpenAI model |

```bash
# Example: build with OpenAI as default
export ZEPTOCLAW_DEFAULT_MODEL=gpt-5.1
cargo build --release
```

## Priority order

1. Environment variables (highest priority)
2. Config file (`~/.zeptoclaw/config.json`)
3. Built-in defaults (lowest priority)
