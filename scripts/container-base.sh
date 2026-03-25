#!/bin/bash
set -euo pipefail
set -x
# Base library for containerized devtools (design + Dockerfile.tests deps note)
# Usage: source scripts/container-base.sh ; container_run "cargo nextest --lib"
# Podman default/fallback docker. Podman vols: podman volume create zeptoclaw-{target,registry,benches}_cache

# Pre-qualified fallback. Custom: docker build -t zeptodev Dockerfile.dev (podman: -t localhost/zeptodev)
IMAGE="${IMAGE:-docker.io/library/rust:1.93-slim}"  
WORKDIR="/src"

# Podman preferred, fallback docker
if ! command -v podman >/dev/null 2>&1; then
  echo "Podman required for rootless containers (see rootless tutorial)." >&2
  exit 1
fi
RUNTIME="podman"
TARGET_MOUNT="-v zeptoclaw-target_cache:/src/target:rw,U"
REGISTRY_MOUNT="-v zeptoclaw-registry_cache:/cargo-home:rw,U"
BENCH_MOUNT="-v zeptoclaw-benches_cache:/src/benches/target:rw,U"

CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-$RUNTIME}"

SCCACHE_MOUNT="-v $HOME/.cache/sccache:/root/.cache/sccache:rw,U"  # Host sccache (user: curl install.sh; export RUSTC_WRAPPER=sccache)

# Check if stdin is a TTY and set flags accordingly
TTY_FLAG=""
if [ -t 0 ] && [ -t 1 ]; then
    TTY_FLAG="-it"
else
    TTY_FLAG="-i"
fi

printf "%s\n" target registry benches | xargs -I{} podman volume create --ignore zeptoclaw-{}_cache &>/dev/null

container_run() {
  # Override CARGO_HOME and mount for rootless Podman (avoids /usr/local/cargo root perms)
  local cmd=("$@")
  $CONTAINER_RUNTIME run --userns=keep-id --rm $TTY_FLAG \
    -w "$WORKDIR" \
    -v "$(pwd):/src:Z" \
    -v "$(pwd)/scripts/artifacts/clippy.toml:/clippy.toml:Z" \
    -e CARGO_HOME=/cargo-home \
    $TARGET_MOUNT $REGISTRY_MOUNT $BENCH_MOUNT $SCCACHE_MOUNT \
    $IMAGE \
    bash -c "${cmd[*]}"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    container_run "$@"
fi
