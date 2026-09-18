# CLI Commands Reference

## Build & Run

```bash
cargo build --release
cargo build --release --features android    # Android device control
cargo build --release --features mqtt       # MQTT IoT channel

./target/release/zeptoclaw agent -m "Hello"
./target/release/zeptoclaw agent -m "Hello" --no-stream
./target/release/zeptoclaw agent --template <name> -m "..."
./target/release/zeptoclaw gateway
./target/release/zeptoclaw config check
./target/release/zeptoclaw provider status
```

## Interactive Slash Commands (inside `zeptoclaw agent`)

```
/help  /model  /model list  /model <provider:model>
/persona  /persona list  /persona <name>
/tools  /template  /history  /memory
/trust  /trust on  /trust off  /clear  /quit
```

Note: `/trust` and approval prompts only active when both stdin and stdout are real TTYs.

## Telegram Gateway Commands (in chat)

```
/model  /model list  /model reset  /model <provider:model>
/persona  /persona list  /persona <preset>  /persona <custom text>  /persona reset
```

## CLI Commands

```bash
# History
zeptoclaw history list [--limit 20]
zeptoclaw history show <query>
zeptoclaw history cleanup [--keep 50]

# Templates
zeptoclaw template list
zeptoclaw template show <name>

# Hands-lite
zeptoclaw hand list | activate <name> | deactivate | status

# Batch mode
zeptoclaw batch --input prompts.txt [--output results.jsonl --format jsonl --template coder --stop-on-error]

# Secrets
zeptoclaw secrets encrypt | decrypt | rotate

# Memory
zeptoclaw memory list [--category user]
zeptoclaw memory search "query"
zeptoclaw memory set <key> "value" --category user --tags "tag1,tag2"
zeptoclaw memory delete <key>
zeptoclaw memory stats

# Tools
zeptoclaw tools list
zeptoclaw tools info <name>

# Panel
zeptoclaw panel
zeptoclaw panel install | uninstall
zeptoclaw panel auth set-password | show-token

# Channels
zeptoclaw channel list | setup <name> | test <name>

# Quota
zeptoclaw quota status | reset [provider]

# Watch
zeptoclaw watch <url> --interval 1h --notify telegram

# Onboard
zeptoclaw onboard [--full]

# Update / Uninstall
zeptoclaw update [--check | --version v0.5.2 | --force]
zeptoclaw uninstall --yes [--remove-binary]

# Heartbeat & Skills
zeptoclaw heartbeat --show
zeptoclaw skills list

# Gateway with container/tunnel
zeptoclaw gateway --containerized [docker|apple]
zeptoclaw gateway --tunnel [cloudflare|ngrok|tailscale|auto]
```

## Panel Password Login

`POST /api/auth/login` allows five attempts per socket peer IP within a rolling
60-second window. Successful logins and malformed requests also count. Further
attempts receive HTTP 429 with `Retry-After: 60`; wait 60 seconds before retrying.
Static API-token access remains available, and password login returns 404 when
no password is configured.

The limiter ignores forwarded IP headers. Clients behind the same reverse proxy
share its bucket; use a trusted proxy's own rate limiter if individual client
limits are needed. At most 1024 IPs are tracked; new IPs are rejected while all
slots are active, and expired slots are reclaimed without resetting active limits.

## Release

```bash
# Requires: cargo install cargo-release
cargo release patch          # bug fixes (dry-run)
cargo release minor          # new functionality (dry-run)
cargo release patch --execute  # actually release

# patch = backward-compatible fixes, hardening, docs, internal refactors
# minor = new commands, flags, config fields, tools, providers, channels
```

## MCP Server Discovery

Config in `.mcp.json` or `~/.mcp/servers.json`:
```json
{"mcpServers":{"web":{"url":"http://localhost:3000"}}}
{"mcpServers":{"fs":{"command":"node","args":["server.js"]}}}
```
