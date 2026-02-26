<p align="center">
  <img src="assets/mascot-no-bg.png" width="200" alt="Zippy — ZeptoClaw mascot">
</p>
<h1 align="center">ZeptoClaw</h1>
<p align="center">
  <strong>Ultra-lightweight personal AI assistant.</strong>
</p>
<p align="center">
  <a href="https://zeptoclaw.com/docs/"><img src="https://img.shields.io/badge/docs-zeptoclaw.com-3b82f6?style=for-the-badge&logo=bookstack&logoColor=white" alt="Documentation"></a>
</p>
<p align="center">
  <a href="https://github.com/qhkm/zeptoclaw/actions/workflows/ci.yml"><img src="https://github.com/qhkm/zeptoclaw/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/qhkm/zeptoclaw/releases/latest"><img src="https://img.shields.io/github/v/release/qhkm/zeptoclaw?color=blue" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License"></a>
</p>

---

```
$ zeptoclaw agent --stream -m "Analyze our API for security issues"

🤖 ZeptoClaw — Streaming analysis...

  [web_fetch]        Fetching API docs...
  [shell]            Running integration tests...
  [longterm_memory]  Storing findings...

→ Found 12 endpoints, 3 missing auth headers, 1 open redirect
→ Saved findings to long-term memory under "api-audit"

✓ Analysis complete in 4.2s
```

We studied the best AI assistants — and their tradeoffs. OpenClaw's integrations without the 100MB. NanoClaw's security without the TypeScript bundle. PicoClaw's size without the bare-bones feature set. One Rust binary with 29 tools, 9 channels, 9 providers, and 6 sandbox runtimes.

<p align="center">
  <img src="https://img.shields.io/badge/binary-~4MB-3b82f6" alt="~4MB binary">
  <img src="https://img.shields.io/badge/startup-~50ms-3b82f6" alt="~50ms startup">
  <img src="https://img.shields.io/badge/RAM-~6MB-3b82f6" alt="~6MB RAM">
  <img src="https://img.shields.io/badge/tests-2%2C880%2B-3b82f6" alt="2,880+ tests">
  <img src="https://img.shields.io/badge/providers-9-3b82f6" alt="9 providers">
</p>

## Why ZeptoClaw

We studied what works — and what doesn't.

**OpenClaw** proved an AI assistant can handle 12 channels and 100+ skills. But it costs 100MB and 400K lines. **NanoClaw** proved security-first is possible. But it's still 50MB of TypeScript. **PicoClaw** proved AI assistants can run on $10 hardware. But it stripped out everything to get there.

**ZeptoClaw** took notes. The integrations, the security, the size discipline — without the tradeoffs each one made. One 4MB Rust binary that starts in 50ms, uses 6MB of RAM, and ships with container isolation, prompt injection detection, and a circuit breaker provider stack.

## Security

AI agents execute code. Most frameworks trust that nothing will go wrong.

The OpenClaw ecosystem has seen CVE-2026-25253 (CVSS 8.8 — cross-site WebSocket hijacking to RCE), ClawHavoc (341 malicious skills, 9,000+ compromised installations), and 42,000 exposed instances with auth bypass. ZeptoClaw was built with this threat model in mind.

| Layer | What it does |
|-------|-------------|
| **6 Sandbox Runtimes** | Docker, Apple Container, Landlock, Firejail, Bubblewrap, or native — per request |
| **Prompt Injection Detection** | Aho-Corasick multi-pattern matcher (17 patterns) + 4 regex rules |
| **Secret Leak Scanner** | 22 regex patterns catch API keys, tokens, and credentials before they reach the LLM |
| **Policy Engine** | 7 rules blocking system file access, crypto key extraction, SQL injection, encoded exploits |
| **Input Validator** | 100KB limit, null byte detection, whitespace ratio analysis, repetition detection |
| **Shell Blocklist** | Regex patterns blocking reverse shells, `rm -rf`, privilege escalation |
| **SSRF Prevention** | DNS pinning, private IP blocking, IPv6 transition guard, scheme validation |
| **Chain Alerting** | Detects dangerous tool call sequences (write→execute, memory→execute) |
| **Tool Approval Gate** | Require explicit confirmation before executing dangerous tools |

Every layer runs by default. No flags to remember, no config to enable.

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://raw.githubusercontent.com/qhkm/zeptoclaw/main/install.sh | sh

# Homebrew
brew install qhkm/tap/zeptoclaw

# Docker
docker pull ghcr.io/qhkm/zeptoclaw:latest

