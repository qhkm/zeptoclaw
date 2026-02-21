# AGENTS.md

Project-level guidance for coding agents working in this repository.

## Project Snapshot

- Language: Rust (edition 2021)
- Core binary: `zeptoclaw` (`src/main.rs` thin entrypoint; CLI handlers in `src/cli/`)
- Extra binary: `benchmark` (`src/bin/benchmark.rs`)
- Benchmarks: `benches/message_bus.rs`
- Integration tests: `tests/integration.rs`
- Codebase: ~43,000+ lines of Rust
- Channels: 5 (Telegram, Slack, Discord, Webhook, WhatsApp)
- Skills: OpenClaw-compatible (reads `metadata.zeptoclaw` > `metadata.openclaw` > raw)
- Plugins: Command-mode (shell template) + Binary-mode (JSON-RPC 2.0 stdin/stdout)
- Runtime provider resolution: builds chain in registry order only when `providers.fallback.enabled`; honors `providers.fallback.provider`; can wrap chain with `RetryProvider` via `providers.retry.*`
- Channel dispatch: avoids holding the channels map `RwLock` across async `send()` awaits
- Telegram outbound formatting: sends HTML parse mode with `||spoiler||` → `<tg-spoiler>` conversion
- Discord outbound delivery: supports reply references and thread-create metadata (`discord_thread_*`) in `OutboundMessage`
- Cron scheduling hardening: dispatch timeout + exponential error backoff + one-shot delete-after-run only on success
- Tests: 1791 lib + 59 main + 23 cli_smoke + 13 e2e + 68 integration + 141 doc (116 passed, 25 ignored)

## Task Tracking Protocol

**Every session MUST track work via GitHub Issues.**

1. **Start of session** — Run `gh issue list --repo qhkm/zeptoclaw --state open --limit 20` and present open issues
2. **New work** — If no issue exists for the requested work, create one with `gh issue create` before writing code. Use labels: type (`bug`/`feat`/`rfc`/`chore`/`docs`), area (`area:tools`/`area:channels`/etc.), priority (`P1`/`P2`/`P3`)
3. **End of work** — Close the issue: `Closes #N` in PR body, or `gh issue close N` for direct commits

Skip issue creation only for trivial changes (typo fixes, one-line tweaks).

## Post-Implementation Checklist

**After completing ANY feature, you MUST:**

1. **Close the GitHub issue** — `Closes #N` in PR or `gh issue close N`
2. **`CLAUDE.md`** — update architecture tree, test counts, module descriptions, new CLI flags
3. **`AGENTS.md`** (this file) — update project snapshot above

If you skip this, the next agent starts with stale context and wastes time.

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
4. Register in `create_agent()` in `src/cli/common.rs`

### Adding a provider wrapper
1. Create `src/providers/<name>.rs`
2. Implement `LLMProvider` trait (must impl both `chat()` and `chat_stream()`)
3. Add `pub mod <name>;` + re-export in `src/providers/mod.rs`
4. Wire in `src/cli/common.rs` provider resolution

### Key code patterns
- `ProviderRef` wrapper in `delegate.rs` — converts `Arc<dyn LLMProvider>` to `Box<dyn LLMProvider>`
- Builder pattern for provider wrappers — `RetryProvider::new(inner).with_max_retries(5)`
- Interior mutability via `Mutex<HashMap>` — used in MetricsCollector, OpenAIProvider
- Atomic counters via `AtomicU64` — used in TokenBudget for lock-free token tracking
- Recursion blocking — check `ctx.channel` to prevent delegate/spawn infinite loops
