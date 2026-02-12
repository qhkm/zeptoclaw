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

# Run gateway (Telegram bot)
./target/release/zeptoclaw gateway

# Onboard (interactive setup)
./target/release/zeptoclaw onboard
```

## Architecture

```
src/
├── agent/          # Agent loop and context management
├── bus/            # Async message bus (channels communication)
├── channels/       # Input channels (Telegram, CLI)
├── config/         # Configuration types and loading
├── providers/      # LLM providers (Claude, OpenAI)
├── runtime/        # Container runtimes (Native, Docker, Apple)
├── security/       # Security policies
├── session/        # Session and message persistence
├── skills/         # Agent skills/capabilities
├── tools/          # Agent tools (Shell, Filesystem)
├── utils/          # Utility functions
├── error.rs        # Error types
├── lib.rs          # Library exports
└── main.rs         # CLI entry point
```

## Key Modules

### Runtime (`src/runtime/`)
Selectable container isolation for shell commands:
- `NativeRuntime` - Direct execution (default)
- `DockerRuntime` - Docker container isolation
- `AppleContainerRuntime` - macOS 15+ native containers

### Providers (`src/providers/`)
LLM provider abstraction via `LLMProvider` trait:
- `ClaudeProvider` - Anthropic Claude API
- `OpenAIProvider` - OpenAI Chat Completions API

### Channels (`src/channels/`)
Message input channels via `Channel` trait:
- `TelegramChannel` - Telegram bot integration
- CLI mode via direct agent invocation

### Tools (`src/tools/`)
Agent tools via `Tool` async trait:
- `ShellTool` - Execute shell commands (with runtime isolation)
- Filesystem tools - Read, write, list files

## Configuration

Config file: `~/.config/zeptoclaw/config.json`

Environment variables override config:
- `ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY`
- `ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY`
- `ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN`

## Design Patterns

- **Async-first**: All I/O uses Tokio async runtime
- **Trait-based abstraction**: `LLMProvider`, `Channel`, `Tool`, `ContainerRuntime`
- **Builder pattern**: Runtime configuration (e.g., `DockerRuntime::new().with_memory_limit()`)
- **Arc for shared state**: `Arc<dyn ContainerRuntime>` for runtime sharing
- **Conditional compilation**: `#[cfg(target_os = "macos")]` for Apple-specific code

## Testing

```bash
# Unit tests
cargo test --lib

# Integration tests
cargo test --test integration

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
3. Register in tool registry

### Add a new channel
1. Create `src/channels/newchannel.rs`
2. Implement `Channel` trait
3. Export from `src/channels/mod.rs`
4. Add to gateway mode in `main.rs`

## Dependencies

Key crates:
- `tokio` - Async runtime
- `reqwest` - HTTP client
- `serde` / `serde_json` - Serialization
- `async-trait` - Async trait support
- `tracing` - Structured logging
- `clap` - CLI argument parsing
