# ZeptoClaw vs OpenClaw vs PicoClaw vs NanoClaw vs ZeroClaw

> Factual comparison based on source code analysis. Last updated: 2026-02-26.

Five open-source projects solving the same problem — a self-hosted AI assistant you control — with different tradeoffs.

## At a Glance

| | **OpenClaw** | **PicoClaw** | **NanoClaw** | **ZeroClaw** | **ZeptoClaw** |
|---|---|---|---|---|---|
| **Language** | TypeScript / Node.js | Go | TypeScript / Node.js | Rust | Rust |
| **Binary / Install** | ~100MB+ (Node + 53 deps) | Single binary (<10MB) | Node.js + 10 deps | Single binary (~3.4MB) | Single binary (~4MB) |
| **Memory (RSS)** | ~200MB+ | <10MB | ~50MB+ (Node + containers) | Low (Rust native) | ~6MB |
| **Startup** | Seconds | <1s (on 0.6GHz SBC) | Seconds | <10ms | ~50ms |
| **Built-in tools** | 52 skills | 12 tools | 0 (Claude Agent SDK) | 9 tools | 32 tools + MCP client |
| **Channels** | 14 + 32 extensions | 10 | 1 (WhatsApp) | 7 | 9 |
| **LLM providers** | Anthropic, OpenAI | 5-7 | Claude only | 22+ (OpenAI-compatible) | Claude, OpenAI + retry/fallback stack |
| **Container isolation** | Docker (deployment only) | None | Yes (per-agent, Apple/Docker) | None (planned) | Yes (per-command, Apple/Docker) |
| **Codebase** | ~465K lines | ~20.6K lines | ~3.4K lines | ~27.4K lines | ~106K lines |
| **Tests** | 949 test files | 222 tests | ~413 assertions | ~700+ tests | 3,100+ tests |
| **License** | MIT | MIT | MIT | MIT | Apache 2.0 |

## Tools & Capabilities

| Capability | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| Shell execution | Yes | Yes | Via Claude Agent SDK | Yes (allowlist-based) | Yes (with runtime isolation) |
| Filesystem (read/write/edit) | Yes | Yes (5 tools) | Via Claude Agent SDK | Yes (2 tools) | Yes (3 tools) |
| Web search + fetch | Yes | Yes | Via skills | No | Yes (SSRF-protected) |
| Memory / context persistence | Vector embeddings (SQLite) | Markdown-based | Per-group CLAUDE.md + SQLite | Hybrid search (SQLite FTS5 + vector) | Long-term KV store + workspace memory |
| Cron / scheduling | Yes | Yes | Yes (cron-parser) | Yes | Yes |
| Background tasks (spawn) | Yes | Yes | No | No | Yes (spawn + delegate) |
| Agent swarms / delegation | Multi-agent routing | Subagent manager | Yes (Claude Agent Teams) | No | DelegateTool (parallel fan-out + sequential scratchpad) |
| Hardware I/O (I2C, GPIO, NVS) | No | Yes | No | No | Yes (ESP32, RPi, Arduino, Nucleo) |
| Browser control | Yes (CDP) | No | Via skill (agent-browser) | Yes (agent-browser) | No |
| Google Sheets | No | No | No | No | Yes |
| WhatsApp tool | Yes (Baileys) | Yes | Yes (Baileys, primary) | Yes (Cloud API channel) | Yes (Cloud API) |
| MCP client | No | No | Yes (via Claude Agent SDK) | No | Yes (JSON-RPC 2.0) |
| Plugin system | Yes (32 extensions) | No | Skills (markdown-based) | Skills (TOML) + Composio (1000+ apps) | Yes (JSON manifest) |
| Batch processing | No | No | No | No | Yes (text/JSONL) |
| Agent templates | No | No | No | No | Yes (4 built-in + custom) |
| Tool approval gate | No | No | No | Autonomy levels (ReadOnly/Supervised/Full) | Yes (policy-based) |
| Token budget tracking | No | No | No | No | Yes |
| Cost tracking | No | No | No | No | Yes (8 models) |
| Secret encryption at rest | No | No | No | Yes (ChaCha20-Poly1305) | Yes (XChaCha20-Poly1305 + Argon2id) |
| Tunnel support | Tailscale | No | No | Cloudflare, Tailscale, ngrok, custom | Yes (Cloudflare, ngrok, Tailscale) |

