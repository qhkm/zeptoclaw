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

## Prompt injection detection

ZeptoClaw includes a multi-layered prompt injection detector that runs on all LLM inputs:

- **Aho-Corasick matcher** — 17 patterns for common injection phrases ("ignore previous instructions", "system prompt override", etc.)
- **Regex rules** — 4 additional patterns for encoded or obfuscated injection attempts

The safety layer is enabled by default. Configure via:

```bash
export ZEPTOCLAW_SAFETY_ENABLED=true  # default
```

## Secret leak scanning

Before any content reaches the LLM, a leak detector scans for 22 regex patterns covering:

- API keys (AWS, OpenAI, Anthropic, Google, Stripe, etc.)
- Authentication tokens (JWT, Bearer, OAuth)
- Private keys (RSA, SSH, PGP)
- Database connection strings
- Cloud credentials

Detected secrets can be blocked, redacted, or warned about based on configuration.

```bash
export ZEPTOCLAW_SAFETY_LEAK_DETECTION_ENABLED=true  # default
```

## Security policy engine

A 7-rule policy engine enforces:

1. System file access prevention
2. Crypto key extraction blocking
3. SQL injection pattern detection
4. Shell injection prevention
5. Encoded exploit detection (base64, hex payloads)
6. Privilege escalation blocking
7. Data exfiltration prevention

## Input validation

All inputs are validated before processing:

- **Length limit** — 100KB maximum input size
- **Null byte detection** — Blocks null bytes used in injection attacks
- **Whitespace ratio analysis** — Detects padding-based attacks
- **Repetition detection** — Catches repeated-pattern attacks

## Secret encryption at rest

API keys and tokens in `config.json` can be encrypted using XChaCha20-Poly1305 AEAD with Argon2id key derivation:

```bash
# Encrypt all plaintext secrets
zeptoclaw secrets encrypt

# Decrypt for editing
zeptoclaw secrets decrypt

# Rotate to a new master key
zeptoclaw secrets rotate
```

Encrypted values are stored as `ENC[version:salt:nonce:ciphertext]` in the config file. The master key can be provided via:
- `ZEPTOCLAW_MASTER_KEY` environment variable (hex-encoded 32 bytes)
- A key file
- Interactive prompt

## Sender allowlists

All channels support deny-by-default sender allowlists:

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "...",
      "deny_by_default": true,
      "allow_from": ["123456789"]
    }
  }
}
```

When `deny_by_default` is `true` and the allowlist is empty, all messages are rejected.

## Best practices

1. **Always use container isolation in production** — Run `zeptoclaw gateway --containerized`
2. **Set a token budget** — Prevent runaway costs with `token_budget`
3. **Enable the approval gate** — Require approval for destructive tools
4. **Use environment variables for secrets** — Never commit API keys to config files
5. **Restrict the webhook endpoint** — Use auth tokens and IP allowlists
6. **Monitor with telemetry** — Enable Prometheus export for observability
7. **Set agent timeouts** — Prevent long-running sessions with `agent_timeout_secs`
8. **Use tool whitelists** — Restrict sub-agent tools via templates
9. **Encrypt secrets at rest** — Use `zeptoclaw secrets encrypt` to protect API keys in config
10. **Enable sender allowlists** — Use `deny_by_default: true` on production channels