# Build from source
cargo install zeptoclaw --git https://github.com/qhkm/zeptoclaw
```

## Quick Start

```bash
# Interactive setup (walks you through API keys, channels, workspace)
zeptoclaw onboard

# Talk to your agent
zeptoclaw agent -m "Hello, set up my workspace"

# Stream responses token-by-token
zeptoclaw agent --stream -m "Explain async Rust"

# Use a built-in template
zeptoclaw agent --template researcher -m "Search for Rust agent frameworks"

# Process prompts in batch
zeptoclaw batch --input prompts.txt --output results.jsonl

# Start as a Telegram/Slack/Discord/Webhook gateway
zeptoclaw gateway

# With full container isolation per request
zeptoclaw gateway --containerized
```

## Migrate from OpenClaw

Already running OpenClaw? ZeptoClaw can import your config and skills in one command.

```bash
# Auto-detect OpenClaw installation (~/.openclaw, ~/.clawdbot, ~/.moldbot)
zeptoclaw migrate

# Specify path manually
zeptoclaw migrate --from /path/to/openclaw

# Preview what would be migrated (no files written)
zeptoclaw migrate --dry-run

# Non-interactive (skip confirmation prompts)
zeptoclaw migrate --yes
```

The migration command:
- Converts provider API keys, model settings, and channel configs
- Copies skills to `~/.zeptoclaw/skills/`
- Backs up your existing ZeptoClaw config before overwriting
- Validates the migrated config and reports any issues
- Lists features that can't be automatically ported

Supports JSON and JSON5 config files (comments, trailing commas, unquoted keys).

## Deploy

<p align="center">
  <a href="https://cloud.digitalocean.com/apps/new?repo=https://github.com/qhkm/zeptoclaw/tree/main"><img src="https://img.shields.io/badge/DigitalOcean-0080FF?style=for-the-badge&logo=digitalocean&logoColor=white" alt="Deploy to DigitalOcean"></a>
  <a href="https://railway.com/deploy?template=https://github.com/qhkm/zeptoclaw"><img src="https://img.shields.io/badge/Railway-0B0D0E?style=for-the-badge&logo=railway&logoColor=white" alt="Deploy to Railway"></a>
  <a href="https://render.com/deploy?repo=https://github.com/qhkm/zeptoclaw"><img src="https://img.shields.io/badge/Render-46E3B7?style=for-the-badge&logo=render&logoColor=white" alt="Deploy to Render"></a>
  <a href="https://fly.io/docs/hands-on/"><img src="https://img.shields.io/badge/Fly.io-6E42C1?style=for-the-badge&logo=fly.io&logoColor=white" alt="Deploy to Fly.io"></a>
</p>

### Any VPS

```bash
curl -fsSL https://zeptoclaw.com/setup.sh | bash
```

Installs the binary and prints next steps. Run `zeptoclaw onboard` to configure providers and channels.

## Providers

ZeptoClaw supports 9 LLM providers. All OpenAI-compatible endpoints work out of the box.

| Provider | Config key | Setup |
|----------|------------|-------|
| **Anthropic** | `anthropic` | `api_key` |
| **OpenAI** | `openai` | `api_key` |
| **OpenRouter** | `openrouter` | `api_key` |
| **Groq** | `groq` | `api_key` |
| **Ollama** | `ollama` | `api_key` (any value) |
| **VLLM** | `vllm` | `api_key` (any value) |
| **Google Gemini** | `gemini` | `api_key` |
| **NVIDIA NIM** | `nvidia` | `api_key` |
| **Zhipu (GLM)** | `zhipu` | `api_key` |

Configure in `~/.zeptoclaw/config.json` or via environment variables:

```json
{
  "providers": {
    "openrouter": { "api_key": "sk-or-..." },
    "ollama": { "api_key": "ollama" }
  },
  "agents": { "defaults": { "model": "anthropic/claude-sonnet-4" } }
}
```

```bash
export ZEPTOCLAW_PROVIDERS_GROQ_API_KEY=gsk_...
```

Any provider's base URL can be overridden with `api_base` for proxies or self-hosted endpoints. See the [provider docs](https://zeptoclaw.com/docs/concepts/providers/) for full details.

## Features

### Core

| Feature | What it does |
|---------|-------------|
| **Multi-Provider LLM** | 9 providers with SSE streaming, retry with backoff + budget cap, auto-failover |
| **29 Tools + Plugins** | Shell, filesystem, web, git, stripe, PDF, transcription, Android ADB, and more |
| **Tool Composition** | Create new tools from natural language descriptions — composable `{{param}}` templates |
| **Agent Swarms** | Delegate to sub-agents with parallel fan-out, aggregation, and cost-aware routing |
| **Library Facade** | Embed as a crate — `ZeptoAgent::builder().provider(p).tool(t).build()` for Tauri/GUI apps |
| **Batch Mode** | Process hundreds of prompts from text/JSONL files with template support |
| **Agent Modes** | Observer, Assistant, Autonomous — category-based tool access control |

### Channels & Integration

| Feature | What it does |
|---------|-------------|
| **9-Channel Gateway** | Telegram, Slack, Discord, WhatsApp (bridge + Cloud), Lark, Email, Webhook, Serial — unified message bus |
| **Persona System** | Per-chat personality switching via `/persona` command with LTM persistence |
| **Plugin System** | JSON manifest plugins auto-discovered from `~/.zeptoclaw/plugins/` |
| **Hooks** | `before_tool`, `after_tool`, `on_error` with Log, Block, and Notify actions |
| **Cron & Heartbeat** | Schedule recurring tasks, proactive check-ins, background spawning |
| **Memory & History** | Workspace memory, long-term key-value store, conversation history |

### Security & Ops

| Feature | What it does |
|---------|-------------|
| **6 Sandbox Runtimes** | Docker, Apple Container, Landlock, Firejail, Bubblewrap, or native |
| **Gateway Startup Guard** | Degrade gracefully after N crashes — prevents crash loops |
| **Channel Supervisor** | Auto-restart dead channels with cooldown and max-restart limits |
| **Tool Approval Gate** | Policy-based gating — require confirmation for dangerous tools |
| **SSRF Prevention** | DNS pinning, private IP blocking, IPv6 transition guard, scheme validation |
| **Shell Blocklist** | Regex patterns blocking reverse shells, rm -rf, privilege escalation |
| **Token Budget & Cost** | Per-session budget enforcement, per-model cost estimation for 8 models |
| **Rich Health Endpoint** | `/health` with version, uptime, RSS, usage metrics, component checks |
| **Telemetry** | Prometheus + JSON metrics export, structured logging, per-tenant tracing |
| **Self-Update** | `zeptoclaw update` downloads latest release from GitHub |
| **Multi-Tenant** | Hundreds of tenants on one VPS — isolated workspaces, ~6MB RAM each |

> **Full documentation** — [zeptoclaw.com/docs](https://zeptoclaw.com/docs/) covers configuration, environment variables, CLI reference, deployment guides, and more.

## Inspired By

ZeptoClaw is inspired by projects in the open-source AI agent ecosystem — OpenClaw, NanoClaw, and PicoClaw — each taking a different approach to the same problem. ZeptoClaw's contribution is Rust's memory safety, async performance, and container isolation for production multi-tenant deployments.

## Usage

```bash
# CLI agent (one-shot)
zeptoclaw agent -m "Summarize this repo"

