---
title: Providers
description: LLM providers and the resilient provider stack
---

Providers are the LLM backends that power your agent. ZeptoClaw supports 9 providers out of the box, all configurable via `~/.zeptoclaw/config.json` or environment variables.

## Supported providers

| Provider | Backend | Default model | Notes |
|----------|---------|---------------|-------|
| **Anthropic** | Native | claude-sonnet-4-5 | Claude API |
| **OpenAI** | Native | gpt-5.1 | OpenAI API |
| **OpenRouter** | OpenAI-compatible | — | 400+ models via single key |
| **Groq** | OpenAI-compatible | — | Fast inference |
| **Ollama** | OpenAI-compatible | — | Local models |
| **VLLM** | OpenAI-compatible | — | Local model serving |
| **Google Gemini** | OpenAI-compatible | — | Gemini models |
| **NVIDIA NIM** | OpenAI-compatible | — | NVIDIA inference |
| **Zhipu (GLM)** | OpenAI-compatible | — | Chinese LLM |

All providers except Anthropic use the OpenAI-compatible chat completions API, so any endpoint that speaks that protocol works.

## Quick setup

The fastest way to configure a provider:

```bash
zeptoclaw onboard
```

The onboard wizard supports Anthropic, OpenAI, and OpenRouter. For other providers, edit `~/.zeptoclaw/config.json` directly or use environment variables.

## Configuration

### Anthropic

```json
{
  "providers": {
    "anthropic": {
      "api_key": "sk-ant-..."
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...
```

### OpenAI

```json
{
  "providers": {
    "openai": {
      "api_key": "sk-..."
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY=sk-...
```

### OpenRouter

```json
{
  "providers": {
    "openrouter": {
      "api_key": "sk-or-..."
    }
  },
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4"
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_OPENROUTER_API_KEY=sk-or-...
```

The default base URL is `https://openrouter.ai/api/v1`. Set `model` to any model available on OpenRouter.

### Groq

```json
{
  "providers": {
    "groq": {
      "api_key": "gsk_..."
    }
  },
  "agents": {
    "defaults": {
      "model": "mixtral-8x7b-32768"
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_GROQ_API_KEY=gsk_...
```

### Ollama

```json
{
  "providers": {
    "ollama": {
      "api_key": "ollama"
    }
  },
  "agents": {
    "defaults": {
      "model": "mistral"
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_OLLAMA_API_KEY=ollama
```

The default base URL is `http://localhost:11434/v1`. The API key value doesn't matter (Ollama doesn't require one) but the field must be set.

To connect to a remote Ollama instance:

```json
{
  "providers": {
    "ollama": {
      "api_key": "ollama",
      "api_base": "http://my-server:11434/v1"
    }
  }
}
```

### VLLM

```json
{
  "providers": {
    "vllm": {
      "api_key": "vllm"
    }
  },
  "agents": {
    "defaults": {
      "model": "meta-llama/Llama-2-7b-hf"
    }
  }
}
```

The default base URL is `http://localhost:8000/v1`.

### Google Gemini

```json
{
  "providers": {
    "gemini": {
      "api_key": "AIza..."
    }
  },
  "agents": {
    "defaults": {
      "model": "gemini-2.5-pro"
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_GEMINI_API_KEY=AIza...
```

### NVIDIA NIM

```json
{
  "providers": {
    "nvidia": {
      "api_key": "nvapi-..."
    }
  },
  "agents": {
    "defaults": {
      "model": "meta/llama-3.1-8b-instruct"
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_NVIDIA_API_KEY=nvapi-...
```

### Zhipu (GLM)

```json
{
  "providers": {
    "zhipu": {
      "api_key": "..."
    }
  },
  "agents": {
    "defaults": {
      "model": "glm-4"
    }
  }
}
```

## Custom API base URL

Any provider's base URL can be overridden. This is useful for proxies, self-hosted endpoints, or alternative API gateways:

```json
{
  "providers": {
    "openai": {
      "api_key": "sk-...",
      "api_base": "https://my-proxy.example.com/v1"
    }
  }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_OPENAI_API_BASE=https://my-proxy.example.com/v1
```

## Provider stack

ZeptoClaw wraps providers in a composable stack:

```
Base Provider
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

### Retry

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

Delay doubles after each retry, capped at `max_delay_ms`.

### Fallback

Automatically switches to a backup provider when the primary fails:

```json
{
  "providers": {
    "anthropic": { "api_key": "sk-ant-..." },
    "openai": { "api_key": "sk-..." },
    "fallback": {
      "enabled": true,
      "provider": "openai"
    }
  }
}
```

The fallback provider uses a circuit breaker: after 3 consecutive failures the primary is bypassed, and after a 30-second cooldown it's probed again.

## Streaming

All providers support SSE streaming:

```bash
zeptoclaw agent --stream -m "Tell me a story"
```

## Environment variables

Every provider field can be set via environment variables:

```bash
# API keys
ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=...
ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY=...
ZEPTOCLAW_PROVIDERS_OPENROUTER_API_KEY=...
ZEPTOCLAW_PROVIDERS_GROQ_API_KEY=...
ZEPTOCLAW_PROVIDERS_OLLAMA_API_KEY=...
ZEPTOCLAW_PROVIDERS_VLLM_API_KEY=...
ZEPTOCLAW_PROVIDERS_GEMINI_API_KEY=...
ZEPTOCLAW_PROVIDERS_NVIDIA_API_KEY=...
ZEPTOCLAW_PROVIDERS_ZHIPU_API_KEY=...

# Custom base URLs
ZEPTOCLAW_PROVIDERS_OPENAI_API_BASE=...
ZEPTOCLAW_PROVIDERS_OLLAMA_API_BASE=...

# Retry
ZEPTOCLAW_PROVIDERS_RETRY_ENABLED=true
ZEPTOCLAW_PROVIDERS_RETRY_MAX_RETRIES=3
ZEPTOCLAW_PROVIDERS_RETRY_BASE_DELAY_MS=1000
ZEPTOCLAW_PROVIDERS_RETRY_MAX_DELAY_MS=30000

# Fallback
ZEPTOCLAW_PROVIDERS_FALLBACK_ENABLED=true
ZEPTOCLAW_PROVIDERS_FALLBACK_PROVIDER=openai
```

Environment variables take precedence over config.json values.
