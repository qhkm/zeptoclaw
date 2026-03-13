#!/bin/bash
set -euo pipefail

# Base library for containerized devtools (design + Dockerfile.tests deps note)
# Usage: source scripts/container-base.sh ; container_run "cargo nextest --lib"
# Opt: --sccache (mount ~/.cache/sccache:/root/.cache/sccache:rw, set RUSTC_WRAPPER=sccache host-side)
# Podman default/fallback docker. Podman vols: podman volume create zeptoclaw-{target,registry,benches}_cache

IMAGE="${IMAGE:-docker.io/library/rust:1.88-slim}"  # Pre-qualified fallback. Custom: docker build -t zeptodev Dockerfile.dev (podman: -t localhost/zeptodev)
WORKDIR="/src"

# Podman preferred, fallback docker
if command -v podman >/dev/null 2>&1; then
  RUNTIME="podman"
  TARGET_MOUNT="-v zeptoclaw-target_cache:/src/target:rw"
  REGISTRY_MOUNT="-v zeptoclaw-registry_cache:/src/Cargo-registry:rw"
  BENCH_MOUNT="-v zeptoclaw-benches_cache:/src/benches/target:rw"
elif command -v docker >/dev/null 2>&1; then
  RUNTIME="docker"
  TARGET_MOUNT="--mount type=cache,target=/src/target"
  REGISTRY_MOUNT="--mount type=cache,target=/src/Cargo-registry"
  BENCH_MOUNT="--mount type=cache,target=/src/benches/target"
else
  echo "Neither podman nor docker found. Install podman (preferred) or docker." >&2
  exit 1
fi

CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-$RUNTIME}"

sccache_mount=""
if [[ "${1:-}" == "--sccache" ]]; then
  shift
  sccache_mount="-v ~/.cache/sccache:/root/.cache/sccache:rw"  # Host sccache (user: curl install.sh; export RUSTC_WRAPPER=sccache)
fi

container_run() {
  local cmd=("$@")
  $RUNTIME run --rm -it \
    -w "$WORKDIR" \
    -v "$(pwd)":/src:rw \
    $TARGET_MOUNT $REGISTRY_MOUNT $BENCH_MOUNT $sccache_mount \
    $IMAGE \
    bash -c "${cmd[*]}"
}

container_run "$@"