## Channels

| Channel | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| Telegram | Yes | Yes | Planned (via skill) | Yes | Yes |
| Slack | Yes | Yes | Planned (via skill) | Yes | Yes |
| Discord | Yes | Yes | Planned (via skill) | Yes | Yes |
| WhatsApp | Yes (Baileys) | Yes | Yes (Baileys, primary) | Yes (Cloud API) | Yes (bridge + Cloud API) |
| Webhook (generic HTTP) | No | No | No | No | Yes |
| CLI / REPL | Yes | Yes | No | Yes | Yes |
| LINE | Yes | Yes | No | No | No |
| QQ | No | Yes | No | No | No |
| DingTalk | No | Yes | No | No | No |
| Feishu / Lark | No | Yes | No | No | Yes |
| Google Chat | Yes | No | No | No | No |
| Signal | Yes | No | No | No | No |
| iMessage | Yes | No | No | Yes | No |
| Microsoft Teams | Yes | No | No | No | No |
| Matrix | Yes | No | No | Yes | No |
| MaixCAM (device) | No | Yes | No | No | No |

OpenClaw leads on channel breadth (14+32 extensions). PicoClaw covers Chinese platforms. NanoClaw focuses on WhatsApp. ZeroClaw has 7 channels including iMessage and Matrix. ZeptoClaw has 9 channels (Telegram, Slack, Discord, WhatsApp, WhatsApp Cloud, Lark, Email, Webhook, Serial).

## Security

| Feature | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| Workspace path isolation | Yes | Yes | Yes (per-group mount) | Yes (14 system dirs blocked) | Yes |
| Shell command blocklist | Limited | Yes (8 patterns) | No (relies on container) | Yes (allowlist-based) | Yes (regex-based) |
| Prompt injection detection | No | No | No | No | Yes (17 patterns + 4 regex) |
| Secret leak scanning | No | No | No | No | Yes (22 patterns) |
| Security policy engine | No | No | No | No | Yes (7 rules) |
| Input validation | No | No | No | No | Yes (length, null bytes, repetition) |
| SSRF prevention | No | No | No | No | Yes (DNS pinning, private IP blocking) |
| Container isolation (per-agent) | No | No | Yes (Apple/Docker) | No (planned) | Yes (Docker + Apple Container) |
| Mount allowlist / blocklist | No | No | Yes (external allowlist) | Yes (sensitive dotfiles blocked) | Yes (mount policy) |
| Secret encryption at rest | No | No | No | Yes (ChaCha20-Poly1305) | Yes (XChaCha20-Poly1305 + Argon2id) |
| Gateway pairing auth | No | No | No | Yes (6-digit code + Bearer token) | No |
| Sender allowlists | No | No | No | Yes (deny-by-default per channel) | Yes (deny-by-default per channel) |
| DM pairing / auth | Yes | No | No | No | No |
| Audit trails | Yes | No | No | No | No |
| Tool approval gate | No | No | No | Autonomy levels | Yes |

ZeroClaw and ZeptoClaw take different security approaches. ZeroClaw focuses on access control — gateway pairing, sender allowlists, secret encryption, autonomy levels. ZeptoClaw focuses on content security — prompt injection detection, leak scanning, SSRF prevention, policy engine — and now also has secret encryption at rest (XChaCha20-Poly1305 + Argon2id) and deny-by-default sender allowlists. Both block sensitive filesystem paths.

**Known vulnerabilities:** OpenClaw's ecosystem has seen CVE-2026-25253 (CVSS 8.8 — WebSocket hijacking to RCE), ClawHavoc (341 malicious skills), and 42,000 exposed instances. The other projects have no known CVEs as of this writing.

## Memory & Context

