---
title: Providers
description: LLM providers and the resilient provider stack
---

Providers are the LLM backends that power your agent. ZeptoClaw supports multiple providers with automatic retry and failover.

## Supported providers

| Provider | Models | Streaming |
|----------|--------|-----------|
| **Claude** (Anthropic) | Claude Sonnet 4.5, Opus 4, etc. | SSE |
| **OpenAI** | GPT-5.1, GPT-4o, etc. | SSE |

## Provider stack

ZeptoClaw wraps providers in a composable stack:

```
Base Provider (Claude or OpenAI)
    │
    ▼
┌───────────────────┐
│ FallbackProvider   │  Primary → Secondary auto-failover
└────────┬──────────┘
         │
         ▼
┌───────────────────┐
│ RetryProvider      │  Exponential backoff on 429/5xx
└────────┬──────────┘
         │
         ▼
      Agent Loop
```

## Configuration

Set your provider in `~/.zeptoclaw/config.json`:

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
    }
  }
}
```

## Retry provider

Automatically retries on transient failures (HTTP 429 rate limits and 5xx server errors):

```json
{
  "providers": {
    "retry": {
      "enabled": true,
      "max_retries": 3,
      "base_delay_ms": 1000,
      "max_delay_ms": 30000
    }
  }
}
```

Uses exponential backoff: delay doubles after each retry, capped at `max_delay_ms`.

## Fallback provider

Automatically switches to a backup provider when the primary fails:

```json
{
  "providers": {
    "default": "anthropic",
    "fallback": {
      "enabled": true,
      "provider": "openai"
    }
  }
}
```

If Claude returns an error, ZeptoClaw automatically retries with OpenAI.

## Streaming

Both providers support SSE streaming for real-time token delivery:

```bash
zeptoclaw agent --stream -m "Tell me a story"
```

The `StreamEvent` enum carries individual tokens, tool calls, and completion signals. Streaming works in CLI mode, gateway mode, and batch mode.

## Structured output

Control the response format with the `output_format` option:

- **Text** — Default free-form text
- **Json** — Requests JSON output from the model
- **JsonSchema** — Enforces a specific JSON schema (OpenAI `response_format`)

## Cost tracking

ZeptoClaw tracks token usage and estimates costs per model. Pricing tables cover 8 models across both providers. View costs in metrics output or Prometheus export.

## Compile-time defaults

Override default models at build time:

```bash
export ZEPTOCLAW_DEFAULT_MODEL=gpt-5.1
cargo build --release
```
