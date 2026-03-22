# Architecture Details

## Key Design Patterns

- **Async-first**: All I/O uses Tokio. `spawn_blocking` for sync I/O (memory, filesystem)
- **Trait-based**: `LLMProvider`, `Channel`, `Tool`, `ContainerRuntime`
- **Arc shared state**: `Arc<dyn LLMProvider>`, `Arc<dyn ContainerRuntime>`
- **Parallel tool execution**: `futures::future::join_all`
- **Tool result sanitization**: strip base64, hex, truncate to 50KB
- **Per-session mutex map**: prevents concurrent message race conditions
- **Conditional compilation**: `#[cfg(target_os = "macos")]` for Apple-specific code

## Providers (`src/providers/`)

`LLMProvider` trait with implementations:
- `ClaudeProvider` — Anthropic Claude API (120s timeout, SSE streaming)
- `OpenAIProvider` — OpenAI Chat Completions API; supports any compatible endpoint via `api_base` (Ollama, Groq, Zhipu/GLM, Together, Fireworks, LM Studio, vLLM, DeepSeek, Kimi/Moonshot, Azure, Bedrock, xAI/Grok, Baidu Qianfan). Custom auth header via `auth_header`, API version via `api_version`
- `RetryProvider` — exponential backoff on 429/5xx
- `FallbackProvider` — primary → secondary auto-failover with circuit breaker (Closed/Open/HalfOpen)
- `QuotaProvider` — per-provider cost/token quota enforcement; action: reject, failover, warn

Provider stack assembly in `create_agent()`: base providers → optional FallbackProvider → optional RetryProvider. `ProviderError` enum (Auth, RateLimit, Billing, ServerError, InvalidRequest, ModelNotFound, Timeout) enables smart retry/fallback. Per-provider model mapping via `ProviderConfig.model`. Streaming via `StreamEvent` + `chat_stream()`. `OutputFormat` enum (Text/Json/JsonSchema).

## Channels (`src/channels/`)

`Channel` trait implementations:
- `TelegramChannel` — numeric-ID allowlist default, legacy username behind `allow_usernames`
- `SlackChannel` — outbound messaging
- `DiscordChannel` — Gateway WebSocket + REST (reply + thread create)
- `WebhookChannel` — HTTP POST inbound with Bearer + HMAC-SHA256 auth, fixed server-side identity
- `WhatsAppWebChannel` — wa-rs native (QR pairing, feature: `whatsapp-web`)
- `WhatsAppCloudChannel` — signed webhook + REST
- `LarkChannel` — WS long-connection
- `EmailChannel` — IMAP IDLE + SMTP, From-header trust
- `MqttChannel` — rumqttc async (feature: `mqtt`)
- `SerialChannel` — UART line-delimited JSON (feature: `hardware`)

`ChannelManager`: `Arc<Mutex<_>>` handles, polling supervisor (15s detect dead, 60s cooldown, max 5 restarts). Per-chat persona via `/persona` + `PersonaOverrideStore` (LTM persistence). All channels support `deny_by_default`.

## Agent (`src/agent/`)

- `AgentLoop` — core message loop with tool execution + pre-compaction memory flush + per-message LTM injection
- `process_message_streaming()` mirrors non-streaming loop for hooks, metrics, logging
- `ContextBuilder` — system prompt + conversation context + optional per-message memory override
- `TokenBudget` — atomic per-session tracker (lock-free `AtomicU64`)
- `ContextMonitor` — token estimation (`words * 1.3 + 4/msg`), threshold-based compaction
- `LoopGuard` — SHA256 tool-call repetition detection with warning + circuit breaker
- `Compactor` — Summarize (LLM-based) or Truncate strategies
- `SwarmScratchpad` — `Arc<RwLock<HashMap>>` for agent-to-agent context (2000 chars per entry)
- `start()` routes through `process_inbound_message()` → `try_queue_or_process()`

## Tools (`src/tools/`)

33 built-in + dynamic MCP + composed tools via `Tool` async trait. All filesystem tools require workspace.

**Composed tools** (`composed.rs`): `CreateToolTool` (create/list/delete/run), `ComposedTool` (interpolates `{{param}}` placeholders). Stored at `~/.zeptoclaw/composed_tools.json`.

**Delegate tool** (`delegate.rs`): `DelegateTool` with `run` (single task) and `aggregate` (multiple). `parallel: true` = concurrent via `join_all` + semaphore (`swarm.max_concurrent`). `parallel: false` = sequential with `SwarmScratchpad` chaining. Recursion blocked. `ProviderRef` wrapper shares `Arc<dyn LLMProvider>`. Config: `SwarmConfig` (enabled, max_depth=1, max_concurrent=3, roles).

