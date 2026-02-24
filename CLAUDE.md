# ZeptoClaw

Ultra-lightweight personal AI assistant. The best of OpenClaw's integrations, NanoClaw's security, and PicoClaw's minimalism — without their tradeoffs.

## Quick Reference

```bash
# Build
cargo build --release

# Build with Android device control
cargo build --release --features android

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt

# Test counts (cargo test)
# lib: 2533 (2572 with --features hardware), main: 91, cli_smoke: 23, e2e: 13, integration: 70, doc: 147 (121 passed, 26 ignored)

# Version
./target/release/zeptoclaw --version

# Run agent
./target/release/zeptoclaw agent -m "Hello"

# Run agent with streaming
./target/release/zeptoclaw agent -m "Hello" --stream

# Run gateway (Telegram bot)
./target/release/zeptoclaw gateway

# Telegram model switching (in chat)
/model
/model list
/model reset
/model <provider:model>

# Telegram persona switching (in chat)
/persona              # show current persona
/persona list         # show available presets
/persona concise      # switch to preset
/persona Be a pirate  # set custom persona
/persona reset        # clear per-chat override

# Run gateway with container isolation
./target/release/zeptoclaw gateway --containerized          # auto-detect
./target/release/zeptoclaw gateway --containerized docker   # force Docker
./target/release/zeptoclaw gateway --containerized apple    # force Apple Container (macOS)

# Validate configuration
./target/release/zeptoclaw config check

# Heartbeat and skills
./target/release/zeptoclaw heartbeat --show
./target/release/zeptoclaw skills list

# Conversation history
./target/release/zeptoclaw history list [--limit 20]
./target/release/zeptoclaw history show <query>
./target/release/zeptoclaw history cleanup [--keep 50]

# Agent templates
./target/release/zeptoclaw template list
./target/release/zeptoclaw template show coder
./target/release/zeptoclaw agent --template researcher -m "Search for..."
./target/release/zeptoclaw agent --template task-manager -m "Add task: finish proposal by Friday"

# Batch mode (process multiple prompts from file)
./target/release/zeptoclaw batch --input prompts.txt
./target/release/zeptoclaw batch --input prompts.jsonl --output results.jsonl --format jsonl
./target/release/zeptoclaw batch --input prompts.txt --template coder --stop-on-error

# Secret encryption
./target/release/zeptoclaw secrets encrypt   # Encrypt plaintext secrets in config
./target/release/zeptoclaw secrets decrypt   # Decrypt for editing
./target/release/zeptoclaw secrets rotate    # Re-encrypt with new key

# Gateway with tunnel
./target/release/zeptoclaw gateway --tunnel cloudflare
./target/release/zeptoclaw gateway --tunnel ngrok
./target/release/zeptoclaw gateway --tunnel tailscale
./target/release/zeptoclaw gateway --tunnel auto

# Channel management
./target/release/zeptoclaw channel list
./target/release/zeptoclaw channel setup whatsapp
./target/release/zeptoclaw channel test whatsapp

# Onboard (express setup by default)
./target/release/zeptoclaw onboard
./target/release/zeptoclaw onboard --full    # full 10-step wizard

# Memory management
./target/release/zeptoclaw memory list [--category user]
./target/release/zeptoclaw memory search "project name"
./target/release/zeptoclaw memory set user:name "Your Name" --category user --tags "profile,name"
./target/release/zeptoclaw memory delete user:name
./target/release/zeptoclaw memory stats

# Tool discovery
./target/release/zeptoclaw tools list
./target/release/zeptoclaw tools info web_search

# Watch URLs for changes
./target/release/zeptoclaw watch https://example.com --interval 1h --notify telegram

# Release (requires cargo-release: cargo install cargo-release)
cargo release patch          # preview bump 0.5.x → 0.5.x+1 (dry-run by default)
cargo release minor          # preview bump 0.5.x → 0.6.0
cargo release patch --execute  # actually bump, commit, tag, push, publish to crates.io

# Self-update
./target/release/zeptoclaw update              # update to latest
./target/release/zeptoclaw update --check      # check without downloading
./target/release/zeptoclaw update --version v0.5.2  # specific version
./target/release/zeptoclaw update --force      # re-download even if current
```

