#!/usr/bin/env bash
# Build ZeptoClaw inside a container, running clippy + fmt check.
# Supports Docker (BuildKit) and Podman (buildah).
#
# Usage:
#   ./scripts/lint-container.sh              # build with cache mounts
#   ./scripts/lint-container.sh --no-cache   # build without cache
#   ./scripts/lint-container.sh --fallback   # force mount-free fallback
#   CONTAINER_ENGINE=podman ./scripts/lint-container.sh  # force engine
set -euo pipefail

IMAGE="zeptoclaw:dev-lint"
DOCKERFILE="Dockerfile.dev"
FALLBACK_DOCKERFILE=""
NO_CACHE=""

for arg in "$@"; do
  case "$arg" in
    --no-cache) NO_CACHE="--no-cache" ;;
    --fallback) FALLBACK_DOCKERFILE="yes" ;;
    *) echo "Unknown option: $arg" >&2; exit 1 ;;
  esac
done

# ── Detect container engine ──────────────────────────────────────────────────
# Set CONTAINER_ENGINE=podman (or docker) to skip auto-detection.

detect_engine() {
  if [ -n "${CONTAINER_ENGINE:-}" ]; then
    case "$CONTAINER_ENGINE" in
      docker|podman) ;;
      *)
        echo "Error: CONTAINER_ENGINE must be 'docker' or 'podman'" >&2
        exit 1
        ;;
    esac
    if ! command -v "$CONTAINER_ENGINE" &>/dev/null; then
      echo "Error: CONTAINER_ENGINE=$CONTAINER_ENGINE not found" >&2
      exit 1
    fi
    echo "$CONTAINER_ENGINE"
    return
  fi

  # Try docker first (most common), but detect Podman-as-Docker (#435).
  # Podman-as-Docker breaks buildx's docker-container driver (#424).
  if command -v docker &>/dev/null && docker info &>/dev/null 2>&1; then
    if docker info 2>/dev/null | grep -qi podman; then
      # Docker CLI is actually a Podman alias — use podman directly
      if command -v podman &>/dev/null; then
        echo "podman"
        return
      fi
    fi
    echo "docker"
    return
  fi

  if command -v podman &>/dev/null; then
    echo "podman"
    return
  fi

  echo "Error: neither docker nor podman found" >&2
  exit 1
}

ENGINE=$(detect_engine)

echo "Engine: $ENGINE"

# ── Check BuildKit / buildah mount support ───────────────────────────────────

supports_cache_mount() {
  if [ "$ENGINE" = "docker" ]; then
    # Docker with BuildKit supports --mount=type=cache
    return 0
  fi

  # Podman: buildah >= 1.28 / podman >= 4.1 supports --mount=type=cache
  local ver
  ver=$(podman version --format '{{.Client.Version}}' 2>/dev/null || echo "0.0.0")
  local major minor
  major=$(echo "$ver" | cut -d. -f1)
  minor=$(echo "$ver" | cut -d. -f2)
  if [ "$major" -gt 4 ] || { [ "$major" -eq 4 ] && [ "$minor" -ge 1 ]; }; then
    return 0
  fi

  echo "Podman $ver does not support --mount=type=cache (requires >= 4.1)" >&2
  return 1
}

# ── Generate mount-free fallback Dockerfile ──────────────────────────────────

generate_fallback() {
  FALLBACK_DOCKERFILE_PATH=$(mktemp "${TMPDIR:-/tmp}/Dockerfile.dev-fallback.XXXXXX")
  # Strip --mount flags from RUN lines
  sed 's/--mount=type=cache,[^ ]* *//g; /^# syntax=/d' "$DOCKERFILE" > "$FALLBACK_DOCKERFILE_PATH"
  echo "$FALLBACK_DOCKERFILE_PATH"
}

# ── Build ────────────────────────────────────────────────────────────────────

build_args=()

if [ -n "$FALLBACK_DOCKERFILE" ] || ! supports_cache_mount; then
  echo "Using mount-free fallback Dockerfile"
  fb=$(generate_fallback)
  build_args+=(-f "$fb")
  trap 'rm -f "$fb"' EXIT
else
  build_args+=(-f "$DOCKERFILE")
  if [ "$ENGINE" = "docker" ]; then
    export DOCKER_BUILDKIT=1
  fi
fi

if [ -n "$NO_CACHE" ]; then
  build_args+=("$NO_CACHE")
fi

build_args+=(-t "$IMAGE" .)

echo "Running: $ENGINE build ${build_args[*]}"
$ENGINE build "${build_args[@]}"

echo ""
echo "Container lint passed."
