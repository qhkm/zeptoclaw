---
title: Deployment
description: Deploy ZeptoClaw to production
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 3
---

ZeptoClaw can be deployed anywhere a Linux binary can run. Choose the method that fits your infrastructure.

## Docker (single container)

The simplest production deployment:

```dockerfile
FROM ghcr.io/qhkm/zeptoclaw:latest

COPY config.json /root/.zeptoclaw/config.json

EXPOSE 8080
CMD ["zeptoclaw", "gateway"]
```

```bash
docker build -t my-agent .
docker run -d --name zeptoclaw \
  -e ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-... \
  -e ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN=123456:ABC... \
  my-agent
```

## Docker Compose

For multi-service setups:

```yaml
version: '3.8'
services:
  zeptoclaw:
    image: ghcr.io/qhkm/zeptoclaw:latest
    restart: unless-stopped
    environment:
      - ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=${ANTHROPIC_KEY}
      - ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN=${TELEGRAM_TOKEN}
    volumes:
      - zeptoclaw-data:/root/.zeptoclaw
    healthcheck:
      test: ["CMD", "zeptoclaw", "config", "check"]
      interval: 30s
      retries: 3

volumes:
  zeptoclaw-data:
```

## Fly.io

Deploy to Fly.io with a single command:

```toml
# fly.toml
app = "my-zeptoclaw"
primary_region = "sin"

[build]
  image = "ghcr.io/qhkm/zeptoclaw:latest"

[env]
  RUST_LOG = "info"

[[services]]
  internal_port = 8080
  protocol = "tcp"

  [[services.ports]]
    port = 443
    handlers = ["tls", "http"]
```

```bash
fly launch
fly secrets set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...
fly secrets set ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN=123456:ABC...
fly deploy
```

## Railway

Deploy via Railway CLI:

```bash
railway init
railway up

# Set secrets
railway variables set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...
railway variables set ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN=123456:ABC...
```

## Render

Use the Render dashboard or `render.yaml`:

```yaml
services:
  - type: worker
    name: zeptoclaw
    runtime: docker
    dockerfilePath: ./Dockerfile
    envVars:
      - key: ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY
        sync: false
      - key: ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN
        sync: false
```

## Systemd (bare metal)

For direct deployment on a Linux server:

```ini
# /etc/systemd/system/zeptoclaw.service
[Unit]
Description=ZeptoClaw AI Agent
After=network-online.target

[Service]
Type=simple
User=zeptoclaw
ExecStart=/usr/local/bin/zeptoclaw gateway
Restart=always
RestartSec=5
Environment=RUST_LOG=info
EnvironmentFile=/etc/zeptoclaw/env

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable zeptoclaw
sudo systemctl start zeptoclaw
```

## Health checks

ZeptoClaw's `config check` command can be used as a health check:

```bash
zeptoclaw config check
```

Returns exit code 0 if configuration is valid.

## Persistent data

Important directories to persist across restarts:

| Path | Contents |
|------|----------|
| `~/.zeptoclaw/config.json` | Configuration |
| `~/.zeptoclaw/memory/` | Long-term memory |
| `~/.zeptoclaw/sessions/` | Conversation history |
| `~/.zeptoclaw/skills/` | Custom skills |
| `~/.zeptoclaw/plugins/` | Custom plugins |

Mount `~/.zeptoclaw` as a volume in Docker deployments.

## Resource requirements

ZeptoClaw is very lightweight:

| Resource | Requirement |
|----------|-------------|
| CPU | Any modern CPU (ARM64 or x86_64) |
| RAM | ~6MB RSS at idle |
| Disk | ~5MB binary + data |
| Network | Outbound HTTPS to LLM provider APIs |
