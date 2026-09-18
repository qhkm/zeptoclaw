# ZeptoClaw

Fast, small, secure, local-first personal AI assistant infrastructure. Fresh configs default to `assistant` mode with dangerous tool approvals enabled.

## Quick Reference

```bash
cargo build --release                      # Build
cargo nextest run --lib                    # Test (use nextest to avoid OOM)
cargo deny check                          # Dependency security and policy checks
cargo clippy -- -D warnings && cargo fmt   # Lint & format
./target/release/zeptoclaw agent -m "Hello"  # Run agent
./target/release/zeptoclaw config check      # Validate config
./target/release/zeptoclaw provider status   # Check providers
```

For full CLI reference, slash commands, and gateway commands see `docs/claude/commands.md`.

## Agent Workflow — Task Tracking Protocol

Every Claude Code session MUST follow these rules:

### 1. Session Start — Check open issues
```bash
gh issue list --state open --limit 20
```
Present issues, ask what to work on.

### 2. New Work — Create issue first
```bash
gh issue create \
  --title "feat: short description" \
  --label "feat,area:tools" \
  --body "Brief description of the work."
```
Labels: `bug`, `feat`, `rfc`, `chore`, `docs` + `area:tools`, `area:channels`, `area:providers`, `area:safety`, `area:config`, `area:cli`, `area:memory` + `P1-critical`, `P2-high`, `P3-normal`. Skip for trivial changes.

### 3. Session End — Link and close
- Follow the PR guidelines in `docs/claude/pr.md` and use the template at `.github/PULL_REQUEST_TEMPLATE.md`
- PR body: include `Closes #N`
- **NEVER merge PRs without explicit user approval.** Present local validation results and the PR URL, merge only after user says to
- Merge: `gh pr merge <number> --squash --delete-branch --admin`
- Direct commit: `gh issue close N --comment "Done in <commit-sha>"`
- Update `CLAUDE.md` and `AGENTS.md` per the post-implementation checklist

## Pre-Push Checklist (MANDATORY)

```bash
cargo fmt && cargo clippy -- -D warnings && cargo nextest run --lib && cargo test --doc && cargo fmt -- --check
```

**After subagent work:** ALWAYS run `cargo fmt` before committing.

## Architecture

```
src/
├── agent/       # Agent loop, context builder, token budget, compaction
├── api/         # Panel API, rate-limited password login, one-time WebSocket auth tickets, OpenAI-compatible routes (axum)
├── auth/        # OAuth (PKCE), token refresh, Claude CLI import
├── bus/         # Async message bus
├── channels/    # Telegram, Slack, Discord, Webhook, WhatsApp Web/Cloud, Lark, Email, Serial, ACP; MQTT parked
├── cli/         # Clap commands + handlers
├── config/      # Config types/loading + hot-reload
├── cron/        # Persistent cron scheduler
├── deps/        # Dependency manager
├── gateway/     # Containerized agent proxy
├── health.rs    # Health server + metrics
├── memory/      # Workspace + long-term memory (pluggable search)
├── peripherals/ # Hardware: GPIO, I2C, NVS (ESP32, RPi, Arduino)
├── providers/   # Claude, OpenAI, Retry, Fallback, Quota
├── runtime/     # Six runtimes + shared scrubbed-env/process-tree executor
├── routines/    # Event/webhook/cron automations
├── r8r_bridge/  # WebSocket bridge for r8r workflow approvals
├── safety/      # Injection detection, leak scanning, policy engine
├── security/    # Shell blocklist, path validation, secret encryption
├── session/     # Session persistence, history, auto-repair
├── tools/       # 33 built-in + MCP + plugins + android
├── utils/       # sanitize, secure filesystem writes, metrics, telemetry, cost
└── main.rs      # Entry point → cli::run()

panel/           # React + Vite dashboard
landing/         # Static landing page
```

For detailed module docs see `docs/claude/architecture.md`.

## Coding Core Notes

