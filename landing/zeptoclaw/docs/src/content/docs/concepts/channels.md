---
title: Channels
description: How messaging channels work in ZeptoClaw
---

Channels are the input/output interfaces for your agent. They receive messages from users and deliver agent responses back.

## Available channels

| Channel | Protocol | Direction |
|---------|----------|-----------|
| **Telegram** | Bot API (long polling) | Bidirectional |
| **Slack** | Web API | Outbound |
| **Discord** | Gateway WebSocket + REST | Bidirectional |
| **WhatsApp Cloud** | Webhook + REST API | Bidirectional |
| **Lark** | WebSocket (pbbp2 frames) | Bidirectional |
| **Email** | IMAP IDLE + SMTP | Bidirectional |
| **Webhook** | HTTP POST | Inbound |
| **CLI** | stdin/stdout | Bidirectional |

## Gateway mode

Run all configured channels simultaneously with the gateway command:

```bash
zeptoclaw gateway
```

The gateway starts each enabled channel and routes messages through the agent loop via an async MessageBus.

You can expose your gateway to the internet using a tunnel:

```bash
# With tunnel (auto-detect provider)
zeptoclaw gateway --tunnel auto

# With specific tunnel provider
zeptoclaw gateway --tunnel cloudflare
```

## Telegram

The Telegram channel uses the Bot API with long polling. Configure it with:

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "123456:ABC..."
    }
  }
}
```

Or via environment variable:
```bash
export ZEPTOCLAW_CHANNELS_TELEGRAM_TOKEN=123456:ABC...
```

## Slack

Slack integration provides outbound messaging via the Web API:

```json
{
  "channels": {
    "slack": {
      "enabled": true,
      "bot_token": "xoxb-..."
    }
  }
}
```

## Discord

Discord uses the Gateway WebSocket for real-time events and REST API for sending messages:

```json
{
  "channels": {
    "discord": {
      "enabled": true,
      "bot_token": "MTIz...",
      "guild_id": "123456789"
    }
  }
}
```

## Webhook

The webhook channel accepts HTTP POST requests with optional Bearer token authentication:

```json
{
  "channels": {
    "webhook": {
      "enabled": true,
      "bind": "0.0.0.0",
      "port": 8080,
      "auth_token": "my-secret-token"
    }
  }
}
```

Send messages to your agent:
```bash
curl -X POST http://localhost:8080/webhook \
  -H "Authorization: Bearer my-secret-token" \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello agent", "chat_id": "user-123"}'
```

## WhatsApp Cloud

Official WhatsApp Cloud API integration (no bridge dependency):

```json
{
  "channels": {
    "whatsapp_cloud": {
      "enabled": true,
      "phone_number_id": "123456789",
      "access_token": "EAAx...",
      "verify_token": "my-verify-token"
    }
  }
}
```

## Lark

Lark/Feishu messaging integration via WebSocket:

```json
{
  "channels": {
    "lark": {
      "enabled": true,
      "app_id": "cli_...",
      "app_secret": "..."
    }
  }
}
```

## Email

Email channel using IMAP IDLE for inbound and SMTP for outbound (feature-gated: `--features channel-email`):

```json
{
  "channels": {
    "email": {
      "enabled": true,
      "imap_host": "imap.gmail.com",
      "imap_port": 993,
      "smtp_host": "smtp.gmail.com",
      "smtp_port": 587,
      "username": "agent@example.com",
      "password": "app-password"
    }
  }
}
```

## Sender allowlists

All channels support the `deny_by_default` config option for sender allowlists. When enabled, only explicitly allowed sender IDs can interact with the agent. This works on all channels including Telegram, Discord, WhatsApp Cloud, Lark, Email, and Webhook.

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "123456:ABC...",
      "deny_by_default": true,
      "allow_from": ["123456789", "987654321"]
    }
  }
}
```

## Container isolation

When running in gateway mode with `--containerized`, each agent interaction runs inside an isolated container:

```bash
# Auto-detect container runtime
zeptoclaw gateway --containerized

# Force Docker
zeptoclaw gateway --containerized docker

# Force Apple Container (macOS 15+)
zeptoclaw gateway --containerized apple
```

## Message bus

All channels communicate through an async MessageBus. Inbound messages are published to the bus, processed by the agent loop, and outbound responses are delivered back through the originating channel.

The bus also supports proactive messaging — the agent can send messages to any channel using the `message` tool.
