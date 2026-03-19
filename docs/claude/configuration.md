# Configuration Reference

Config file: `~/.zeptoclaw/config.json`. Validate with `zeptoclaw config check`.

Environment variables override config with pattern `ZEPTOCLAW_<SECTION>_<KEY>`.

## Core Environment Variables

### Provider Keys
- `ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY`
- `ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY`
- `ZEPTOCLAW_OAUTH_CLIENT_ID` — OAuth client id (used by `auth login`)
- `ZEPTOCLAW_PROVIDERS_ANTHROPIC_OAUTH_CLIENT_ID` — provider-specific OAuth override

### Agent Defaults
- `ZEPTOCLAW_AGENTS_DEFAULTS_MODEL`
- `ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS` — wall-clock agent timeout (default: 300)
- `ZEPTOCLAW_AGENTS_DEFAULTS_TOOL_TIMEOUT_SECS` — per-tool timeout (default: 0 = inherit agent)
- `ZEPTOCLAW_AGENTS_DEFAULTS_TIMEZONE` — IANA timezone (default: system or UTC)
- `ZEPTOCLAW_AGENTS_DEFAULTS_TOKEN_BUDGET` — per-session budget (default: 0 = unlimited)
- `ZEPTOCLAW_AGENTS_DEFAULTS_MESSAGE_QUEUE_MODE` — "collect" (default) or "followup"
- `ZEPTOCLAW_AGENTS_DEFAULTS_SYSTEM_PROMPT` — custom system prompt

### Channels
- `ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN`
- `ZEPTOCLAW_CHANNELS_WHATSAPP_WEB_ENABLED` (default: false)
- `ZEPTOCLAW_CHANNELS_WHATSAPP_WEB_AUTH_DIR` (default: ~/.zeptoclaw/state/whatsapp_web)
- `ZEPTOCLAW_CHANNELS_ACP_ENABLED` (default: false)
- `ZEPTOCLAW_CHANNELS_ACP_HTTP_ENABLED` (default: false)
- `ZEPTOCLAW_CHANNELS_ACP_HTTP_PORT` (default: 8765)
- `ZEPTOCLAW_CHANNELS_ACP_HTTP_BIND` (default: 127.0.0.1)
- `ZEPTOCLAW_CHANNELS_ACP_HTTP_AUTH_TOKEN` — Bearer auth token (default: none)

### Retry & Fallback
- `ZEPTOCLAW_PROVIDERS_RETRY_ENABLED` (default: false)
- `ZEPTOCLAW_PROVIDERS_RETRY_MAX_RETRIES` (default: 3)
- `ZEPTOCLAW_PROVIDERS_RETRY_BASE_DELAY_MS` (default: 1000)
- `ZEPTOCLAW_PROVIDERS_RETRY_MAX_DELAY_MS` (default: 30000)
- `ZEPTOCLAW_PROVIDERS_RETRY_BUDGET_MS` — total wall-clock budget, 0=unlimited (default: 45000)
- `ZEPTOCLAW_PROVIDERS_FALLBACK_ENABLED` (default: false)
- `ZEPTOCLAW_PROVIDERS_FALLBACK_PROVIDER` — fallback provider name

### Per-Provider Overrides
- `ZEPTOCLAW_PROVIDERS_<NAME>_MODEL` — model override per provider (e.g. `ZEPTOCLAW_PROVIDERS_NVIDIA_MODEL=nvidia/llama-3.3-70b`)
- `ZEPTOCLAW_PROVIDERS_<NAME>_QUOTA_MAX_COST_USD` / `_MAX_TOKENS` / `_PERIOD` / `_ACTION`

### Provider-Specific Keys
- Azure: `ZEPTOCLAW_PROVIDERS_AZURE_API_KEY` (or `AZURE_OPENAI_API_KEY`), `_API_BASE` (or `AZURE_OPENAI_ENDPOINT`), `_API_VERSION`
- Bedrock: `ZEPTOCLAW_PROVIDERS_BEDROCK_API_KEY` (or `AWS_ACCESS_KEY_ID`), `_API_BASE`
- xAI: `ZEPTOCLAW_PROVIDERS_XAI_API_KEY` (or `XAI_API_KEY`), `_API_BASE`, `_MODEL`
- Qianfan: `ZEPTOCLAW_PROVIDERS_QIANFAN_API_KEY` (or `QIANFAN_API_KEY`), `_API_BASE`, `_MODEL`

