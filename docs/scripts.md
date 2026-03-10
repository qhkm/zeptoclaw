# Containerized Devtools

## Quick Setup

**docker/podman** installed.

**Build dev image** (one-time, pre-clippy/nextest ~2min):
```
**Docker:** `docker build -f Dockerfile.dev -t zeptodev .`
**Podman:** `podman build -f Dockerfile.dev -t localhost/zeptodev .`  # localhost/ avoids short-name error
```
Fallback: rust:1.88-slim (rustup install each run ~20s).

**Podman** (macOS/WSL):
```
podman machine init
podman machine start
```

**Volumes** (podman only, pre-run once):
```
podman volume create zeptoclaw-target_cache zeptoclaw-registry_cache zeptoclaw-benches_cache zeptoclaw-sccache
```

**sccache** opt (faster compiles):
Host: `curl -sSf https://raw.githubusercontent.com/mozilla/sccache/master/install.sh | sh`
Env: `export RUSTC_WRAPPER=sccache` (add to ~/.bashrc)

## Usage

```
# Tests
./scripts/test-container.sh lib                    # cargo nextest --lib
./scripts/test-container.sh integration            # cargo test
./scripts/test-container.sh all extra args...      # nextest lib + test + passthru

# Lint (AGENTS.md gates)
./scripts/lint-container.sh                        # clippy/fmt/doc

# Bench
./scripts/bench-container.sh                       # cargo bench message_bus --no-run

# Podman
CONTAINER_RUNTIME=podman ./scripts/test-container.sh lib

# sccache
./scripts/test-container.sh lib --sccache
```

**Dry-run:** Scripts print docker/podman cmd if `echo DRY_RUN=1 ./scripts/...`

## Cache Prune

**Docker:** `docker builder prune --filter type=cache`

**Podman:** `podman volume prune` (or `podman volume rm zeptoclaw-*`)

## Pre-Push Workflow (Recommended)

./scripts/lint-container.sh
./scripts/test-container.sh lib
./scripts/bench-container.sh

Add to .git/hooks/pre-push for auto.

## Troubleshooting

- Slow first run: Image pull/deps.
- Podman vols missing: Create above.
- sccache miss: Host env set?
- Integrity: `cargo clean` + prune if stale.