| Feature | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| Session persistence | Yes | Yes | Yes (SQLite) | Yes (SQLite) | Yes |
| Conversation history CLI | No | No | No | No | Yes (list, show, search, cleanup) |
| Long-term memory | Vector embeddings (SQLite) | Markdown files | Per-group CLAUDE.md | SQLite + Markdown backends | JSON KV store (categories, tags) |
| Memory search | Hybrid (keyword + vector) | Text search | No (file-based) | Hybrid (FTS5 BM25 + vector cosine) | Fuzzy search + chunked scoring |
| Embedding support | Yes (multiple) | No | No | Yes (OpenAI, custom URL) | No |
| Global shared memory | No | No | Yes (read-only global dir) | No | No |
| Token budget tracking | No | No | No | No | Yes (atomic per-session) |
| Context compaction | Yes (/compact) | No | No | No | Yes (auto-triggered) |
| Cost tracking | No | No | No | No | Yes (8 models, per-provider) |
| Metrics / telemetry | OpenTelemetry (extension) | No | No | Observer trait (Log/Noop/Multi) | Yes (Prometheus + JSON) |

ZeroClaw has the strongest memory search after OpenClaw — hybrid SQLite FTS5 (BM25 scoring) + vector embeddings with cosine similarity, all built-in with no external dependencies. ZeptoClaw uses simpler text matching but adds token budgeting and context compaction.

## Architecture

| Aspect | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| Runtime | Node.js >= 22.12 | Go binary | Node.js 20+ | Rust binary (Tokio) | Rust binary (Tokio async) |
| Concurrency | Event loop | Goroutines | Per-group queue | Tokio async | Tokio async + futures::join_all |
| Agent execution | In-process | In-process | Container per group | In-process (native only) | In-process or container per command |
| Deployment | VPS, Docker Compose, Tailscale | Binary, Docker | Docker, Apple Container | Binary + tunnel (Cloudflare/Tailscale/ngrok) | Binary, Docker, systemd, one-click cloud |
| Companion apps | macOS, iOS, Android | No | No | No | No |
| Voice support | Wake + Talk Mode | Groq transcription | No | No | No |
| Multi-tenant | Yes (workspace isolation) | No | Yes (group isolation) | No | Yes (~6MB per tenant) |
| Config format | JSON | JSON | TypeScript | TOML | JSON |
| Config validation | Yes (doctor command) | Basic | No | Yes (doctor command) | Yes (config check) |
| Onboarding | Wizard | No | Claude Code skill | Interactive onboard | Interactive onboard |
| Core traits | N/A | N/A | Channel interface | 8 (Provider, Channel, Memory, Tool, Observer, Runtime, Tunnel, Embedding) | 4 (LLMProvider, Channel, Tool, ContainerRuntime) |

## Design Philosophy

| | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| **Philosophy** | Feature-complete platform | Ultra-portable IoT agent | Minimal, forkable, secure | Zero overhead, pluggable everything | Dense, secure, agent-first |
| **Target user** | Power users, businesses | IoT/embedded developers | Single-user, personal | Provider-agnostic developers | Developers, multi-tenant ops |
| **Extensibility** | Plugin ecosystem | None | Fork & modify code | 8 swappable traits + Composio | Plugins + MCP + config |
| **Complexity** | High (~465K lines) | Low (~20.6K lines) | Minimal (~3.4K lines) | Moderate (~27.4K lines) | Moderate (~106K lines) |
| **Tagline** | "Personal AI assistant" | "AI for $10 hardware" | "Understand in 8 minutes" | "Zero overhead. Zero compromise." | "Ultra-lightweight personal AI assistant" |

## When to Use What

