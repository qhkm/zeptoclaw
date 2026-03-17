# Containerized Devtools (Podman Rootless)

## Quick Setup

**Podman** rootless (docker dropped).

**Admin (one-time)**: `usermod --add-subuids 100000-165535 --add-subgids 100000-165535 $USER; podman system migrate`

**Volumes** (pre-run):
```
printf '%s\n' zeptoclaw-{target,registry,benches,sccache}_cache | xargs -I {} podman volume create {}
```

**sccache** opt:
Host: `curl -sSf https://raw.githubusercontent.com/mozilla/sccache/master/install.sh | sh`
Env: `export RUSTC_WRAPPER=sccache && source ~/.bashrc`

**Build dev image** (~2min first):
```
podman build --userns=keep-id -f Dockerfile.dev -t localhost/zeptodev .
# Rootless tutorial: keep-id maps host uid:gid 1:1 (no perms)
```

## Usage
(all scripts auto --userns=keep-id)

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
