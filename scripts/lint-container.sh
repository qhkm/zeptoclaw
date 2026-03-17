#!/bin/bash
set -euo pipefail

source "$(dirname "$0")/container-base.sh"

SCRIPT_DIR="$(dirname "$0")"
# Check if stdin is a TTY and set flags accordingly
TTY_FLAG=""
if [ -t 0 ] && [ -t 1 ]; then
    TTY_FLAG="-it"
else
    TTY_FLAG="-i"
fi

# Auto-build zeptodev if missing (docker: zeptodev, podman: localhost/zeptodev)
IMAGE_TAG="zeptodev"
if [[ "$RUNTIME" == "podman" ]]; then
    IMAGE_TAG="localhost/${IMAGE_TAG}:custom"

    if ! $RUNTIME image inspect "$IMAGE_TAG" >/dev/null 2>&1; then
      echo "Building $IMAGE_TAG first-run image (Dockerfile.dev)..."

      buildah bud \
      --userns=host \
      -f Dockerfile.dev \
      -t "${IMAGE_TAG}" .
    fi
fi

# Use pre-built
ORIGINAL_IMAGE="$IMAGE"
IMAGE="$IMAGE_TAG"
trap 'IMAGE="$ORIGINAL_IMAGE"' EXIT

container_run "cargo clippy --all-targets --config /clippy.toml -- -D warnings && cargo fmt --all -- --check && cargo test --doc"
