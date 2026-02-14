# Changelog

All notable changes to ZeptoClaw will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

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

[0.2.0]: https://github.com/qhkm/zeptoclaw/releases/tag/v0.2.0