# Streaming output
zeptoclaw agent --stream -m "Explain async Rust"

# Use a template (researcher, coder, task-manager, etc.)
zeptoclaw agent --template coder -m "Add error handling to main.rs"

# Batch process prompts from a file
zeptoclaw batch --input prompts.txt --output results.jsonl --format jsonl

# Run as a multi-channel gateway (Telegram, Slack, Discord, etc.)
zeptoclaw gateway

# With container isolation per request
zeptoclaw gateway --containerized

# Manage long-term memory
zeptoclaw memory set project:name "ZeptoClaw" --category project
zeptoclaw memory search "project"

# Self-update to latest release
zeptoclaw update

# Encrypt secrets in config
zeptoclaw secrets encrypt
```

## Development

```bash
# Build
cargo build

# Run all tests (~2,880 total)
cargo test

# Lint and format (required before every PR)
cargo clippy -- -D warnings
cargo fmt -- --check
```

See [CLAUDE.md](CLAUDE.md) for full architecture reference, [AGENTS.md](AGENTS.md) for coding guidelines, and [docs/](docs/) for benchmarks, multi-tenant deployment, and performance guides.

## Contributing

We welcome contributions! Please read **[CONTRIBUTING.md](CONTRIBUTING.md)** for:

- How to set up your fork and branch from upstream
- Issue-first workflow (open an issue before coding)
- Pull request process and quality gates
- Guides for adding new tools, channels, and providers

## License

Apache 2.0 — see [LICENSE](LICENSE)

---

<p align="center">
  <em>ZeptoClaw — Because your AI assistant shouldn't need more RAM than your text editor.</em>
</p>
<p align="center">
  Built by <a href="https://github.com/qhkm">Aisar Labs</a>
</p>
