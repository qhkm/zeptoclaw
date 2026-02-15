# ZeptoClaw

A complete AI agent runtime in 4MB. The best of OpenClaw's integrations, NanoClaw's security, and PicoClaw's minimalism — without their tradeoffs.

## Quick Reference

```bash
# Build
cargo build --release

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt

# Version
./target/release/zeptoclaw --version

# Run agent
./target/release/zeptoclaw agent -m "Hello"

# Run agent with streaming
./target/release/zeptoclaw agent -m "Hello" --stream

# Run gateway (Telegram bot)
./target/release/zeptoclaw gateway

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

# Channel management
./target/release/zeptoclaw channel list
./target/release/zeptoclaw channel setup whatsapp
./target/release/zeptoclaw channel test whatsapp

# Onboard (interactive setup)
./target/release/zeptoclaw onboard
```

## Architecture

```
src/
├── agent/          # Agent loop, context builder, token budget, context compaction
├── bus/            # Async message bus (pub/sub)
├── channels/       # Input channels (Telegram, Slack, WhatsApp, etc.)
│   ├── factory.rs  # Channel factory/registry
│   ├── manager.rs  # Channel lifecycle management
│   ├── telegram.rs # Telegram bot channel
│   ├── slack.rs    # Slack outbound channel
│   ├── discord.rs  # Discord Gateway WebSocket + REST
│   ├── webhook.rs  # Generic HTTP webhook inbound
│   └── whatsapp.rs # WhatsApp via whatsmeow-rs bridge (WebSocket)
├── cli/            # Clap command parsing + command handlers
├── config/         # Configuration types and loading
├── cron/           # Persistent cron scheduler service
├── deps/           # Dependency manager (install, start, stop, health check)
│   ├── types.rs    # Dependency, DepKind, HealthCheck, HasDependencies
│   ├── registry.rs # JSON registry (installed state tracking)
│   ├── fetcher.rs  # DepFetcher trait + real/mock implementations
│   └── manager.rs  # DepManager lifecycle orchestrator
├── gateway/        # Containerized agent proxy (Docker/Apple)
├── heartbeat/      # Periodic background task service
├── memory/         # Workspace memory (markdown search) + long-term memory
├── providers/      # LLM providers (Claude, OpenAI, Retry, Fallback)
├── runtime/        # Container runtimes (Native, Docker, Apple)
├── routines/       # Event/webhook/cron triggered automations
├── safety/         # Prompt injection detection, secret leak scanning, policy engine
├── security/       # Shell blocklist, path validation, mount policy
├── session/        # Session, message persistence, conversation history
├── skills/         # Markdown-based skill system (OpenClaw-compatible, loader, types)
├── plugins/        # Plugin system (JSON manifest, discovery, registry, binary mode)
├── tools/          # Agent tools (18 tools + MCP + binary plugins)
│   ├── binary_plugin.rs # Binary plugin adapter (JSON-RPC 2.0 stdin/stdout)
│   ├── shell.rs       # Shell execution with runtime isolation
│   ├── filesystem.rs  # Read, write, list, edit files
│   ├── web.rs         # Web search (Brave) and fetch with SSRF protection
│   ├── whatsapp.rs    # WhatsApp Cloud API messaging
│   ├── gsheets.rs     # Google Sheets read/write
│   ├── message.rs     # Proactive channel messaging
│   ├── memory.rs      # Workspace memory get/search
│   ├── longterm_memory.rs # Long-term memory tool (set/get/search/delete/list/categories)
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

### Gateway (`src/gateway/`)
Containerized agent proxy for full request isolation:
- Stdin/stdout IPC with containerized agent
- Semaphore-based concurrency limiting (`max_concurrent` config)
- Mount allowlist validation, docker binary verification

### Providers (`src/providers/`)
LLM provider abstraction via `LLMProvider` trait:
- `ClaudeProvider` - Anthropic Claude API (120s timeout, SSE streaming)
- `OpenAIProvider` - OpenAI Chat Completions API (120s timeout, SSE streaming)
- `RetryProvider` - Decorator: exponential backoff on 429/5xx with structured `ProviderError` classification
- `FallbackProvider` - Decorator: primary → secondary auto-failover with circuit breaker (Closed/Open/HalfOpen)
- `ProviderError` enum: Auth, RateLimit, Billing, ServerError, InvalidRequest, ModelNotFound, Timeout — enables smart retry/fallback
- Provider stack in `create_agent()`: base → optional FallbackProvider → optional RetryProvider
- `StreamEvent` enum + `chat_stream()` on LLMProvider trait for token-by-token streaming
- `OutputFormat` enum (Text/Json/JsonSchema) with `to_openai_response_format()` and `to_claude_system_suffix()`

### Channels (`src/channels/`)
Message input channels via `Channel` trait:
- `TelegramChannel` - Telegram bot integration
- `SlackChannel` - Slack outbound messaging
- `DiscordChannel` - Discord Gateway WebSocket + REST API messaging
- `WebhookChannel` - Generic HTTP POST inbound with optional Bearer auth
- `WhatsAppChannel` - WhatsApp via whatsmeow-rs bridge (WebSocket JSON protocol)
- CLI mode via direct agent invocation

### Deps (`src/deps/`)
- `HasDependencies` trait — components declare external dependencies
- `DepKind` enum: Binary (GitHub Releases), DockerImage, NpmPackage, PipPackage
- `DepManager` — install, start, stop, health check lifecycle orchestrator
- `Registry` — JSON file at `~/.zeptoclaw/deps/registry.json` tracks installed state
- `DepFetcher` trait — abstracts network calls for testability

### Tools (`src/tools/`)
16 built-in tools + dynamic MCP tools via `Tool` async trait. All filesystem tools require workspace.

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
- `AgentLoop` - Core message processing loop with tool execution
- `ContextBuilder` - System prompt and conversation context builder
- `TokenBudget` - Atomic per-session token budget tracker (lock-free via `AtomicU64`)
- `ContextMonitor` - Token estimation (`words * 1.3 + 4/msg`), threshold-based compaction triggers
- `Compactor` - Summarize (LLM-based) or Truncate strategies for context window management

### Memory (`src/memory/`)
- Workspace memory - Markdown search/read with chunked scoring
- `LongTermMemory` - Persistent key-value store at `~/.zeptoclaw/memory/longterm.json` with categories, tags, access tracking

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

### Security (`src/security/`)
- `shell.rs` - Regex-based command blocklist
- `path.rs` - Workspace path validation, symlink escape detection
- `mount.rs` - Mount allowlist validation, docker binary verification

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
- `ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN`
- `ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS` — wall-clock timeout for agent runs (default: 300)
- `ZEPTOCLAW_AGENTS_DEFAULTS_MESSAGE_QUEUE_MODE` — "collect" (default) or "followup"
- `ZEPTOCLAW_PROVIDERS_RETRY_ENABLED` — enable retry wrapper (default: false)
- `ZEPTOCLAW_PROVIDERS_RETRY_MAX_RETRIES` — max retry attempts (default: 3)
- `ZEPTOCLAW_PROVIDERS_RETRY_BASE_DELAY_MS` — base delay in ms (default: 1000)
- `ZEPTOCLAW_PROVIDERS_RETRY_MAX_DELAY_MS` — max delay in ms (default: 30000)
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
# Unit tests (1314 tests)
cargo test --lib

# Integration tests (68 tests)
cargo test --test integration

# All tests (~1,314 total including doc tests)
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
