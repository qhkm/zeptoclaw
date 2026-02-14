# ZeptoClaw Roadmap

> Last updated: 2026-02-14

## Process

After completing any feature, update these 3 files:
1. **This file** (`docs/plans/TODO.md`) — check off items, update "Last updated" date
2. **`CLAUDE.md`** — architecture tree, test counts, module docs, CLI flags
3. **`AGENTS.md`** — "Current State" section, test counts, "Not Yet Wired" list

---

## Completed

### Quick Wins (2026-02-13)
- [x] Tool result sanitization (`src/utils/sanitize.rs`) — strips base64, hex blobs, truncates to 50KB
- [x] Parallel tool execution (`src/agent/loop.rs`) — `futures::future::join_all`
- [x] Agent-level timeout — `agent_timeout_secs` config field (default 300s)
- [x] Config validation CLI (`src/config/validate.rs`) — `zeptoclaw config check`
- [x] Message queue modes — Collect (default) and Followup for busy sessions

### Agent Swarm (2026-02-13)
- [x] SwarmConfig + SwarmRole structs (`src/config/types.rs`)
- [x] DelegateTool (`src/tools/delegate.rs`) — creates sub-agent with role-specific prompt + tool whitelist
- [x] Recursion blocking via channel check
- [x] ProviderRef wrapper for shared provider
- [x] Wired into `create_agent()` after provider resolution

### Streaming Responses (2026-02-14)
- [x] StreamEvent enum + `chat_stream()` default on LLMProvider trait
- [x] Claude SSE streaming (`src/providers/claude.rs`)
- [x] OpenAI SSE streaming (`src/providers/openai.rs`)
- [x] Streaming config field + `--stream` CLI flag
- [x] ProviderRef `chat_stream()` forwarding for delegate tool
- [x] `process_message_streaming()` on AgentLoop
- [x] CLI output wiring (single-message + interactive modes)
- [x] Integration tests

### Provider Infrastructure (2026-02-14)
- [x] RetryProvider (`src/providers/retry.rs`) — exponential backoff on 429/5xx
- [x] FallbackProvider (`src/providers/fallback.rs`) — primary → secondary auto-failover
- [x] MetricsCollector (`src/utils/metrics.rs`) — tool call stats, token tracking, session summary

### Wiring (2026-02-14)
- [x] Config fields for retry/fallback (`config/types.rs`, `config/mod.rs` env overrides)
- [x] RetryProvider wired into provider resolution — base → fallback → retry stack
- [x] FallbackProvider wired with multi-provider resolution (`providers/registry.rs`)
- [x] MetricsCollector wired into AgentLoop — tracks tool duration/success + token usage
- [x] Status output shows retry/fallback state

### Agent Loop / CLI Wiring (2026-02-14)
- [x] ConversationHistory CLI commands — `history list`, `history show <query>`, `history cleanup`
- [x] TokenBudget wired — `token_budget` config field + env override + budget check in agent loop
- [x] OutputFormat wired — `output_format` field on `ChatOptions` + OpenAI `response_format` + Claude system suffix
- [x] LongTermMemory tool — `longterm_memory` agent tool (set/get/search/delete/list/categories), 22 tests

### Features (2026-02-14)
- [x] **Conversation persistence** (`src/session/history.rs`) — 12 tests
- [x] **Token budget** (`src/agent/budget.rs`) — 18 tests
- [x] **Structured output** (`src/providers/structured.rs`) — 19 tests
- [x] **Multi-turn memory** (`src/memory/longterm.rs`) — 19 tests
- [x] **Webhook channel** (`src/channels/webhook.rs`) — 28 tests
- [x] **Discord channel** (`src/channels/discord.rs`) — 27 tests
- [x] **Tool approval** (`src/tools/approval.rs`) — 24 tests
- [x] **Agent templates** (`src/config/templates.rs`) — 21 tests
- [x] **Plugin system** (`src/plugins/`) — 70+ tests
- [x] **Telemetry export** (`src/utils/telemetry.rs`) — 13 tests
- [x] **Cost tracking** (`src/utils/cost.rs`) — 18 tests
- [x] **Batch mode** (`src/batch.rs`) — 15+ tests
- [x] **Hooks system** (`src/hooks/mod.rs`) — config-driven before_tool/after_tool/on_error, 17 tests
- [x] **Deploy templates** (`deploy/`) — Docker single/multi, Fly.io, Railway, Render