## Agent Workflow — Task Tracking Protocol

Every Claude Code session in this repo MUST follow these three rules:

### 1. Session Start — Check open issues

Before starting any work, run:

```bash
gh issue list --repo qhkm/zeptoclaw --state open --limit 20
```

Present the open issues to the user and ask what they want to work on. If they pick an existing issue, reference it throughout the session.

### 2. New Work — Create issue first

When the user requests work that has no matching open issue (new feature, bug fix, refactor), create one **before writing code**:

```bash
gh issue create --repo qhkm/zeptoclaw \
  --title "feat: short description" \
  --label "feat,area:tools" \
  --body "Brief description of the work."
```

Use labels from: `bug`, `feat`, `rfc`, `chore`, `docs` + `area:tools`, `area:channels`, `area:providers`, `area:safety`, `area:config`, `area:cli`, `area:memory` + `P1-critical`, `P2-high`, `P3-normal`.

Skip issue creation only for trivial changes (typo fixes, one-line tweaks).

### 3. Session End — Link and close

- If creating a PR: include `Closes #N` in the PR body
- If committing directly to main: close the issue with `gh issue close N --comment "Done in <commit-sha>"`
- Update `CLAUDE.md` and `AGENTS.md` per the post-implementation checklist

## Pre-Push Checklist (MANDATORY)

Before EVERY `git push`, run these checks **in the worktree you are pushing from**. Do NOT skip any step. CI will reject the PR if these fail.

```bash
# 1. Format (MUST pass — CI fails on any diff)
cargo fmt

# 2. Clippy (MUST pass — CI fails on any warning)
cargo clippy -- -D warnings

# 3. Tests (MUST pass)
cargo test --lib

# 4. Verify clean format (no unstaged changes from fmt)
cargo fmt -- --check
```

**When delegating to subagents:** The parent agent MUST run `cargo fmt` and `cargo fmt -- --check` after the subagent completes, before committing. Subagents frequently skip formatting.

**Quick one-liner for CI parity:**
```bash
cargo fmt && cargo clippy -- -D warnings && cargo test --lib && cargo fmt -- --check
```

## Architecture

