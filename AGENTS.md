# AGENTS.md

Project-level guidance for coding agents working in this repository.

## Project Snapshot

- Language: Rust (edition 2021)
- Core binary: `zeptoclaw` (`src/main.rs` thin entrypoint; CLI handlers in `src/cli/`)
- Extra binary: `benchmark` (`src/bin/benchmark.rs`)
- Benchmarks: `benches/message_bus.rs`
- Integration tests: `tests/integration.rs`
- Agent coding benchmark fixture: `test-coding/` with intentionally buggy Python code and stdlib verification tests
- Pristine agent comparison fixture: `test-coding-pristine/` preserves the original failing state for repeatable head-to-head runs
- Codebase: ~106,000+ lines of Rust
- Channels: 10 (Telegram, Slack, Discord, WhatsApp, WhatsApp Web, WhatsApp Cloud, Lark, Email, Webhook, Serial)
- Runtimes: 6 (Native, Docker, Apple Container, Landlock, Firejail, Bubblewrap)
- Peripherals: 4 boards (ESP32, RPi, Arduino, Nucleo) with GPIO, I2C, NVS, Serial
- Skills: OpenClaw-compatible (reads `metadata.zeptoclaw` > `metadata.openclaw` > raw)
- Plugins: Command-mode (shell template) + Binary-mode (JSON-RPC 2.0 stdin/stdout)
- Library facade: `ZeptoAgent::builder()` for embedding as a crate (Tauri, GUI apps)
- Runtime provider resolution: builds chain in registry order only when `providers.fallback.enabled`; honors `providers.fallback.provider`; can wrap chain with `RetryProvider` via `providers.retry.*`
- Provider introspection CLI: `zeptoclaw provider status` prints resolved providers, wrapper config (retry/fallback), and quota usage snapshot
- Channel dispatch: avoids holding the channels map `RwLock` across async `send()` awaits
- Channel supervisor: polling (15s) detects dead channels, restarts with 60s cooldown, max 5 restarts
- Channel panic isolation: Slack/Discord/Webhook/WhatsApp/WhatsApp Web/WhatsApp Cloud/Lark/Email/MQTT/Serial spawned tasks are wrapped with `catch_unwind` and panic logging
- Webhook auth hardening: generic webhook supports optional HMAC-SHA256 body signatures plus fixed server-side sender/chat identity by default (`trust_payload_identity` is an explicit legacy escape hatch); WhatsApp Cloud verifies `X-Hub-Signature-256` when `app_secret` is configured
- Telegram allowlist hardening: numeric user IDs are the safe default for new setups; legacy username matching remains available only through `channels.telegram.allow_usernames` for compatibility and emits warnings when non-numeric allowlist entries are present
- Email allowlist limitation surfaced: `channels.email.allowed_senders` matches the parsed `From` header only and now emits config/runtime warnings so authenticated-mail enforcement is pushed upstream
- Telegram outbound formatting: sends HTML parse mode with `||spoiler||` → `<tg-spoiler>` conversion
- Discord outbound delivery: supports reply references and thread-create metadata (`discord_thread_*`) in `OutboundMessage`
- Cron scheduling hardening: dispatch timeout + exponential error backoff + one-shot delete-after-run only on success
- Model switching: Telegram `/model` supports per-chat overrides (in-memory + long-term)
- Persona switching: `/persona` command with presets and custom text, LTM persistence per chat
- CLI interactive mode: TTY-gated local slash commands with rustyline tab completion when available, persisted REPL history, inline tool approval prompts, session-scoped `/trust` override for local use, `/model` and `/persona` overrides, `/tools`, `/template`, and `/clear`
- Memory injection: per-message query-matched injection via shared LTM on `AgentLoop` (startup static injection removed)
- Tool execution convergence: agent loop and MCP server both route through `kernel::execute_tool()` (shared safety scan + taint checks + single metrics recording)
- Tool composition: natural language tool creation with `{{param}}` template interpolation
- Filesystem hardening: filesystem write/edit tools now create parent directories one component at a time inside the workspace and use secure no-follow writes; mount validation rejects Unix regular-file mounts with multiple hard links in both blocked-path and allowlist flows
- Safer default execution posture: fresh configs now start in `agent_mode = "assistant"` with approvals enabled under the `require_for_dangerous` policy
- Gateway startup guard: degrade after N crashes to prevent crash loops
- Loop guard: SHA256 tool-call repetition detection with warn + circuit-breaker stop
- Tool execution hardening: per-tool-call timeout + panic capture in both `process_message` and `process_message_streaming` tool `join_all` paths
- Streaming tool parity: `process_message_streaming()` now mirrors non-streaming hook callbacks, usage-metric accounting, success/failure logging, thinking/response feedback, and malformed tool-argument parse preservation
- Context trimming: normal/emergency/critical compaction tiers (70%/90%/95%)
- Session repair: auto-fixes orphan tool results, empty/duplicate messages, alternation issues
- Config hot-reload: gateway polls config mtime every 30s and applies provider/channel/safety updates
- Config validation: `zeptoclaw config check` recognizes top-level `tunnel` and agent defaults such as `timezone` and `tool_timeout_secs`
- MCP transport: supports both HTTP and stdio MCP servers (`url` or `command` + args/env) with tool registration during `create_agent()`
- Hands-lite: `HAND.toml` + bundled hands (`researcher`, `coder`, `monitor`) + `hand` CLI
- Uninstall CLI: `zeptoclaw uninstall` removes `~/.zeptoclaw`; `--remove-binary` deletes direct installs in `~/.local/bin` or `/usr/local/bin` and defers Homebrew/Cargo binaries to their package managers
- Process exit codes: explicit `main` mapping for success (0) and error (1); uncaught panic/crash remains Rust default (101)
- Tests: current local build runs 3156 lib (3149 passed, 1 failed, 6 ignored) + 92 main + 24 cli_smoke + 13 e2e + 70 integration + 127 doc (27 ignored); optional features such as `whatsapp-web` add feature-gated coverage

## Task Tracking Protocol

**Every session MUST track work via GitHub Issues.**

1. **Start of session** — Run `gh issue list --repo qhkm/zeptoclaw --state open --limit 20` and present open issues
2. **New work** — If no issue exists for the requested work, create one with `gh issue create` before writing code. Use labels: type (`bug`/`feat`/`rfc`/`chore`/`docs`), area (`area:tools`/`area:channels`/etc.), priority (`P1`/`P2`/`P3`)
3. **End of work** — Create PR with `Closes #N` in body, or `gh issue close N` for direct commits
4. **NEVER merge PRs** — Only the user merges PRs. After creating a PR, wait for CI, present the URL to the user, and only merge after explicit user approval

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
cargo nextest run --lib
cargo test --doc
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

## Release Versioning

- Use `patch` for backward-compatible bug fixes, reliability hardening, docs corrections, and internal refactors that do not add user-visible capability.
- Use `minor` for backward-compatible new functionality such as new commands, flags, config fields, tools, providers, runtimes, channels, or other opt-in capabilities.
- If upgrading should only give existing users fixes, choose `patch`.
- If upgrading gives existing users new capabilities without requiring migration, choose `minor`.

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
4. Register in `src/kernel/registrar.rs` inside `register_all_tools()` behind `filter.is_enabled("<name>")`
5. If the tool assumes laptop/server environment (bash, filesystem, shell): make it opt-in by gating on `coding_tools_on` (see the grep/find block in registrar.rs), add it to the `TOOLS` array in `src/cli/tools.rs` with `opt_in: true`, and add it to `opt_in_tool_hint()` in `src/tools/registry.rs`

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
