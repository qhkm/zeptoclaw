# Changelog

All notable changes to ZeptoClaw will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.4.0] - 2026-02-15

### Added
- **Secret encryption at rest** — XChaCha20-Poly1305 AEAD with Argon2id KDF; `ENC[version:salt:nonce:ciphertext]` format stored in config.json; `secrets encrypt/decrypt/rotate` CLI commands; transparent decryption on config load
- **Tunnel support** — Cloudflare, ngrok, and Tailscale tunnel providers; `--tunnel` gateway flag with auto-detect mode; subprocess lifecycle management
- **Deny-by-default sender allowlists** — `deny_by_default` bool on all channel configs; when true + empty allowlist = reject all messages
- **Memory decay and injection** — Importance-weighted decay scoring for long-term memory; pinned memories auto-injected into system prompt; pre-compaction memory flush
- **Memory pin action** — `pin` action on longterm_memory tool for always-included context
- **OpenAI-compatible provider tests** — 13 tests confirming `api_base` works for Ollama, Groq, Together, Fireworks, LM Studio, vLLM
- **OpenClaw migration** — `zeptoclaw migrate` command to import config and skills from OpenClaw installations
- **Binary plugin system** — JSON-RPC 2.0 stdin/stdout protocol for external tool binaries
- **Reminder tool** — Persistent reminder store with 6 actions; task-manager agent template
- **Custom tools** — CLI-defined tools via `custom_tools` config with compact descriptions
- **Tool profiles** — Named tool subsets for different agent configurations
- **Agent engine resilience** — Structured provider errors, three-tier overflow recovery, circuit breaker on fallback, dynamic tool result budgets, runtime context injection
- **URL watch command** — `zeptoclaw watch <url>` monitors pages for changes with channel notifications
- **Tool discovery CLI** — `zeptoclaw tools list` and `zeptoclaw tools info <name>`
- **Memory CLI** — `zeptoclaw memory list/search/set/delete/stats`
- **Express onboard** — Streamlined setup as default, full wizard behind `--full` flag
- **CLI smoke tests** — Integration test suite for CLI command validation
- **OG meta tags** — Open Graph and Twitter Card meta for landing page

### Changed
- Rebrand positioning to "A complete AI agent runtime in 4MB"
- Tool count increased from 17 to 18 built-in tools

### Security
- Prompt injection detection (17 patterns + 4 regex via Aho-Corasick)
- Secret leak scanning (22 regex patterns)
- Security policy engine (7 rules)
- Input validation (length, null bytes, repetition detection)
- XChaCha20-Poly1305 secret encryption with OWASP-recommended Argon2id params (m=64MB, t=3, p=1)
- Deny-by-default sender allowlists propagated to all channel spawned tasks

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

[0.4.0]: https://github.com/qhkm/zeptoclaw/releases/tag/v0.4.0
[0.2.0]: https://github.com/qhkm/zeptoclaw/releases/tag/v0.2.0