```
src/
├── agent/          # Agent loop, context builder, token budget, context compaction
├── auth/           # OAuth (PKCE), token refresh, encrypted token store
├── bus/            # Async message bus (pub/sub)
├── channels/       # Input channels (Telegram, Slack, WhatsApp, etc.)
│   ├── factory.rs  # Channel factory/registry
│   ├── manager.rs  # Channel lifecycle management
│   ├── model_switch.rs # /model command parsing + model registry + persistence
│   ├── persona_switch.rs # /persona command parsing + preset registry + LTM persistence
│   ├── telegram.rs # Telegram bot channel (HTML parse mode + ||spoiler|| support)
│   ├── slack.rs    # Slack outbound channel
│   ├── discord.rs  # Discord Gateway WebSocket + REST (reply + thread create)
│   ├── webhook.rs  # Generic HTTP webhook inbound
│   ├── whatsapp.rs # WhatsApp via whatsmeow-rs bridge (WebSocket)
│   ├── whatsapp_cloud.rs # WhatsApp Cloud API (official webhook + REST)
│   └── serial.rs  # Serial (UART) channel for embedded device messaging (feature: hardware)
├── cli/            # Clap command parsing + command handlers
│   ├── memory.rs   # Memory list/search/set/delete/stats commands
│   ├── tools.rs    # Tool discovery list/info + dynamic status summary
│   └── watch.rs    # URL change monitoring with channel notification
├── config/         # Configuration types and loading
├── cron/           # Persistent cron scheduler service (dispatch timeout + error backoff)
├── deps/           # Dependency manager (install, start, stop, health check)
│   ├── types.rs    # Dependency, DepKind, HealthCheck, HasDependencies
│   ├── registry.rs # JSON registry (installed state tracking)
│   ├── fetcher.rs  # DepFetcher trait + real/mock implementations
│   └── manager.rs  # DepManager lifecycle orchestrator
├── gateway/        # Containerized agent proxy (Docker/Apple)
├── health.rs       # Health server, HealthRegistry, UsageMetrics, get_rss_bytes()
├── heartbeat/      # Periodic background task service
├── memory/         # Workspace memory + long-term memory with pluggable search backends
│   ├── traits.rs         # MemorySearcher trait
│   ├── builtin_searcher.rs # Default substring scorer (always compiled)
│   ├── bm25_searcher.rs  # BM25 keyword scorer (feature: memory-bm25)
│   ├── factory.rs        # create_searcher() factory from config
│   ├── longterm.rs       # Persistent KV store with pluggable searcher
│   └── mod.rs            # Workspace markdown search with pluggable searcher
├── peripherals/    # Hardware peripherals (serial boards, GPIO, I2C, NVS)
│   ├── traits.rs         # Peripheral trait (always compiled)
│   ├── board_profile.rs  # BoardProfile registry — pin ranges, capabilities per board
│   ├── serial.rs         # SerialTransport + SerialPeripheral + GPIO tools (feature: hardware)
│   ├── i2c.rs            # I2C tools — scan, read, write (feature: hardware)
│   ├── nvs.rs            # NVS tools — get, set, delete (feature: hardware)
│   ├── esp32.rs          # ESP32 peripheral wrapper (feature: peripheral-esp32)
│   ├── rpi.rs            # RPi GPIO peripheral + pin validation (feature: peripheral-rpi, Linux)
│   ├── rpi_i2c.rs        # RPi native I2C tools — scan, read, write via rppal (feature: peripheral-rpi, Linux)
│   ├── arduino.rs        # Arduino peripheral wrapper (feature: hardware)
│   └── nucleo.rs         # STM32 Nucleo peripheral wrapper (feature: hardware)
├── providers/      # LLM providers (Claude, OpenAI, Retry, Fallback)
├── runtime/        # Container runtimes (Native, Docker, Apple)
├── routines/       # Event/webhook/cron triggered automations
├── safety/         # Prompt injection detection, secret leak scanning, policy engine, chain alerting
├── security/       # Shell blocklist, path validation, mount policy, secret encryption
│   └── encryption.rs # XChaCha20-Poly1305 + Argon2id secret encryption at rest
├── session/        # Session, message persistence, conversation history
├── tunnel/         # Tunnel providers (Cloudflare, ngrok, Tailscale)
├── skills/         # Markdown-based skill system (OpenClaw-compatible, loader, types)
├── plugins/        # Plugin system (JSON manifest, discovery, registry, binary mode)
├── tools/          # Agent tools (18 tools + MCP + binary plugins + android)
│   ├── android/     # Android device control via ADB (feature-gated: --features android)
│   │   ├── mod.rs      # AndroidTool struct, Tool trait impl, action dispatch
│   │   ├── types.rs    # UIElement, ScreenState, StuckAlert
│   │   ├── adb.rs      # AdbExecutor: async subprocess, retry, device detection
│   │   ├── screen.rs   # XML parser (quick-xml), scoring, dedup, compact JSON
│   │   ├── actions.rs  # Action handlers, text escaping, coordinate sanitization
│   │   └── stuck.rs    # Screen hash, repetition/drift detection, alerts
│   ├── binary_plugin.rs # Binary plugin adapter (JSON-RPC 2.0 stdin/stdout)
│   ├── shell.rs       # Shell execution with runtime isolation
│   ├── filesystem.rs  # Read, write, list, edit files
│   ├── web.rs         # Web search (Brave) and fetch with SSRF protection
│   ├── whatsapp.rs    # WhatsApp Cloud API messaging
│   ├── gsheets.rs     # Google Sheets read/write
│   ├── message.rs     # Proactive channel messaging (reply/thread hints)
│   ├── memory.rs      # Workspace memory get/search
│   ├── longterm_memory.rs # Long-term memory tool (set/get/search/delete/list/categories/pin)
│   ├── cron.rs        # Cron job scheduling
│   ├── spawn.rs       # Background task delegation
│   ├── delegate.rs    # Agent swarm delegation (DelegateTool)
│   ├── plugin.rs      # Plugin tool adapter (PluginTool)
│   ├── approval.rs    # Tool approval gate (ApprovalGate)
│   ├── r8r.rs         # R8r workflow integration
│   ├── reminder.rs    # Persistent reminders (add/complete/snooze/overdue) with cron delivery
│   └── mcp/           # MCP (Model Context Protocol) client tools
│       ├── protocol.rs   # JSON-RPC 2.0 types, content blocks
│       ├── client.rs     # HTTP transport, tools cache
│       └── wrapper.rs    # McpToolWrapper adapts MCP tools to Tool trait
├── utils/          # Utility functions (sanitize, metrics, telemetry, cost)
├── batch.rs        # Batch mode (load prompts from file, format results)
├── error.rs        # Error types (ZeptoError)
├── lib.rs          # Library exports
└── main.rs         # Thin entry point delegating to cli::run()

landing/
└── zeptoclaw/
    ├── index.html        # Static landing page (hero, sections, interactive animations)
    └── mascot-no-bg.png  # README mascot asset used in landing hero
```

