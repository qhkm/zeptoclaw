#!/bin/sh
# Build a local ZeptoClaw release binary for testing current branch changes.
#
# Usage:
#   ./scripts/build-local-release.sh
#
# Output:
#   target/release/zeptoclaw
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARY_PATH="${REPO_ROOT}/target/release/zeptoclaw"

echo "Building local release from: ${REPO_ROOT}"
cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"

if [ ! -x "${BINARY_PATH}" ]; then
  echo "Error: build completed but binary not found at ${BINARY_PATH}"
  exit 1
fi

echo ""
echo "Local release ready: ${BINARY_PATH}"
echo "Version check:"
"${BINARY_PATH}" --version
