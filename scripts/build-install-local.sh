#!/bin/sh
# Build and install a local ZeptoClaw release binary for command-line use.
#
# Usage:
#   ./scripts/build-install-local.sh
#   INSTALL_DIR="$HOME/.local/bin" ./scripts/build-install-local.sh
#
# Default install location:
#   ~/.local/bin/zeptoclaw
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARY_PATH="${REPO_ROOT}/target/release/zeptoclaw"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
INSTALL_PATH="${INSTALL_DIR}/zeptoclaw"

echo "Building local release from: ${REPO_ROOT}"
cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"

if [ ! -x "${BINARY_PATH}" ]; then
  echo "Error: build completed but binary not found at ${BINARY_PATH}"
  exit 1
fi

mkdir -p "${INSTALL_DIR}"
cp "${BINARY_PATH}" "${INSTALL_PATH}"
chmod +x "${INSTALL_PATH}"

echo ""
echo "Installed: ${INSTALL_PATH}"
"${INSTALL_PATH}" --version

if ! command -v zeptoclaw >/dev/null 2>&1; then
  echo ""
  echo "Note: zeptoclaw is not currently on PATH for this shell."
  echo "Add this to your shell profile:"
  echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
fi