## Key Modules

### Runtime (`src/runtime/`)
Selectable container isolation for shell commands:
- `NativeRuntime` - Direct execution (default)
- `DockerRuntime` - Docker container isolation
- `AppleContainerRuntime` - macOS 15+ native containers
- `LandlockRuntime` - Linux kernel LSM (5.13+), pure-Rust, no binary dep (`--features sandbox-landlock`)
- `FirejailRuntime` - Linux namespace + seccomp via `firejail` binary (`--features sandbox-firejail`)
- `BubblewrapRuntime` - OCI-compatible `bwrap` sandbox (`--features sandbox-bubblewrap`)

### Gateway (`src/gateway/`)
Containerized agent proxy for full request isolation:
- Stdin/stdout IPC with containerized agent
- Semaphore-based concurrency limiting (`max_concurrent` config)
- Mount allowlist validation, docker binary verification
- **Auto-installs channel dependencies** (e.g., whatsmeow-bridge for WhatsApp)
- Dependencies installed at gateway startup via DepManager
- Warn-and-continue on dependency failures (non-blocking)

### Providers (`src/providers/`)
LLM provider abstraction via `LLMProvider` trait:
- `ClaudeProvider` - Anthropic Claude API (120s timeout, SSE streaming)
- `OpenAIProvider` - OpenAI Chat Completions API (120s timeout, SSE streaming); supports any OpenAI-compatible endpoint via `api_base` (Ollama, Groq, Together, Fireworks, LM Studio, vLLM)
- `RetryProvider` - Decorator: exponential backoff on 429/5xx with structured `ProviderError` classification
- `FallbackProvider` - Decorator: primary → secondary auto-failover with circuit breaker (Closed/Open/HalfOpen)
- `ProviderError` enum: Auth, RateLimit, Billing, ServerError, InvalidRequest, ModelNotFound, Timeout — enables smart retry/fallback
- Runtime provider assembly in `create_agent()`: resolves configured runtime providers in registry order, builds fallback chain only when `providers.fallback.enabled`, honors `providers.fallback.provider` as preferred first fallback, and optionally wraps the chain with `RetryProvider` (`providers.retry.*`)
- `StreamEvent` enum + `chat_stream()` on LLMProvider trait for token-by-token streaming
- `OutputFormat` enum (Text/Json/JsonSchema) with `to_openai_response_format()` and `to_claude_system_suffix()`

### Auth (`src/auth/`)
OAuth support with PKCE, CSRF state validation, encrypted token persistence, and best-effort refresh before expiry.

### Channels (`src/channels/`)
Message input channels via `Channel` trait:
- `TelegramChannel` - Telegram bot integration
- `SlackChannel` - Slack outbound messaging
- `DiscordChannel` - Discord Gateway WebSocket + REST API messaging (replies + thread creation)
- `WebhookChannel` - Generic HTTP POST inbound with optional Bearer auth
- `WhatsAppChannel` - WhatsApp via whatsmeow-rs bridge (WebSocket JSON protocol)
- `WhatsAppCloudChannel` - WhatsApp Cloud API (webhook inbound + REST outbound, no bridge)
- `SerialChannel` - UART serial messaging (line-delimited JSON, feature: hardware)
- CLI mode via direct agent invocation
- All channels support `deny_by_default` config option for sender allowlists
- Per-chat persona override via `/persona` command (mirrors `/model` pattern)
- `PersonaOverrideStore` + LTM persistence for per-chat personas
- First-chat detection: `FIRST_RUN_PERSONA_PROMPT` constant for prompting persona selection on first message
- `ChannelManager` stores channel handles as `Arc<Mutex<_>>`, so outbound dispatch does not hold the channel map lock across async `send()`
- `ChannelManager` supervision: polling supervisor (15s) detects dead channels via `is_running()`, restarts with 60s cooldown, max 5 restarts, reports to `HealthRegistry`
- All spawned channel tasks set `running = false` on exit to prevent stale `is_running()` flags

