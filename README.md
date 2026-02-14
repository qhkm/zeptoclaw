<p align="center">
  <img src="assets/mascot-no-bg.png" width="200" alt="Zippy — ZeptoClaw mascot">
</p>
<h1 align="center">ZeptoClaw</h1>
<p align="center">
  <strong>AI assistant framework that fits in 5 megabytes.</strong>
</p>
<p align="center">
  16+ tools &bull; runtime providers: Anthropic + OpenAI &bull; container isolation &bull; multi-tenant &bull; written in Rust
</p>
<p align="center">
  <a href="#install">Install</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="#tools">Tools</a> &bull;
  <a href="#architecture">Architecture</a>
</p>
<p align="center">
  <a href="https://github.com/qhkm/zeptoclaw/actions/workflows/ci.yml"><img src="https://github.com/qhkm/zeptoclaw/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/qhkm/zeptoclaw/releases/latest"><img src="https://img.shields.io/github/v/release/qhkm/zeptoclaw?color=blue" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License"></a>
</p>

---

```
$ zeptoclaw agent "Set up my project workspace"

🤖 ZeptoClaw — I'll set up your workspace.

  [read_file] Reading project structure...
  [shell]     Running cargo check...
  [web_search] Looking up best practices...

→ Created workspace at ~/.zeptoclaw/workspace
→ Found 40+ Rust source files across 17 modules
→ Providers: Anthropic + OpenAI (runtime), OpenRouter/Groq/Zhipu/VLLM/Gemini (registry)
→ Tools: shell, filesystem, web, memory, cron, whatsapp + 10 more

✓ Workspace ready in 1.2s
```

## Why ZeptoClaw?

It started with **OpenClaw** — a TypeScript powerhouse with 52+ modules and 12 channels. It could do everything. But "everything" comes at a cost: complexity, dependencies, and resource bloat.

Then came **NanoClaw** — a forkable assistant in ~5,000 lines of TypeScript. Then **PicoClaw** pushed further — a Go binary that runs on a $10 RISC-V board.

**ZeptoClaw** is the Rust evolution: memory safety, async performance, and container isolation — built for teams who need security and multi-tenancy without sacrificing simplicity.

| | ~5MB binary | ~50ms startup* | ~6MB RAM* | ~37K LOC | single crate |
|---|---|---|---|---|---|

\* Measured on Apple Silicon release builds. Exact numbers vary by workload and hardware.

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
zeptoclaw agent -m "Explain async Rust" --stream

# Use a template
zeptoclaw agent --template researcher -m "Search for Rust agent frameworks"

# Process prompts in batch
zeptoclaw batch --input prompts.txt --output results.jsonl

# Start as a Telegram/Slack/Discord/Webhook gateway
zeptoclaw gateway

