#!/bin/bash
set -euo pipefail

source "$(dirname "$0")/container-base.sh"

# Auto-build zeptodev if missing (docker: zeptodev, podman: localhost/zeptodev)
IMAGE_TAG="zeptodev"
if [[ "$RUNTIME" == "podman" ]]; then
  IMAGE_TAG="localhost/zeptodev"
fi

if ! $RUNTIME image inspect "$IMAGE_TAG" >/dev/null 2>&1; then
  echo "Building $IMAGE_TAG first-run image (Dockerfile.dev)..."
  $RUNTIME build -f ../Dockerfile.dev -t "$IMAGE_TAG" --quiet
fi

# Use pre-built
ORIGINAL_IMAGE="$IMAGE"
IMAGE="$IMAGE_TAG"
trap 'IMAGE="$ORIGINAL_IMAGE"' EXIT

container_run "cargo clippy --all-targets --config /clippy.toml -- -D warnings && cargo fmt --all -- --check && cargo test --doc"