### Deps (`src/deps/`)
- `HasDependencies` trait — components declare external dependencies
- `DepKind` enum: Binary (GitHub Releases), DockerImage, NpmPackage, PipPackage
- `DepManager` — install, start, stop, health check lifecycle orchestrator
- `Registry` — JSON file at `~/.zeptoclaw/deps/registry.json` tracks installed state
- `DepFetcher` trait — abstracts network calls for testability

### Tools (`src/tools/`)
18 built-in tools + dynamic MCP tools + composed tools via `Tool` async trait. All filesystem tools require workspace.

**Composed tools** (`src/tools/composed.rs`): Natural language tool composition.
- `CreateToolTool` — agent tool with create/list/delete/run actions
- `ComposedTool` — wraps a `ComposedToolDef`, interpolates `{{param}}` placeholders into action template, returns instructions for the agent to follow
- `ComposedToolStore` — persistence at `~/.zeptoclaw/composed_tools.json`
- Auto-loaded at startup in `create_agent()` as first-class tools

### Utils (`src/utils/`)
- `sanitize.rs` - Tool result sanitization (strip base64, hex, truncate)
- `metrics.rs` - MetricsCollector: per-tool call stats, token tracking, session summary (wired into AgentLoop)
- `telemetry.rs` - Prometheus text exposition + JSON metrics rendering from MetricsCollector
- `cost.rs` - Model pricing tables (8 models), CostTracker with per-provider/model cost accumulation

### Batch (`src/batch.rs`)
- Load prompts from text files or JSONL (one per line, `#` comments skipped)
- `BatchResult` struct with index, prompt, response, error, duration
- `format_results()` renders as plain text or JSONL

### Session (`src/session/`)
- `SessionManager` - Async session storage with file persistence
- `ConversationHistory` - CLI session discovery, listing, fuzzy search by title/key, cleanup

### Agent (`src/agent/`)
- `AgentLoop` - Core message processing loop with tool execution + pre-compaction memory flush
- `ContextBuilder` - System prompt and conversation context builder + memory context injection
- `TokenBudget` - Atomic per-session token budget tracker (lock-free via `AtomicU64`)
- `ContextMonitor` - Token estimation (`words * 1.3 + 4/msg`), threshold-based compaction triggers
- `Compactor` - Summarize (LLM-based) or Truncate strategies for context window management
- `start()` now routes inbound work through `process_inbound_message()` helper and calls `try_queue_or_process()` before processing

### Memory (`src/memory/`)
- `MemorySearcher` trait - Pluggable search/scoring backend (builtin, bm25, embedding, hnsw, tantivy)
- `BuiltinSearcher` - Default substring + term-frequency scorer (always compiled, zero deps)
- `Bm25Searcher` - Okapi BM25 keyword scorer (feature-gated: `memory-bm25`, zero deps)
- `create_searcher()` - Factory maps `MemoryBackend` config to `Arc<dyn MemorySearcher>`
- Workspace memory - Markdown search/read with pluggable searcher injection
- `LongTermMemory` - Persistent key-value store at `~/.zeptoclaw/memory/longterm.json` with pluggable searcher, categories, tags, access tracking; injection guard on `set()` rejects values containing prompt injection patterns
- `decay_score()` on `MemoryEntry` - 30-day half-life decay with importance weighting; pinned entries exempt (always 1.0)
- `build_memory_injection()` - Pinned + query-matched memory injection for system prompt (2000 char budget)
- Pre-compaction memory flush - Silent LLM turn saves important facts before context compaction (10s timeout)