**MCP client** (`mcp/`): JSON-RPC 2.0 protocol, `McpTransport` trait (HTTP + stdio), `McpClient` with tools cache, `McpToolWrapper` adapts to Tool trait with prefixed names (`{server}_{tool}`). Discovery via `.mcp.json` / `~/.mcp/servers.json`.

## Safety (`src/safety/`)

- `SafetyLayer` — orchestrator: length → leak detection → policy → injection sanitization
- `sanitizer.rs` — Aho-Corasick 17 patterns + 4 regex for prompt injection
- `leak_detector.rs` — 22 regex patterns for API keys/tokens/secrets; Block/Redact/Warn
- `policy.rs` — 7 rules (system files, crypto keys, SQL, shell injection, encoded exploits) with selective ignore
- `validator.rs` — 100KB max, null byte, whitespace ratio, repetition detection
- `chain_alert.rs` — per-session tool sequence tracking, warns on dangerous patterns
- Tiered inbound scanning: webhook=block, allowlisted channels=warn-only

## Security (`src/security/`)

- `shell.rs` — regex blocklist + optional allowlist (Off/Warn/Strict); blocks `.zeptoclaw/config.json` exfiltration
- `path.rs` — workspace validation, symlink escape detection, secure dir-chain creation
- `mount.rs` — allowlist validation, docker binary verification, traversal rejection, hardlink alias rejection
- `encryption.rs` — XChaCha20-Poly1305 AEAD + Argon2id KDF, `ENC[...]` format, transparent config decrypt
- `agent_mode.rs` — Observer/Assistant/Autonomous (defaults to Assistant)

## Memory (`src/memory/`)

- `MemorySearcher` trait — pluggable backends (builtin, bm25, embedding, hnsw, tantivy)
- `BuiltinSearcher` — substring + term-frequency (always compiled)
- `Bm25Searcher` — Okapi BM25 (feature: `memory-bm25`)
- `LongTermMemory` — KV store at `~/.zeptoclaw/memory/longterm.json` with categories, tags, access tracking, injection guard
- `decay_score()` — 30-day half-life with importance weighting; pinned entries exempt
- `build_memory_injection()` — pinned + query-matched injection (2000 char budget)
- Pre-compaction memory flush — silent LLM turn saves facts before compaction (10s timeout)

## Other Modules

- **Runtime** (`src/runtime/`): Native, Docker, Apple Container (macOS 15+), Landlock (Linux 5.13+), Firejail, Bubblewrap
- **Gateway** (`src/gateway/`): stdin/stdout IPC, semaphore concurrency, mount allowlist validation
- **Auth** (`src/auth/`): OAuth PKCE, CSRF, encrypted token store, Claude CLI credential import (Keychain/json)
- **Deps** (`src/deps/`): `HasDependencies` trait, `DepKind` (Binary/Docker/Npm/Pip), registry at `~/.zeptoclaw/deps/registry.json`
- **Health** (`src/health.rs`): `/health` (version, uptime, RSS, metrics, checks), `/ready`, raw TCP server
- **API** (`src/api/`): axum, EventBus (broadcast), AppState, JWT + Bearer auth, CSRF, WebSocket streaming, TaskStore
- **Session** (`src/session/`): `SessionManager`, `ConversationHistory` (fuzzy search), `repair.rs`
- **Routines** (`src/routines/`): Trigger (Cron/Event/Webhook/Manual), `RoutineStore`, `RoutineEngine` with regex cache
- **R8r Bridge** (`src/r8r_bridge/`): WebSocket bridge for r8r workflow approvals, health pings, event deduplication
- **Tunnel** (`src/tunnel/`): Cloudflare, ngrok, Tailscale, auto-detect
- **Batch** (`src/batch.rs`): text/JSONL input, `BatchResult`, plain text or JSONL output
- **Utils** (`src/utils/`): sanitize, MetricsCollector, Prometheus telemetry, CostTracker (8 model pricing tables)

## Key Paths

| Path | Purpose |
|------|---------|
| `~/.zeptoclaw/config.json` | Main configuration |
| `~/.zeptoclaw/memory/longterm.json` | Long-term memory store |
| `~/.zeptoclaw/composed_tools.json` | User-created composed tools |
| `~/.zeptoclaw/deps/registry.json` | Installed dependency tracking |
| `~/.zeptoclaw/skills/<name>/SKILL.md` | Skill definitions |
| `.mcp.json` / `~/.mcp/servers.json` | MCP server discovery |
