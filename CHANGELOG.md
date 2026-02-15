# Changelog

All notable changes to ZeptoClaw will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **Memory decay scoring** — `importance` field on memories with 30-day half-life decay (`importance * 0.5^(age_days / 30)`); pinned entries exempt
- **Auto memory injection** — Pinned memories automatically injected into system prompt at agent startup via `ContextBuilder::with_memory_context()`
- **Pre-compaction memory flush** — Silent LLM turn saves important facts and consolidates duplicates before context window shrinks (10s timeout)
- **Pin action** — New `pin` action on `longterm_memory` tool (shorthand for `set` with `category="pinned"`)
- **Importance weighting** — Configurable `importance` parameter (0.0–2.0) on memory `set` action; higher = decays slower

## [0.3.1] - 2026-02-15

### Added
- **Secret encryption** — `secrets encrypt/decrypt/rotate` CLI commands with XChaCha20-Poly1305 + Argon2id
- **Transparent config decryption** — `ENC[...]` values in config.json auto-decrypted at load time
- **Tunnel support** — `--tunnel` flag on gateway for Cloudflare, ngrok, and Tailscale tunnels
- **Sender allowlists** — `deny_by_default` mode for channel sender filtering

## [0.3.0] - 2026-02-14

### Added
- **OpenAI-compatible providers** — Groq, Ollama, Gemini, and any OpenAI-compatible API via `api_base` config
- **WhatsApp channel** — WhatsApp via whatsmeow-rs bridge with CLI channel setup/test
- **Binary plugin system** — Standalone executables communicating via JSON-RPC 2.0 over stdin/stdout
- **Reminder tool** — Persistent reminders with add/complete/snooze/overdue actions and cron delivery
- **OpenClaw skills compatibility** — Reads `metadata.openclaw` and `metadata.zeptoclaw` in skill manifests
- **SOUL.md identity** — Auto-detected agent personality file prepended to system prompt
- **Token efficiency** — Compact tool descriptions, custom CLI-defined tools, tool profiles
- **UX overhaul** — Express onboard, memory CLI, tools CLI, watch command, actionable error messages
- **Agent engine resilience** — Structured provider errors, context overflow recovery, circuit breaker, runtime context injection
- **Dependency manager** — `HasDependencies` trait, GitHub Release/Docker/NPM/Pip fetcher, JSON registry

## [0.2.0] - 2026-02-14

First public release.

### Added
- **Streaming responses** — Token-by-token SSE streaming for Claude and OpenAI providers (`--stream` flag)
- **Agent swarms** — DelegateTool creates specialist sub-agents with role-specific system prompts and tool whitelists
- **Plugin system** — JSON manifest-based plugin discovery and registration with PluginTool adapter
- **Agent templates** — Pre-configured agent profiles (coder, researcher, etc.) with `--template` flag
- **4 channels** — Telegram, Slack (outbound), Discord (Gateway WebSocket + REST), Webhook (HTTP POST inbound)
- **Batch mode** — Process multiple prompts from text/JSONL files with `batch` CLI command
- **Conversation history** — CLI commands to list, search, and clean up past sessions
- **Long-term memory** — Persistent key-value store with categories, tags, and keyword search
- **Token budget** — Per-session token budget tracking with atomic counters
- **Structured output** — JSON and JSON Schema output format support for OpenAI and Claude
- **Tool approval** — Configurable approval gate checked before tool execution
- **Retry provider** — Exponential backoff wrapper for 429/5xx errors
- **Fallback provider** — Automatic primary-to-secondary provider failover
- **Cost tracking** — Per-provider/model cost accumulation with pricing tables for 8 models
- **Telemetry export** — Prometheus text exposition and JSON metrics rendering
- **Hooks system** — Config-driven before_tool, after_tool, on_error hooks with pattern matching
- **17 built-in tools** — shell, filesystem (read/write/list/edit), web search, web fetch, memory, cron, spawn, delegate, WhatsApp, Google Sheets, message, long-term memory, r8r
- **Container isolation** — Native, Docker, and Apple Container runtimes
- **Multi-tenant deployment** — Per-tenant isolation with Docker Compose templates
- **Cross-platform CI/CD** — GitHub Actions for test/lint/fmt, cross-platform release builds (4 targets), Docker image push

### Security
- Shell command blocklist with regex patterns
- Path traversal protection with symlink escape detection
- SSRF prevention with DNS pre-resolution against private IPs
- Workspace-scoped filesystem tools
- Mount allowlist validation
- Cron job caps and spawn recursion prevention

[Unreleased]: https://github.com/qhkm/zeptoclaw/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/qhkm/zeptoclaw/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/qhkm/zeptoclaw/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/qhkm/zeptoclaw/releases/tag/v0.2.0