### Health (`src/health.rs`)
- `HealthRegistry` — named component checks with restart count, last error
- `UsageMetrics` — lock-free counters (requests, tool calls, tokens, errors)
- `get_rss_bytes()` — platform RSS (macOS mach + Linux /proc/self/statm)
- `/health` returns version, uptime, memory RSS, usage metrics, component checks
- `/ready` returns boolean readiness (all checks not Down)
- Raw TCP server — no web framework dependency

### Landing (`landing/zeptoclaw/index.html`)
- Hero ambient animation, mascot eye/pupil motion, and magnetic CTA interactions
- Scroll-triggered feature-card reveal and stats count-up animations
- Architecture pipeline flow packets and enhanced terminal typing/thinking feedback
- `prefers-reduced-motion` support for accessibility fallback
- README mascot parity: hero now uses `landing/zeptoclaw/mascot-no-bg.png` (bundled by `landing/deploy.sh`)

### Safety (`src/safety/`)
- `SafetyLayer` - Orchestrator: length check → leak detection → policy check → injection sanitization
- `sanitizer.rs` - Aho-Corasick multi-pattern matcher for 17 prompt injection patterns + 4 regex patterns
- `leak_detector.rs` - 22 regex patterns for API keys/tokens/secrets; Block, Redact, or Warn actions
- `policy.rs` - 7 security policy rules (system file access, crypto keys, SQL, shell injection, encoded exploits)
- `validator.rs` - Input length (100KB max), null byte, whitespace ratio, repetition detection
- `chain_alert.rs` - Tool chain alerting: tracks tool call sequences per session, warns on dangerous patterns (write→execute, execute→fetch, memory→execute)
- Tiered inbound injection scanning in agent loop: webhook channel blocked on injection, allowlisted channels (telegram, discord, etc.) warn-only

### Security (`src/security/`)
- `shell.rs` - Regex-based command blocklist + optional allowlist (`ShellAllowlistMode`: Off/Warn/Strict); includes `.zeptoclaw/config.json` blocklist to prevent LLM-driven config exfiltration
- `path.rs` - Workspace path validation, symlink escape detection
- `mount.rs` - Mount allowlist validation, docker binary verification
- `encryption.rs` - `SecretEncryption`: XChaCha20-Poly1305 AEAD + Argon2id KDF, `ENC[...]` ciphertext format, `resolve_master_key()` for env/file/prompt sources, transparent config decrypt on load

### Tunnel (`src/tunnel/`)
- `TunnelProvider` trait with `start()` / `stop()` lifecycle
- `CloudflareTunnel` - Cloudflare quick tunnels via `cloudflared`
- `NgrokTunnel` - ngrok tunnels via `ngrok` CLI
- `TailscaleTunnel` - Tailscale funnel via `tailscale`
- Auto-detect mode: tries available providers in order

### MCP Client (`src/tools/mcp/`)
- `protocol.rs` - JSON-RPC 2.0 types: McpRequest, McpResponse, McpTool, ContentBlock (Text/Image/Resource)
- `client.rs` - McpClient HTTP transport with initialize/list_tools/call_tool; RwLock tools cache
- `wrapper.rs` - McpToolWrapper implements Tool trait; prefixed tool names (`{server}_{tool}`)

### Routines (`src/routines/`)
- `Routine` - Trigger enum (Cron/Event/Webhook/Manual), RoutineAction enum (Lightweight/FullJob)
- `RoutineStore` - JSON file persistence, cooldown enforcement, CRUD operations
- `RoutineEngine` - Compiled regex cache for event matching, webhook path matching, concurrent execution limits

## Configuration

Config file: `~/.zeptoclaw/config.json`

