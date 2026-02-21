# Cache, Security & Hardware Design

**Date:** 2026-02-21
**Status:** Approved
**Scope:** Response cache, device pairing, autonomy levels, full hardware/peripheral stack

---

## Context

Post-zeroclaw2 adoption, three gaps remain that align with ZeptoClaw's vision:

1. **Response cache** — avoid burning tokens on repeated prompts (immediate value)
2. **Device pairing + autonomy levels** — essential before hardware enters the picture; controls who can connect and what the agent can do
3. **Hardware support** — the long-term IoT moat; port the full peripheral stack from zeroclaw2

Memory backends are NOT in scope — the pluggable `MemorySearcher` architecture already covers SQLite FTS, PostgreSQL, and vector embeddings as user-opt-in backends.

---

## 1. Response Cache

**Why:** Avoids burning tokens on repeated prompts. If you ask "What is Rust?" twice to gpt-4, the second response comes from cache instead of paying tokens.

**Design:**
- New file `src/cache/response_cache.rs` with `ResponseCache` struct
- Cache key: SHA-256 of `(model, system_prompt_hash, user_prompt)` — deterministic, model-aware
- TTL-based expiry (default: 1 hour, configurable via `cache.ttl_secs`)
- LRU eviction when capacity exceeded (default: 500 entries, configurable via `cache.max_entries`)
- Storage: JSON file at `~/.zeptoclaw/cache/responses.json` (matches longterm memory pattern)
- Each entry stores: response text, token count, created_at, accessed_at, hit_count
- `stats()` returns total entries, total hits, total tokens saved
- Wiring: checked in agent loop before calling provider — on hit, return cached response; on miss, call provider then cache the result
- Zero new deps (sha2 already in dep tree, serde_json already used)

**Config:**
- `cache.enabled` (default: false)
- `cache.ttl_secs` (default: 3600)
- `cache.max_entries` (default: 500)

**Tests:** Cache hit/miss, TTL expiry, LRU eviction, key determinism, stats tracking, clear (~12 tests)

---

## 2. Device Pairing

**Why:** Critical for IoT — you don't want an unknown RPi or phone sending commands to your agent. Secures the gateway with per-device bearer tokens.

**Design:**
- New file `src/security/pairing.rs` with `PairingManager` struct
- Pairing flow: agent generates one-time 6-digit code (displayed in terminal) → client sends code → server validates → issues bearer token (SHA-256 hashed for storage) → client uses token for all future requests
- Token store: JSON file at `~/.zeptoclaw/security/paired_devices.json` — stores hashed tokens, device name, paired_at, last_seen
- Brute-force protection: 5 attempts per IP/device, then 5-minute lockout
- Gateway integration: middleware check — if pairing enabled, reject requests without valid bearer token
- CLI: `zeptoclaw pair new`, `zeptoclaw pair list`, `zeptoclaw pair revoke <device>`
- When disabled (default), gateway works as today — no auth required

**Config:**
- `security.pairing.enabled` (default: false)
- `security.pairing.max_attempts` (default: 5)
- `security.pairing.lockout_secs` (default: 300)

**Tests:** Code generation, token hashing/validation, brute-force lockout, revocation, CLI command parsing (~15 tests)

---

## 3. Autonomy Levels

**Why:** A laptop agent can run shell commands; a robot agent should be read-only until explicitly supervised. Controls what the agent is allowed to do based on context.

**Design:**
- New file `src/security/autonomy.rs` with `AutonomyLevel` enum and `AutonomyPolicy` struct
- Three levels:
  - `ReadOnly` — read files, search, fetch web, query memory. No write/execute/send
  - `Supervised` — everything allowed but Execute/Destructive tools require approval (reuses `ApprovalGate`)
  - `Full` — no restrictions (current behavior, stays default)
- Risk classification: each tool gets a `ToolRisk` tag (Read/Write/Execute/Destructive)
  - ReadOnly: only Read tools allowed
  - Supervised: Read + Write auto-allowed, Execute + Destructive require approval
  - Full: everything auto-allowed
