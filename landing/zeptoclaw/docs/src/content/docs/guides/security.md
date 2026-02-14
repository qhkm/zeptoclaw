---
title: Security
description: Security features and best practices for ZeptoClaw
---

ZeptoClaw is designed with security as a core concern. This guide covers the built-in security features and best practices for production deployments.

## Container isolation

The strongest security boundary. Shell commands execute inside an isolated container instead of the host system:

```bash
# Auto-detect runtime
zeptoclaw gateway --containerized

# Force Docker
zeptoclaw gateway --containerized docker

# Force Apple Container (macOS 15+)
zeptoclaw gateway --containerized apple
```

When containerized, each agent interaction runs in a fresh container with:
- Isolated filesystem (only mounted workspace visible)
- No network access to the host
- Resource limits via container runtime

## Shell blocklist

A regex-based defense-in-depth layer that blocks dangerous shell patterns:

- Destructive commands (`rm -rf /`, `mkfs`, `dd`)
- Reverse shells (`bash -i >& /dev/tcp`, `nc -e`)
- Privilege escalation (`sudo`, `su -`)
- Data exfiltration patterns (`curl | sh`, `base64 --decode`)
- Script execution (`python -c`, `perl -e`, `node -e`, `eval`)

The blocklist is a secondary boundary — container isolation is the primary defense.

## SSRF protection

The `web_fetch` tool includes multiple layers of SSRF prevention:

- **Private IP blocking** — Rejects requests to 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
- **IPv6 blocking** — Rejects `::1`, link-local, and unique-local addresses
- **Scheme validation** — Only allows HTTP and HTTPS
- **DNS pinning** — Resolves DNS before connecting to prevent rebinding attacks
- **Body size limits** — Prevents memory exhaustion from large responses

## Path traversal prevention

All filesystem tools validate paths against the workspace boundary:

- Rejects paths containing `../`
- Resolves symlinks and checks the canonical path
- Blocks access to files outside the workspace directory
- Rejects URL-encoded bypass attempts (`%2e%2e`)

## Tool approval gate

Policy-based gating for sensitive tools:

```json
{
  "approval": {
    "enabled": true,
    "require_approval": ["shell", "write_file", "delegate"],
    "auto_approve": ["read_file", "memory", "web_search"]
  }
}
```

When enabled, tools in the `require_approval` list will pause and request confirmation before executing.

## Webhook authentication

The webhook channel supports Bearer token authentication with constant-time comparison to prevent timing attacks:

```json
{
  "channels": {
    "webhook": {
      "auth_token": "my-secret-token"
    }
  }
}
```

## Channel message validation

The `message` tool validates that outbound messages target known channels only (telegram, slack, discord, webhook). This prevents the LLM from being tricked into sending messages to arbitrary destinations.

## Plugin security

Plugin command templates automatically shell-escape all parameter values to prevent command injection. Parameters are wrapped in single quotes with proper escaping of embedded quotes.

## Best practices

1. **Always use container isolation in production** — Run `zeptoclaw gateway --containerized`
2. **Set a token budget** — Prevent runaway costs with `token_budget`
3. **Enable the approval gate** — Require approval for destructive tools
4. **Use environment variables for secrets** — Never commit API keys to config files
5. **Restrict the webhook endpoint** — Use auth tokens and IP allowlists
6. **Monitor with telemetry** — Enable Prometheus export for observability
7. **Set agent timeouts** — Prevent long-running sessions with `agent_timeout_secs`
8. **Use tool whitelists** — Restrict sub-agent tools via templates
