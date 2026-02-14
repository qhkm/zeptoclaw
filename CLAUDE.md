# ZeptoClaw

Rust-based AI agent framework with container isolation. The smallest, fastest, safest member of the Claw family.

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

# Batch mode (process multiple prompts from file)
./target/release/zeptoclaw batch --input prompts.txt
./target/release/zeptoclaw batch --input prompts.jsonl --output results.jsonl --format jsonl
./target/release/zeptoclaw batch --input prompts.txt --template coder --stop-on-error

# Onboard (interactive setup)
./target/release/zeptoclaw onboard
```

## Architecture

```
src/
├── agent/          # Agent loop, context builder, token budget
├── bus/            # Async message bus (pub/sub)
├── channels/       # Input channels (Telegram, Slack, CLI)
│   ├── factory.rs  # Channel factory/registry
│   ├── manager.rs  # Channel lifecycle management
│   ├── telegram.rs # Telegram bot channel
│   ├── slack.rs    # Slack outbound channel
│   ├── discord.rs  # Discord Gateway WebSocket + REST
│   └── webhook.rs  # Generic HTTP webhook inbound
├── config/         # Configuration types and loading
├── cron/           # Persistent cron scheduler service
├── gateway/        # Containerized agent proxy (Docker/Apple)
├── heartbeat/      # Periodic background task service
├── memory/         # Workspace memory (markdown search) + long-term memory
├── providers/      # LLM providers (Claude, OpenAI, Retry, Fallback)
├── runtime/        # Container runtimes (Native, Docker, Apple)
├── security/       # Shell blocklist, path validation, mount policy
├── session/        # Session, message persistence, conversation history
├── skills/         # Markdown-based skill system (loader, types)
├── plugins/        # Plugin system (JSON manifest, discovery, registry)
├── tools/          # Agent tools (17 tools)
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
│   └── r8r.rs         # R8r workflow integration
├── utils/          # Utility functions (sanitize, metrics, telemetry, cost)
├── batch.rs        # Batch mode (load prompts from file, format results)
├── error.rs        # Error types (ZeptoError)
├── lib.rs          # Library exports
└── main.rs         # CLI entry point (~2200 lines)
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
- `RetryProvider` - Decorator: exponential backoff on 429/5xx
- `FallbackProvider` - Decorator: primary → secondary auto-failover
- Provider stack in `create_agent()`: base → optional FallbackProvider → optional RetryProvider
- `StreamEvent` enum + `chat_stream()` on LLMProvider trait for token-by-token streaming
- `OutputFormat` enum (Text/Json/JsonSchema) with `to_openai_response_format()` and `to_claude_system_suffix()`

### Channels (`src/channels/`)
Message input channels via `Channel` trait:
- `TelegramChannel` - Telegram bot integration
- `SlackChannel` - Slack outbound messaging
- `DiscordChannel` - Discord Gateway WebSocket + REST API messaging
- `WebhookChannel` - Generic HTTP POST inbound with optional Bearer auth
- CLI mode via direct agent invocation

### Tools (`src/tools/`)
15 tools via `Tool` async trait. All filesystem tools require workspace.

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

### Memory (`src/memory/`)
- Workspace memory - Markdown search/read with chunked scoring
- `LongTermMemory` - Persistent key-value store at `~/.zeptoclaw/memory/longterm.json` with categories, tags, access tracking

### Security (`src/security/`)
- `shell.rs` - Regex-based command blocklist
- `path.rs` - Workspace path validation, symlink escape detection
- `mount.rs` - Mount allowlist validation, docker binary verification

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

## Design Decisions

### Why No Vector Embeddings / RAG

ZeptoClaw uses **agentic search** instead of RAG + vector databases for memory and context retrieval. Early versions of Claude Code used RAG with a local vector DB but found that agentic search works better — it's simpler and avoids issues around security, privacy, staleness, and reliability.

**What we use instead:**
- Agent tools (`shell`, `filesystem`, `memory`) let the agent decide what to search for
- `longterm_memory` tool provides persistent key-value storage with keyword/tag search
- Workspace memory uses markdown search with chunked scoring

**Why this is better for ZeptoClaw:**
- Zero binary bloat (no embedding model or vector DB dependency)
- Always reads current files — no stale index problems
- No data sent to embedding APIs — fully local and private
- Agent adapts search strategy per query instead of relying on cosine similarity
- Keeps the 5MB binary small and the architecture simple

## Testing

```bash
# Unit tests (953 tests)
cargo test --lib

# Integration tests (68 tests)
cargo test --test integration

# All tests (1,119 total including doc tests)
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
4. Wire up in `main.rs` create_agent()

### Add a new tool
1. Create tool in `src/tools/`
2. Implement `Tool` trait with `async fn execute()`
3. Register in `src/tools/mod.rs` and `src/lib.rs`
4. Register in agent setup in `main.rs`

### Add a new channel
1. Create `src/channels/newchannel.rs`
2. Implement `Channel` trait
3. Export from `src/channels/mod.rs`
4. Add to gateway mode in `main.rs`

### Add a new skill
1. Create `~/.zeptoclaw/skills/<name>/SKILL.md`
2. Add YAML frontmatter (name, description, metadata)
3. Add markdown instructions for the agent
4. Or use: `zeptoclaw skills create <name>`

## Dependencies

Key crates:
- `tokio` - Async runtime
- `reqwest` - HTTP client (with 120s timeout)
- `serde` / `serde_json` - Serialization
- `async-trait` - Async trait support
- `tracing` - Structured logging
- `clap` - CLI argument parsing
- `scraper` - HTML parsing for web_fetch