Environment variables override config:
- `ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY`
- `ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY`
- `ZEPTOCLAW_OAUTH_CLIENT_ID` — OAuth client id (used by `auth login`)
- `ZEPTOCLAW_PROVIDERS_ANTHROPIC_OAUTH_CLIENT_ID` — provider-specific OAuth client id override
- `ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN`
- `ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS` — wall-clock timeout for agent runs (default: 300)
- `ZEPTOCLAW_AGENTS_DEFAULTS_MESSAGE_QUEUE_MODE` — "collect" (default) or "followup"
- `ZEPTOCLAW_PROVIDERS_RETRY_ENABLED` — enable retry wrapper (default: false)
- `ZEPTOCLAW_PROVIDERS_RETRY_MAX_RETRIES` — max retry attempts (default: 3)
- `ZEPTOCLAW_PROVIDERS_RETRY_BASE_DELAY_MS` — base delay in ms (default: 1000)
- `ZEPTOCLAW_PROVIDERS_RETRY_MAX_DELAY_MS` — max delay in ms (default: 30000)
- `ZEPTOCLAW_PROVIDERS_RETRY_BUDGET_MS` — total wall-clock retry budget in ms, 0 = unlimited (default: 45000)
- `ZEPTOCLAW_PROVIDERS_FALLBACK_ENABLED` — enable fallback provider (default: false)
- `ZEPTOCLAW_PROVIDERS_FALLBACK_PROVIDER` — fallback provider name
- `ZEPTOCLAW_AGENTS_DEFAULTS_TOKEN_BUDGET` — per-session token budget (default: 0 = unlimited)
- `ZEPTOCLAW_SAFETY_ENABLED` — enable safety layer (default: true)
- `ZEPTOCLAW_SAFETY_LEAK_DETECTION_ENABLED` — enable secret leak detection (default: true)
- `ZEPTOCLAW_COMPACTION_ENABLED` — enable context compaction (default: false)
- `ZEPTOCLAW_COMPACTION_CONTEXT_LIMIT` — max tokens before compaction (default: 100000)
- `ZEPTOCLAW_COMPACTION_THRESHOLD` — compaction trigger threshold (default: 0.80)
- `ZEPTOCLAW_ROUTINES_ENABLED` — enable routines engine (default: false)
- `ZEPTOCLAW_ROUTINES_CRON_INTERVAL_SECS` — cron tick interval (default: 60)
- `ZEPTOCLAW_ROUTINES_MAX_CONCURRENT` — max concurrent routine executions (default: 3)
- `ZEPTOCLAW_ROUTINES_JITTER_MS` — jitter window in ms for scheduled dispatches (default: 0)
- `ZEPTOCLAW_ROUTINES_ON_MISS` — missed schedule policy: "skip" (default) or "run_once"
- `ZEPTOCLAW_HEARTBEAT_DELIVER_TO` — channel for heartbeat result delivery (default: none)
- `ZEPTOCLAW_MASTER_KEY` — hex-encoded 32-byte master encryption key for secret encryption
- `ZEPTOCLAW_TUNNEL_PROVIDER` — tunnel provider (cloudflare, ngrok, tailscale, auto)
- `ZEPTOCLAW_MEMORY_BACKEND` — memory search backend: builtin (default), bm25, embedding, hnsw, tantivy, none
- `ZEPTOCLAW_MEMORY_EMBEDDING_PROVIDER` — embedding provider name (for embedding backend)
- `ZEPTOCLAW_MEMORY_EMBEDDING_MODEL` — embedding model name (for embedding backend)

### Cargo Features

```bash
# Default build (builtin memory searcher only)
cargo build --release

# With BM25 keyword scoring
cargo build --release --features memory-bm25

# Future features (not yet implemented)
# cargo build --release --features memory-embedding
# cargo build --release --features memory-hnsw
# cargo build --release --features memory-tantivy
```

### Cargo Features

- `android` — Enable Android device control tool (adds `quick-xml` dependency)
- `peripheral-esp32` — Enable ESP32 peripheral with I2C + NVS tools (implies `hardware`)
- `peripheral-rpi` — Enable Raspberry Pi GPIO + native I2C tools via rppal (Linux only)
- `sandbox-landlock` — Enable Landlock LSM runtime (Linux only, adds `landlock` crate)
- `sandbox-firejail` — Enable Firejail runtime (Linux only, requires `firejail` binary)
- `sandbox-bubblewrap` — Enable Bubblewrap runtime (Linux only, requires `bwrap` binary)

```bash
cargo build --release --features android

# Linux sandbox runtimes
cargo build --release --features sandbox-landlock
cargo build --release --features sandbox-firejail
cargo build --release --features sandbox-bubblewrap
cargo build --release --features sandbox-landlock,sandbox-firejail,sandbox-bubblewrap
```

### Compile-time Configuration