### Safety & Security
- `ZEPTOCLAW_SAFETY_ENABLED` (default: true)
- `ZEPTOCLAW_SAFETY_LEAK_DETECTION_ENABLED` (default: true)
- `ZEPTOCLAW_MASTER_KEY` — hex-encoded 32-byte encryption key

### Features
- `ZEPTOCLAW_COMPACTION_ENABLED` (default: false)
- `ZEPTOCLAW_COMPACTION_CONTEXT_LIMIT` (default: 100000)
- `ZEPTOCLAW_COMPACTION_THRESHOLD` (default: 0.80)
- `ZEPTOCLAW_ROUTINES_ENABLED` (default: false)
- `ZEPTOCLAW_ROUTINES_CRON_INTERVAL_SECS` (default: 60)
- `ZEPTOCLAW_ROUTINES_MAX_CONCURRENT` (default: 3)
- `ZEPTOCLAW_ROUTINES_JITTER_MS` (default: 0)
- `ZEPTOCLAW_ROUTINES_ON_MISS` — "skip" (default) or "run_once"
- `ZEPTOCLAW_HEARTBEAT_DELIVER_TO` — channel for delivery

### Memory
- `ZEPTOCLAW_MEMORY_BACKEND` — builtin (default), bm25, embedding, hnsw, tantivy, none
- `ZEPTOCLAW_MEMORY_EMBEDDING_PROVIDER` / `_EMBEDDING_MODEL`

### Panel
- `ZEPTOCLAW_PANEL_ENABLED` (default: false)
- `ZEPTOCLAW_PANEL_PORT` (default: 9092)
- `ZEPTOCLAW_PANEL_API_PORT` (default: 9091)
- `ZEPTOCLAW_PANEL_BIND` (default: 127.0.0.1)

### Tools
- `ZEPTOCLAW_TOOLS_WEB_SEARCH_PROVIDER` — "brave", "searxng", "ddg" (default: auto-detect)
- `ZEPTOCLAW_TOOLS_WEB_SEARCH_API_URL` — SearXNG instance URL
- `ZEPTOCLAW_TOOLS_CODING_TOOLS` — enable grep, find (default: false; auto-enabled by coder template)

### Tunnel
- `ZEPTOCLAW_TUNNEL_PROVIDER` — cloudflare, ngrok, tailscale, auto

## Keyless Providers

Ollama and vLLM do not require an API key:
```json
{"providers": {"ollama": {}}}
{"providers": {"ollama": {"api_base": "https://my-cloud-ollama.example.com/v1"}}}
{"providers": {"ollama": {"api_key": "secret", "api_base": "https://my-cloud-ollama.example.com/v1"}}}
```
No `api_key` = no Authorization header. With `api_key` = `Bearer <key>`.

## Cargo Features

| Feature | Description |
|---------|-------------|
| `android` | Android device control via ADB |
| `google` | Google Workspace (Gmail + Calendar) via gogcli-rs |
| `mqtt` | MQTT channel for IoT (rumqttc) |
| `whatsapp-web` | Native WhatsApp Web via wa-rs |
| `memory-bm25` | BM25 keyword scoring for memory |
| `peripheral-esp32` | ESP32 peripheral with I2C + NVS (implies hardware) |
| `peripheral-rpi` | RPi GPIO + I2C via rppal (Linux only) |
| `sandbox-landlock` | Landlock LSM runtime (Linux only) |
| `sandbox-firejail` | Firejail runtime (Linux only) |
| `sandbox-bubblewrap` | Bubblewrap runtime (Linux only) |

```bash
cargo build --release --features android
cargo build --release --features whatsapp-web
cargo build --release --features sandbox-landlock,sandbox-firejail,sandbox-bubblewrap
```

## Compile-time Defaults

```bash
export ZEPTOCLAW_DEFAULT_MODEL=gpt-5.1           # Default agent model (default: claude-sonnet-4-5-20250929)
export ZEPTOCLAW_CLAUDE_DEFAULT_MODEL=...          # Default Claude model
export ZEPTOCLAW_OPENAI_DEFAULT_MODEL=gpt-5.1      # Default OpenAI model
cargo build --release
```
