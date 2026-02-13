---
title: Environment Variables
description: Environment variable reference
---

Complete reference for all environment variables supported by r8r.

## Server configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `R8R_SERVER_HOST` | Bind address | `0.0.0.0` |
| `R8R_SERVER_PORT` | HTTP port | `3000` |
| `R8R_SERVER_REQUEST_TIMEOUT` | Request timeout | `30s` |
| `R8R_SERVER_MAX_BODY_SIZE` | Max body size | `10MB` |
| `R8R_SERVER_WORKERS` | Worker threads | CPU count |

## Workflow configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `R8R_WORKFLOWS_DIRECTORY` | Workflows directory | `./workflows` |
| `R8R_WORKFLOWS_HOT_RELOAD` | Auto-reload on change | `true` |

## Logging

| Variable | Description | Default |
|----------|-------------|---------|
| `R8R_LOGGING_LEVEL` | Log level | `info` |
| `R8R_LOGGING_FORMAT` | Output format | `pretty` |
| `R8R_LOGGING_OUTPUT` | Log destination | `stdout` |

Log levels: `trace`, `debug`, `info`, `warn`, `error`

Formats: `pretty` (colored), `json` (structured), `compact`

## Integration variables

### PostgreSQL

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |

### Redis

| Variable | Description |
|----------|-------------|
| `REDIS_URL` | Redis connection string |

### Slack

| Variable | Description |
|----------|-------------|
| `SLACK_TOKEN` | Bot OAuth token |
| `SLACK_SIGNING_SECRET` | Webhook signing secret |

### Kafka

| Variable | Description |
|----------|-------------|
| `KAFKA_BROKERS` | Comma-separated broker list |

## Security

| Variable | Description |
|----------|-------------|
| `R8R_SECRET_KEY` | Encryption key for secrets |
| `R8R_ALLOWED_HOSTS` | Comma-separated allowed hosts |

## Profile

| Variable | Description |
|----------|-------------|
| `R8R_PROFILE` | Active profile (dev/staging/production) |
| `R8R_ENV` | Environment name |

## Example .env file

```bash
# Server
R8R_SERVER_PORT=8080
R8R_SERVER_WORKERS=4

# Logging
R8R_LOGGING_LEVEL=debug
R8R_LOGGING_FORMAT=json

# Integrations
DATABASE_URL=postgres://user:pass@localhost/db
REDIS_URL=redis://localhost:6379
SLACK_TOKEN=xoxb-your-token

# Profile
R8R_PROFILE=production
```

## Loading .env files

r8r automatically loads `.env` files:

1. `.env` — Base environment
2. `.env.local` — Local overrides (gitignored)
3. `.env.{PROFILE}` — Profile-specific

Example:

```bash
# .env
R8R_SERVER_PORT=3000

# .env.local
R8R_SERVER_PORT=3001  # Local developer override

# .env.production
R8R_LOGGING_LEVEL=warn
```