Default models can be set at compile time using environment variables:
- `ZEPTOCLAW_DEFAULT_MODEL` - Default model for agent (default: claude-sonnet-4-5-20250929)
- `ZEPTOCLAW_CLAUDE_DEFAULT_MODEL` - Default Claude model (default: claude-sonnet-4-5-20250929)
- `ZEPTOCLAW_OPENAI_DEFAULT_MODEL` - Default OpenAI model (default: gpt-5.1)

Example:
```bash
export ZEPTOCLAW_DEFAULT_MODEL=gpt-5.1
cargo build --release
```

## Design Patterns

- **Async-first**: All I/O uses Tokio async runtime
- **Trait-based abstraction**: `LLMProvider`, `Channel`, `Tool`, `ContainerRuntime`
- **Arc for shared state**: `Arc<dyn LLMProvider>`, `Arc<dyn ContainerRuntime>`
- **Parallel tool execution**: `futures::future::join_all` for concurrent tool calls
- **Tool result sanitization**: Strip base64 URIs, hex blobs, truncate to 50KB before LLM
- **Agent-level timeout**: Wall-clock timeout wrapping entire agent runs (default 300s)
- **Message queue modes**: Collect (concatenate) or Followup (replay) for busy sessions
- **Per-session mutex map**: Prevents concurrent message race conditions
- **Semaphore concurrency**: Container gateway limits concurrent requests
- **spawn_blocking**: Wraps sync I/O (memory, filesystem) in async context
- **Conditional compilation**: `#[cfg(target_os = "macos")]` for Apple-specific code

## Testing

```bash
# Unit tests (2430 tests)
cargo test --lib

# Main binary tests (91 tests)
cargo test --bin zeptoclaw

# CLI smoke tests (23 tests)
cargo test --test cli_smoke

# End-to-end tests (13 tests)
cargo test --test e2e

# Integration tests (70 tests)
cargo test --test integration

# All tests (~2,748 total including doc tests)
cargo test

# Specific test
cargo test test_name

# With output
cargo test -- --nocapture
```

## Benchmarks

Verified on Apple Silicon (release build):
- Binary size: ~4MB
- Startup time: ~50ms
- Memory (RSS): ~6MB

## Common Tasks

### Add a new LLM provider
1. Create `src/providers/newprovider.rs`
2. Implement `LLMProvider` trait
3. Export from `src/providers/mod.rs`
4. Wire up provider resolution in `src/cli/common.rs` (`create_agent*`)

### Add a new tool
1. Create tool in `src/tools/`
2. Implement `Tool` trait with `async fn execute()`
3. Register in `src/tools/mod.rs` and `src/lib.rs`
4. Register in agent setup in `src/cli/common.rs`

### Add a new channel
1. Create `src/channels/newchannel.rs`
2. Implement `Channel` trait
3. Export from `src/channels/mod.rs`
4. Add to channel factory wiring used by `src/cli/gateway.rs`

### Add a new skill
1. Create `~/.zeptoclaw/skills/<name>/SKILL.md`
2. Add YAML frontmatter (name, description, metadata)
3. Add markdown instructions for the agent
4. Or use: `zeptoclaw skills create <name>`

Skills are OpenClaw-compatible — the loader reads `metadata.zeptoclaw`, `metadata.openclaw`, or raw metadata objects (in that priority order). Supported extensions: `os` platform filter, `requires.anyBins` (alias `any_bins`).

**Core skills** (bundled in this repo — `skills/`): `github`, `skill-creator`
- Only skills essential to ZeptoClaw's own dev workflow belong here.

**Community skills** (third-party integrations, platform-specific, utilities):
- Maintained at: https://github.com/qhkm/zeptoclaw-skills
- Discoverable via: `zeptoclaw skills search <query>`
- Installable via: `zeptoclaw skills install --github qhkm/zeptoclaw-skills`
- To contribute a skill, open a PR to that repo instead of this one.

## Dependencies

Key crates:
- `tokio` - Async runtime
- `reqwest` - HTTP client (with 120s timeout)
- `serde` / `serde_json` - Serialization
- `async-trait` - Async trait support
- `tracing` - Structured logging
- `clap` - CLI argument parsing
- `scraper` - HTML parsing for web_fetch
- `aho-corasick` - Multi-pattern string matching for safety layer
- `quick-xml` - XML parsing for Android uiautomator dumps (optional, `android` feature)