- Wiring: checked in agent loop before tool execution, before `ApprovalGate`. If blocked by autonomy, tool doesn't reach approval
- Per-template override: `agent --template robot --autonomy supervised`

**Config:**
- `security.autonomy_level` (default: `"full"`)
- Env var: `ZEPTOCLAW_SECURITY_AUTONOMY_LEVEL`

**Tests:** Level enforcement per risk category, config/env override, template override, interaction with approval gate (~15 tests)

---

## 4. Hardware Support (Full Peripheral Stack)

**Why:** The long-term IoT moat. USB discovery, serial communication, and board-specific support. Feature-gated to keep default binary lean.

**Design:**

### Hardware layer (`src/hardware/`)
- `mod.rs` — orchestrator, public API
- `discover.rs` — USB device enumeration via `nusb`
- `introspect.rs` — detect device capabilities (vendor ID, product ID, device class)
- `registry.rs` — JSON persistence at `~/.zeptoclaw/hardware/devices.json`, track known devices

### Peripheral layer (`src/peripherals/`)
- `traits.rs` — `Peripheral` trait: `connect()`, `disconnect()`, `send_command()`, `read_data()`, `status()`
- `serial.rs` — async serial port via `tokio-serial`
- `arduino.rs` — Arduino flash + sketch upload (uses serial)
- `nucleo.rs` — STM32/Nucleo flashing
- `rpi.rs` — Raspberry Pi GPIO via `rppal` (feature-gated: `peripheral-rpi`, Linux-only)

### Agent tool
- `src/tools/hardware.rs` — `HardwareTool` exposing `list_devices`, `connect`, `send_command`, `read_data` to the agent. Feature-gated behind `hardware`

### CLI commands
- `zeptoclaw hardware list`, `zeptoclaw hardware info <device>`

### Feature flags
- `hardware` — gates `nusb` + `tokio-serial` + base peripheral support
- `peripheral-rpi` — gates `rppal` (Linux ARM only)
- `probe` — gates `probe-rs` for STM32 memory inspection (heavy, ~50 deps)

### Without feature
Tool registered but returns "requires hardware build feature" (same pattern as PDF and email)

**New deps (all feature-gated):**
- `nusb` (hardware)
- `tokio-serial` (hardware)
- `rppal` (peripheral-rpi)
- `probe-rs` (probe)

**Tests:** USB discovery mock, serial frame parsing, device registry CRUD, trait compliance, feature-absent error (~20 tests)

---

## Implementation Order

Dependencies and effort guide the sequence:

```
1. Response cache         — new module, zero deps, 1 day
2. Autonomy levels        — new file, wires into agent loop, 1 day
3. Device pairing         — new file + gateway middleware + CLI, 2 days
4. Hardware support       — new directories + feature flags + tool, 3 days
```

Items 1–2 are independent. Item 3 is independent but should come before 4 (pairing secures the gateway before hardware connects to it). Item 4 depends on understanding existing tool/channel patterns.

---

## What We Are Explicitly NOT Adding

| Item | Reason |
|---|---|
| Enterprise audit logging | YAGNI until multi-tenant |
| Action rate limiting (max_actions/hour) | YAGNI for personal use |
| Sandbox backends (Landlock/Bubblewrap/Firejail) | Linux-only; Docker + Apple Containers already cover this |
| SQLite/PostgreSQL memory backends | Pluggable arch — users opt in |
| Full RBAC (roles, permissions) | Autonomy levels are sufficient for now |

---

## Binary Size Impact

| Item | Estimated impact |
|---|---|
| Response cache | ~0 (code only, no new deps) |
| Autonomy levels | ~0 (code only) |
| Device pairing | ~0 (code only, sha2 already in dep tree) |
| Hardware (base) | 0 default / ~200KB with `--features hardware` |
| RPi GPIO | 0 default / ~100KB with `--features peripheral-rpi` |
| probe-rs | 0 default / ~2MB with `--features probe` |

Default binary stays lean. Feature-flagged additions are opt-in.