---

## Backlog

### P0 — CI/CD & Infrastructure
- [x] **GitHub Actions CI** — `.github/workflows/ci.yml` with test, clippy, fmt jobs
- [x] **Release workflow** — `.github/workflows/release.yml` cross-compile 4 targets, GitHub Release with sha256
- [x] **Docker image CI** — `.github/workflows/docker.yml` build + push to ghcr.io on tag

### P1 — Test Coverage Gaps
- [x] **Discord channel tests** — 19 new tests: config serde, gateway payloads, outbound truncation, intents bitmask, edge cases (46 total)
- [x] **CronTool tests** — 14 new tests: add/list/remove actions, validation, error paths
- [x] **SpawnTool tests** — 10 new tests: delegation, recursion blocking, labels, error paths
- [x] **Filesystem security tests** — 5 new tests: path traversal, URL-encoded bypass, workspace boundary, absolute path rejection
- [x] **Web tool SSRF tests** — 7 new tests: private IP ranges, IPv6, non-HTTP schemes, body size limits, no-host URLs
- [x] **GSheets error path tests** — 7 new tests: unknown action, missing args, malformed values, base64 errors, path injection
- [x] **Integration test expansion** — added 5 integration tests for fallback provider flow, cron dispatch, heartbeat trigger/skip behavior, and skills availability filtering (`tests/integration.rs`)

### P2 — Code Quality
- [x] **R8rTool error handling** — already uses match + warn! + fallback (not .expect()), no change needed
- [x] **README provider count** — updated to clarify "Anthropic and OpenAI today" + staged rollout for others
- [x] **README hooks status** — updated from "wiring in progress" to "fully wired into agent loop"
- [x] **Update stats** — 953 lib + 68 integration + 98 doc = 1,119 total tests; 17 tools; hooks system fully wired

### P3 — Documentation
- [x] **Module-level docs** — all four files already have `//!` module docs (plugins, hooks, batch, telemetry)
- [x] **Public API docs** — `generate_env_file_content()` already has doc comment; others renamed/removed
- [x] **Deployment guide** — `deploy/README.md` with step-by-step for Docker, Fly.io, Railway, Render

### P4 — Features
- [ ] **Web UI** — browser-based chat interface (minimal: single HTML page with SSE)
- [ ] **Embeddings memory** — vector search for long-term memory (inspired by Moltis)
- [ ] **Hook notify action** — wire `Notify` hook action to actually send messages via bus (currently logs only)
- [ ] **Pre-commit hooks** — `cargo fmt` + `cargo clippy` enforcement via `.git/hooks/pre-commit`

### P5 — Repo Hygiene
- [ ] **Remove `landing/r8r/docs/node_modules/`** from disk or ensure fully gitignored
- [ ] **Audit `.clone()` calls** — 233 across codebase, most necessary but worth a pass for unnecessary string clones
- [ ] **Clean up docs/internal/** — verify competitor research is properly gitignored

---

## Stats

- Codebase: ~39,000 lines of Rust
- Tests: 953 lib + 68 integration + 98 doc = **1,119 total**
- Tools: 17 agent tools + dynamic plugin tools
- Channels: 4 (Telegram, Slack, Discord, Webhook)
- Providers: 2 (Claude, OpenAI) + RetryProvider + FallbackProvider
- Hooks: 3 points (before_tool, after_tool, on_error)
- Deploy targets: 5 (Docker single, Docker multi, Fly.io, Railway, Render)
- Binary: ~5.3MB release (opt-level="z", lto, strip)
