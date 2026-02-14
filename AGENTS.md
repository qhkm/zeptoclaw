# AGENTS.md

Project-level guidance for coding agents working in this repository.

## Scope

These instructions apply to the entire `rust/` project.

## Post-Implementation Checklist

**After completing ANY feature, you MUST update these files:**

1. **`docs/plans/TODO.md`** — check off completed items, add new backlog items if discovered
2. **`CLAUDE.md`** — update architecture tree, test counts, module descriptions, new CLI flags
3. **`AGENTS.md`** (this file) — update "Current State" section below

If you skip this, the next agent starts with stale context and wastes time.

## Project Snapshot

- Language: Rust (edition 2021)
- Core binary: `zeptoclaw` (`src/main.rs`, ~2200 lines)
- Extra binary: `benchmark` (`src/bin/benchmark.rs`)
- Benchmarks: `benches/message_bus.rs`
- Integration tests: `tests/integration.rs`
- Codebase: ~38,000+ lines of Rust
- Tests: 953 lib + 68 integration + 98 doc = 1,119 total

## Current State (2026-02-14)

### Recently Completed
- Streaming responses — SSE for both Claude and OpenAI providers, `--stream` CLI flag
- Agent swarm — DelegateTool with recursion blocking, ProviderRef wrapper
- RetryProvider — exponential backoff on 429/5xx, wired into provider stack
- FallbackProvider — primary → secondary auto-failover, wired with multi-provider resolution
- MetricsCollector — wired into AgentLoop, tracks per-tool duration/success + token usage
- Config fields for retry (`providers.retry.*`) and fallback (`providers.fallback.*`) with env overrides
- Provider stack: base provider → optional FallbackProvider → optional RetryProvider
- ConversationHistory (`src/session/history.rs`) — CLI session discovery, listing, fuzzy search, cleanup (12 tests)
- TokenBudget (`src/agent/budget.rs`) — atomic per-session token budget with lock-free counters (18 tests)
- OutputFormat (`src/providers/structured.rs`) — Text/Json/JsonSchema enum with OpenAI + Claude helpers (19 tests)
- LongTermMemory (`src/memory/longterm.rs`) — persistent key-value store with categories, tags, access tracking (19 tests)
- ConversationHistory CLI — `history list`, `history show`, `history cleanup` commands wired in main.rs
- TokenBudget wired — `token_budget` config field, env override, budget check in agent loop before LLM calls
- OutputFormat wired — `output_format` on `ChatOptions`, OpenAI `response_format`, Claude system suffix
- LongTermMemoryTool (`src/tools/longterm_memory.rs`) — agent tool for set/get/search/delete/list/categories (22 tests)
- WebhookChannel (`src/channels/webhook.rs`) — HTTP POST inbound with auth, wired in factory (28 tests)
- DiscordChannel (`src/channels/discord.rs`) — Gateway WebSocket + REST, wired in factory (27 tests)
- Tool approval (`src/tools/approval.rs`) — ApprovalGate with configurable policies (24 tests)
- Agent templates (`src/config/templates.rs`) — 4 built-in + JSON file loading (21 tests)
- Plugin system (`src/plugins/`) — JSON manifest plugins, discovery, validation, registry (70+ tests)
- Integration expansion (`tests/integration.rs`) — fallback provider, cron scheduling dispatch, heartbeat trigger behavior, and skills availability filtering

### Deep Wiring (2026-02-14)
- Tool approval wired — `ApprovalConfig` on `Config`, `ApprovalGate` checked before each tool execution in agent loop (both streaming and non-streaming paths)
- Agent templates wired — `template list`, `template show` CLI commands, `agent --template <name>` flag applies system prompt + model/tokens/temperature overrides
- Plugin system wired — `PluginConfig` on `Config`, `PluginTool` adapter (`src/tools/plugin.rs`, 10 tests), plugin discovery + registration in `create_agent()`
- Webhook channel wired — `WebhookConfig` on `ChannelsConfig`, registered in factory with bind/port/auth/allowlist
- DiscordChannel — fully wired in factory, functional when `discord.enabled=true` with token

