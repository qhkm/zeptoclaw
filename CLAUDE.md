# ZeptoClaw

Ultra-lightweight personal AI assistant. Fresh configs default to `assistant` mode with dangerous tool approvals enabled.

## Quick Reference

```bash
cargo build --release                      # Build
cargo nextest run --lib                    # Test (use nextest to avoid OOM)
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
gh issue list --repo qhkm/zeptoclaw --state open --limit 20
```
Present issues, ask what to work on.

### 2. New Work — Create issue first
```bash
gh issue create --repo qhkm/zeptoclaw \
  --title "feat: short description" \
  --label "feat,area:tools" \
  --body "Brief description of the work."
```
Labels: `bug`, `feat`, `rfc`, `chore`, `docs` + `area:tools`, `area:channels`, `area:providers`, `area:safety`, `area:config`, `area:cli`, `area:memory` + `P1-critical`, `P2-high`, `P3-normal`. Skip for trivial changes.

### 3. Session End — Link and close
- PR body: include `Closes #N`
- **NEVER merge PRs without explicit user approval.** Wait for CI, present URL, merge only after user says to
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
├── api/         # Panel API server (axum)
├── auth/        # OAuth (PKCE), token refresh, Claude CLI import
├── bus/         # Async message bus
├── channels/    # Telegram, Slack, Discord, Webhook, WhatsApp, Lark, Email, MQTT, Serial
├── cli/         # Clap commands + handlers
├── config/      # Config types/loading + hot-reload
├── cron/        # Persistent cron scheduler
├── deps/        # Dependency manager
├── gateway/     # Containerized agent proxy
├── health.rs    # Health server + metrics
├── memory/      # Workspace + long-term memory (pluggable search)
├── peripherals/ # Hardware: GPIO, I2C, NVS (ESP32, RPi, Arduino)
├── providers/   # Claude, OpenAI, Retry, Fallback, Quota
├── runtime/     # Native, Docker, Apple, Landlock, Firejail, Bubblewrap
├── routines/    # Event/webhook/cron automations
├── r8r_bridge/  # WebSocket bridge for r8r workflow approvals
├── safety/      # Injection detection, leak scanning, policy engine
├── security/    # Shell blocklist, path validation, secret encryption
├── session/     # Session persistence, history, auto-repair
├── tools/       # 33 built-in + MCP + plugins + android
├── utils/       # sanitize, metrics, telemetry, cost
└── main.rs      # Entry point → cli::run()

panel/           # React + Vite dashboard
landing/         # Static landing page
```

For detailed module docs see `docs/claude/architecture.md`.

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

For smoke checklist and benchmarks see `docs/claude/testing.md`.