| Scenario | Recommended | Why |
|---|---|---|
| Maximum channel coverage | **OpenClaw** | 14 channels + 32 extensions, companion apps |
| Chinese platform integration | **PicoClaw** | QQ, DingTalk, Feishu built-in |
| Embedded / IoT devices | **PicoClaw** or **ZeptoClaw** | PicoClaw: I2C/SPI on $10 SBCs. ZeptoClaw: ESP32/RPi/Arduino/Nucleo with GPIO, I2C, NVS |
| WhatsApp-first personal assistant | **NanoClaw** | WhatsApp primary, group isolation, tiny codebase |
| Smallest codebase to fork | **NanoClaw** | 3.4K lines, designed to be forked |
| Most LLM providers | **ZeroClaw** | 22+ providers, any OpenAI-compatible endpoint |
| Best memory search (no external deps) | **ZeroClaw** | Built-in hybrid FTS5 + vector embeddings |
| Secrets encryption at rest | **ZeroClaw** or **ZeptoClaw** | ZeroClaw: ChaCha20-Poly1305. ZeptoClaw: XChaCha20-Poly1305 + Argon2id |
| Tunnel support (Cloudflare/ngrok) | **ZeroClaw** or **ZeptoClaw** | ZeroClaw: 4 options. ZeptoClaw: Cloudflare, ngrok, Tailscale |
| Security-sensitive deployment | **ZeptoClaw** | Multi-layer safety, container isolation, leak detection |
| Resource-constrained server | **ZeptoClaw** or **ZeroClaw** | Both Rust, both <5MB binary |
| Multi-tenant hosting | **ZeptoClaw** | ~6MB per tenant, container isolation per request |
| Plugin ecosystem | **OpenClaw** | 32 extensions, mature plugin system |
| Voice + mobile companion | **OpenClaw** | Wake Mode, Talk Mode, iOS/Android apps |
| Batch processing / automation | **ZeptoClaw** | Batch mode, routines, cron, agent templates |
| Cost-conscious API usage | **ZeptoClaw** | Token budget, cost tracking, retry + fallback |
| Browser automation | **OpenClaw** or **ZeroClaw** | OpenClaw: CDP. ZeroClaw: agent-browser |
| Hardware sensor integration | **PicoClaw** or **ZeptoClaw** | PicoClaw: I2C, SPI. ZeptoClaw: GPIO, I2C, NVS for 4 board families |
| Agent swarms | **NanoClaw** or **ZeptoClaw** | NanoClaw: Claude Agent Teams. ZeptoClaw: DelegateTool (parallel fan-out + sequential scratchpad) |
| 1000+ app integrations | **ZeroClaw** | Composio integration (OAuth apps) |

## Project Status

| | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | ZeptoClaw |
|---|---|---|---|---|---|
| **Stage** | Established (160K+ stars) | New (5K+ stars in first week) | New | New | New (v0.6.0) |
| **Community** | Large | Growing fast | Small | Early | Early |
| **Codebase** | ~465K lines TS | ~20.6K lines Go | ~3.4K lines TS | ~27.4K lines Rust | ~11K lines Rust |
| **Test coverage** | 949 test files | 222 tests | ~413 assertions | ~700+ tests | 1,314 tests |

---

**OpenClaw** is the most feature-rich — 52 skills, 14 channels, companion apps, voice support. The tradeoff is size (100MB+), resource usage, and security incidents (CVE-2026-25253, ClawHavoc supply chain attack).

**PicoClaw** is the most portable — Go binary on $10 RISC-V boards with I2C/SPI hardware tools and Chinese platform channels. The tradeoff is no security layer (no injection detection, no leak scanning, no container isolation).

**NanoClaw** is the most minimal — 3.4K lines of TypeScript designed to be forked and understood in 8 minutes. Full container isolation per group, WhatsApp-first. The tradeoff is Claude-only (no other LLM providers), one channel, and no built-in tools.

**ZeroClaw** is the most provider-agnostic — 22+ LLM providers, built-in hybrid memory search (FTS5 + vector), secret encryption at rest, and 7 channels including iMessage and Matrix. The tradeoff is no container isolation (planned), no content security (injection/leak detection), and no multi-tenant support.

**ZeptoClaw** is the most secure and operationally complete — multi-layer safety, per-command container isolation, secret encryption at rest (XChaCha20-Poly1305 + Argon2id), sender allowlists, tunnel support (Cloudflare/ngrok/Tailscale), token budgeting, cost tracking, batch mode, hardware peripherals (ESP32/RPi/Arduino/Nucleo), and parallel agent swarms in a 4MB binary. 9 channels including Serial for embedded devices. The tradeoff is no companion apps.

All five are open source, self-hosted, and built for developers who want to own their AI assistant. The right choice depends on your priorities: features (OpenClaw), portability (PicoClaw), simplicity (NanoClaw), provider flexibility (ZeroClaw), or security and efficiency (ZeptoClaw).
