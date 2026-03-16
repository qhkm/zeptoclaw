#!/bin/bash
set -euo pipefail

source "$(dirname "$0")/container-base.sh"

MODE="${1:-all}"
shift || true

case "$MODE" in
  lib)
    container_run "cargo install cargo-nextest --locked || true && cargo nextest run --lib" "$@"
    ;;
  integration)
    container_run "cargo test" "$@"
    ;;
  all)
    container_run "cargo install --locked cargo-nextest || true && cargo nextest run --lib && cargo test" "$@"
    ;;
  *)
    echo "Usage: $0 [lib|integration|all] [cargo args...]" >&2
    exit 1
    ;;
esac