### Low Priority Features (2026-02-14)
- Telemetry export (`src/utils/telemetry.rs`) — Prometheus text exposition + JSON renderers, `TelemetryConfig` on `Config` (13 tests)
- Cost tracking (`src/utils/cost.rs`) — model pricing tables for 8 models, `CostTracker` with per-provider/model accumulation, `CostConfig` on `Config` (18 tests)
- Batch mode (`src/batch.rs`) — load prompts from text/jsonl files, `BatchResult` + formatters, `batch` CLI command with --input/--output/--format/--stop-on-error/--stream/--template (15+ tests)

### Roadmap
See `docs/plans/TODO.md` for the full checklist.

## File Ownership Rules (for parallel agents)

To avoid merge conflicts when multiple agents work simultaneously:

| Zone | Files | Rule |
|------|-------|------|
| **Shared (wire last)** | `src/main.rs`, `src/config/types.rs`, `*/mod.rs` | Only ONE agent touches these. Wiring done after all parallel work finishes. |
| **Provider** | `src/providers/<name>.rs` | One agent per provider file. |
| **Tool** | `src/tools/<name>.rs` | One agent per tool file. |
| **Utils** | `src/utils/<name>.rs` | One agent per util file. |
| **New files** | Any new `*.rs` | Safe — no conflicts if it's a new file. |

## Required Quality Gates

Before finishing any non-trivial change, run:

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```

If benchmark-related code is changed, also run:

```bash
cargo bench --bench message_bus --no-run
```

## Coding Rules

- Keep changes minimal and focused.
- Prefer small, composable functions over large blocks.
- Do not add `unwrap()`/`expect()` in production paths unless failure is truly unrecoverable.
- Preserve existing module boundaries and public APIs unless explicitly requested.
- Keep comments short and only where intent is non-obvious.

## Runtime and Provider Notes

- Runtime isolation features must remain opt-in and degrade safely to native runtime.
- Provider wiring should remain consistent across config, onboarding, status output, and runtime behavior.
- Do not hardcode a single provider path when multiple providers are supported.

## Documentation Rules

- Keep README/docs claims aligned with executable behavior.
- Do not add performance numbers unless they are reproducible with repository commands.
- If adding new commands or workflows, include a runnable example.

## Change Hygiene

- Do not revert unrelated local changes.
- If you detect unexpected file modifications during work, pause and ask before proceeding.
- Include file/line references when reporting review findings.

## Common Patterns

### Adding a config field
1. Add field + doc comment to struct in `src/config/types.rs`
2. Set default in `Default` impl
3. Add env override in `src/config/mod.rs` if needed
4. Add field name to `KNOWN_TOP_LEVEL` in `src/config/validate.rs`

### Adding a new tool
1. Create `src/tools/<name>.rs`
2. Implement `Tool` trait (`name()`, `description()`, `parameters()`, `execute()`)
3. Add `pub mod <name>;` in `src/tools/mod.rs`
4. Register in `create_agent()` in `src/main.rs`

### Adding a provider wrapper
1. Create `src/providers/<name>.rs`
2. Implement `LLMProvider` trait (must impl both `chat()` and `chat_stream()`)
3. Add `pub mod <name>;` + re-export in `src/providers/mod.rs`
4. Wire in `main.rs` provider resolution

### Key code patterns
- `ProviderRef` wrapper in `delegate.rs` — converts `Arc<dyn LLMProvider>` to `Box<dyn LLMProvider>`
- Builder pattern for provider wrappers — `RetryProvider::new(inner).with_max_retries(5)`
- Interior mutability via `Mutex<HashMap>` — used in MetricsCollector, OpenAIProvider
- Atomic counters via `AtomicU64` — used in TokenBudget for lock-free token tracking
- Recursion blocking — check `ctx.channel` to prevent delegate/spawn infinite loops