# With full container isolation per request
zeptoclaw gateway --containerized
```

## Features

**Multi-Provider LLM** — Runtime execution supports Anthropic and OpenAI today. Provider registry/config entries also include OpenRouter, Groq, Zhipu, VLLM, and Gemini for staged rollout.

**16+ Built-in Tools** — Shell, filesystem, web search, web fetch, memory, cron scheduling, spawn, WhatsApp, Google Sheets, and more. Extend with the `Tool` trait in ~50 lines of Rust.

**Configurable Runtime Isolation** — Shell execution supports Native, Docker, or Apple Container runtimes. The containerized gateway isolates each request when `--containerized` is enabled.

**Multi-Channel Gateway** — Implemented channels: Telegram, Slack, Discord, and Webhook (+ CLI mode). Channel factory with per-channel configuration and unified message bus.

**Hooks System** — Config-driven hooks with `before_tool`, `after_tool`, and `on_error` points. Supports Log, Block, and Notify actions with tool and channel pattern matching. Fully wired into the agent loop.

**Memory & Sessions** — Long-running conversations with context. Persistent memory search and retrieval. Sessions survive restarts.

**Cron & Scheduling** — Schedule recurring tasks with cron expressions. Heartbeat service for proactive check-ins. Background agent spawning for async work.

**Health & Observability** — Built-in health endpoints, usage metrics with atomic counters, structured JSON logging, per-request tracing with tenant isolation.

**Security Hardened** — SSRF prevention, path traversal detection, shell command blocklist, mount validation, workspace-scoped filesystem tools.

**Multi-Tenant** — Run hundreds of tenants on a single VPS. Isolated workspaces, per-tenant config, ~6MB RAM per agent.

## The OpenClaw Family

One vision, four languages. Pick the right tool for the job.

| | OpenClaw | NanoClaw | PicoClaw | **ZeptoClaw** |
|---|---|---|---|---|
| **Language** | TypeScript | TypeScript | Go | **Rust** |
| **Philosophy** | Comprehensive | Hackable | Tiny | **Secure** |
| **Size** | 52+ modules | ~5K LOC | <10MB RAM | **~5MB binary** |
| **Channels** | 12 channels | WhatsApp + skills | Telegram, Discord, QQ | **Telegram, Slack, Discord, Webhook (+ CLI)** |
| **Standout** | Voice, Live Canvas | Agent swarms, forkable | $10 hardware, RISC-V | **Container isolation, multi-tenant** |
| **Best for** | Feature seekers | Developers who read code | Edge & IoT | **Production & enterprise** |

## Tools

| Tool | Description | Config Required |
|---|---|---|
| `shell` | Execute commands (runtime-configurable: Native/Docker/Apple) | - |
| `read_file` | Read file contents | - |
| `write_file` | Write content to files | - |
| `list_dir` | List directory contents | - |
| `edit_file` | Find-and-replace in files | - |
| `web_search` | Search the web via Brave API | Brave API key |
| `web_fetch` | Fetch and extract URL content | - |
| `memory_get` | Retrieve workspace memory | - |
| `memory_search` | Search workspace memory | - |
| `cron` | Schedule recurring tasks | - |
| `spawn` | Delegate background tasks | - |
| `message` | Send messages to chat channels | - |
| `whatsapp_send` | Send WhatsApp messages | Meta Cloud API |
| `google_sheets` | Read/write Google Sheets | Google API |
| `r8r` | Content rating and analysis | - |
| `delegate` | Delegate tasks to specialist sub-agents | - |

## Architecture

```
src/
├── agent/       Agent loop, context builder
├── bus/         Async message bus (pub/sub)
├── channels/    Telegram, Slack, Discord, Webhook (+ CLI mode)
├── config/      Configuration types and loading
├── cron/        Persistent cron scheduler
├── gateway/     Containerized agent proxy
├── health/      Health endpoints, usage metrics
├── heartbeat/   Periodic background tasks
├── memory/      Workspace memory (markdown-based)
├── providers/   Claude + OpenAI runtime, additional providers in registry
├── runtime/     Native, Docker, Apple Container
├── security/    Shell blocklist, path validation, SSRF prevention
├── session/     Session and message persistence
├── skills/      Markdown-based skill system
├── tools/       16+ agent tools
├── utils/       Utility functions
├── error.rs     Error types
├── lib.rs       Library exports (17 modules)
└── main.rs      CLI entry point
```

## Configuration

Config: `~/.zeptoclaw/config.json`

```json
{
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4",
      "max_tokens": 8192
    }
  },
  "providers": {
    "anthropic": { "api_key": "sk-ant-xxx" },
    "openai": { "api_key": "sk-xxx" }
  },
  "channels": {
    "telegram": { "enabled": true, "token": "123456:ABC..." }
  }
}
```

Environment variables override config:
- `ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY`
- `ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY`
- `ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN`

Compile-time model defaults:
- `ZEPTOCLAW_DEFAULT_MODEL`
- `ZEPTOCLAW_CLAUDE_DEFAULT_MODEL`
- `ZEPTOCLAW_OPENAI_DEFAULT_MODEL`

## Multi-Tenant Deployment

Run multiple tenants on a single VPS. Each tenant gets isolated container, config, and data volume.

```bash
./scripts/add-tenant.sh shop-ahmad "BOT_TOKEN" "API_KEY"
./scripts/generate-compose.sh > docker-compose.multi-tenant.yml
docker compose -f docker-compose.multi-tenant.yml up -d
```

## Security

Defense-in-depth, not defense-in-hope:

1. **Runtime Isolation** — configurable Native, Docker, or Apple Container runtime (containerized modes provide process/filesystem/network isolation)
2. **Containerized Gateway** — full agent isolation per request with semaphore concurrency
3. **Shell Blocklist** — regex patterns blocking dangerous commands (rm -rf, reverse shells, etc.)
4. **Path Traversal Protection** — symlink escape detection, workspace-scoped filesystem
5. **SSRF Prevention** — DNS pre-resolution against private IPs, redirect host validation
6. **Input Validation** — URL path injection prevention, spreadsheet ID validation, mount allowlist
7. **Rate Limiting** — cron job caps (50 active, 60s minimum interval), spawn recursion prevention

## CLI Reference

| Command | Description |
|---|---|
| `zeptoclaw onboard` | Interactive setup |
| `zeptoclaw agent -m "..."` | Single message |
| `zeptoclaw agent` | Interactive chat |
| `zeptoclaw gateway` | Start channel gateway |
| `zeptoclaw gateway --containerized` | Gateway with container isolation |
| `zeptoclaw heartbeat` | Trigger heartbeat check |
| `zeptoclaw skills list` | List available skills |
| `zeptoclaw status` | Show config status |

## Development

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt -- --check
```

## License

Apache 2.0 &mdash; see [LICENSE](LICENSE)

---

<p align="center">
  Built by <a href="https://github.com/qhkm">Kitakod Ventures</a> &bull; Part of the <strong>OpenClaw</strong> family
</p>