- The dependency audit baseline passes `cargo deny check` with patched `anyhow` 1.0.103, `bcrypt` 0.19.2, `crossbeam-epoch` 0.9.20, `quinn-proto` 0.11.15, `quick-xml` 0.41, `lopdf` 0.42, and `rustls` 0.23.45 (RUSTSEC-2026-0285).
- Outbound tool schemas pass through `utils::tool_schema::sanitize_schema()` in every `ToolRegistry::definitions*()` path, so MCP- and plugin-supplied schemas cannot 400 a whole provider request (illegal property keys, `type` arrays, bare-string schemas, `default` beside `$ref`, top-level combinators) or break llama.cpp's GBNF grammar converter (an object schema with no `properties`). Inbound, `kernel::execute_tool()` runs `unrename_tool_args()` then `coerce_tool_args()`, so renamed keys round-trip to their wire names and the string-typed scalars small local models emit (`"42"`, `"true"`, JSON-encoded containers) reach tools as their declared types at every nesting depth. Both layers are conservative: a string is never rewritten when the schema already permits `string`, so an identifier like `"07030"` survives a `["string", "integer"]` union intact.
- Embedded `ZeptoAgent` tool calls use the same `kernel::execute_tool()` path as the main agent loop and MCP server, so safety scanning, taint checks, and tool metrics stay aligned across entry points.
- Embedded `ZeptoAgent` also supports per-tool timeout, panic capture, and configurable approval gating via the builder for safer embedded coding-agent execution.
- The `panel` CLI namespace is always parsed, but panel-backed behavior still requires the optional Cargo `panel` feature; feature-disabled builds now fail with explicit build/install guidance instead of a Clap unknown-subcommand error.
- Panel password login uses the shared `SlidingWindowRateLimiter` before JSON parsing and bcrypt: five attempts per socket peer IP per rolling 60 seconds, with 429 and `Retry-After: 60` after exhaustion. It tracks at most 1024 IPs, rejecting new IPs at capacity until expired slots can be reclaimed. Forwarded headers are ignored; reverse-proxy clients share the proxy's bucket. `start_server()` supplies `ConnectInfo<SocketAddr>` automatically; embedded password-login routers must serve with `into_make_service_with_connect_info::<SocketAddr>()` or login fails closed with 500. Static-token access remains independent.
- Model-driven provider inference treats vendor-prefixed gateway IDs like `anthropic/...` as OpenRouter models only when OpenRouter is actually available, and live provider model discovery now carries `api-version` while normalizing Azure deployment bases to `/openai/models`.
- The OpenAI-compatible `/v1/chat/completions` serve path forwards request tools, returns OpenAI-style tool-call payloads for assistant/tool messages, and the default provider streaming adapter now emits a text delta plus tool-call events before `Done` so non-native streaming providers are not silently flattened.
- Telegram gateway responses support opt-in cumulative streaming (`channels.telegram.streaming`): the agent loop rate-limits outbound stream phases, Telegram progressively edits UTF-16-safe previews, preserves replies/forum topics, and falls back to a fresh final HTML message after preview failures.
- The serve API only accepts omitted, `null`, or `"auto"` for `tool_choice`; unsupported values are rejected with `400` instead of being ignored.
- `src/audit.rs` now includes an in-memory SHA-256 hash chain (`record_audit_chain_event`, `verify_audit_chain_integrity`, `recent_audit_entries`, `audit_tip_hash`), and `kernel::execute_tool()` records per-call audit entries including shell/network/spawn classifications.
- GitHub Actions CI, E2E, and PR hygiene workflows are removed. Run quality gates locally and check optional features when changing them, for example `cargo check --features channel-email` or `cargo nextest run --lib --features memory-bm25`. Tag-triggered release and Docker publishing remain enabled.
- **Binary size budgets: 11MB linux-x86_64, 7MB aarch64** — measure stripped release binaries locally when relevant. Gate heavy dependencies behind a Cargo feature flag rather than increasing the budgets.
- `shell` tool output is truncated at 2,000 lines / 50KB before it reaches the model context.
- Runtime subprocesses scrub secret-like inherited environment variables by default and terminate/reap their process group on Unix timeouts; `runtime.env_passthrough` is the explicit compatibility escape hatch.
- `grep` reports subprocess failures instead of collapsing them into "No matches found".
- `edit_file` rejects empty `old_text` and accepts optional `expected_replacements` to guard exact-match edits.
- `longterm_memory` includes explicit use/counter-use trigger phrases so durable user corrections, preferences, and project conventions are persisted only when they generalize beyond the current task.

## Common Tasks

### Add a new provider/tool/channel
1. Create file in `src/{providers,tools,channels}/`
2. Implement trait (`LLMProvider`/`Tool`/`Channel`)
3. Export from module's `mod.rs` (tools also need `src/lib.rs`)
4. Wire in `src/cli/common.rs` (providers/tools) or `src/cli/gateway.rs` (channels)

### Add a new skill
1. Create `~/.zeptoclaw/skills/<name>/SKILL.md` with YAML frontmatter
2. Or: `zeptoclaw skills create <name>`
3. Loader priority: `metadata.zeptoclaw` > `metadata.openclaw` > raw. Extensions: `os`, `requires.anyBins`
4. Core skills in `skills/` (`github`, `skill-creator`, `deep-research`), community at github.com/qhkm/zeptoclaw-skills

## Configuration

Config: `~/.zeptoclaw/config.json`. Env vars override with pattern `ZEPTOCLAW_<SECTION>_<KEY>`.

For full env var reference, cargo features, and compile-time config see `docs/claude/configuration.md`.

## Testing

```bash
cargo nextest run --lib                    # Unit tests
cargo nextest run --test cli_smoke | e2e | integration
cargo nextest run test_name                # Specific test
```

Current validation: `cargo fmt -- --check`, `cargo clippy -- -D warnings` (default and `--features panel`), and `cargo nextest run --lib` pass (3589 default / 3797 with `panel`, 6 skipped each). `CARGO_INCREMENTAL=0 cargo test --doc` passes 128 examples with 27 ignored; incremental compilation was disabled after macOS linker cache errors.

For smoke checklist and benchmarks see `docs/claude/testing.md`.
