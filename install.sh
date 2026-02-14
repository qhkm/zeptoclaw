#!/bin/sh
set -eu

REPO="qhkm/zeptoclaw"
INSTALL_DIR="/usr/local/bin"
BINARY="zeptoclaw"

# Detect OS
OS="$(uname -s)"
case "$OS" in
  Linux)  OS_LABEL="linux" ;;
  Darwin) OS_LABEL="macos" ;;
  *)      echo "Error: Unsupported OS: $OS"; exit 1 ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)  ARCH_LABEL="x86_64" ;;
  aarch64|arm64)  ARCH_LABEL="aarch64" ;;
  *)              echo "Error: Unsupported architecture: $ARCH"; exit 1 ;;
esac

ARTIFACT="${BINARY}-${OS_LABEL}-${ARCH_LABEL}"
BASE_URL="https://github.com/${REPO}/releases/latest/download"

echo "Installing ZeptoClaw (${OS_LABEL}/${ARCH_LABEL})..."

# Create temp directory
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

# Download binary and checksum
echo "Downloading ${ARTIFACT}..."
curl -fsSL "${BASE_URL}/${ARTIFACT}" -o "${TMP_DIR}/${BINARY}"
curl -fsSL "${BASE_URL}/${ARTIFACT}.sha256" -o "${TMP_DIR}/${BINARY}.sha256"

# Verify checksum
echo "Verifying checksum..."
cd "$TMP_DIR"
if command -v sha256sum >/dev/null 2>&1; then
  echo "$(cat ${BINARY}.sha256)" | sha256sum -c - >/dev/null 2>&1
elif command -v shasum >/dev/null 2>&1; then
  EXPECTED="$(awk '{print $1}' ${BINARY}.sha256)"
  ACTUAL="$(shasum -a 256 ${BINARY} | awk '{print $1}')"
  if [ "$EXPECTED" != "$ACTUAL" ]; then
    echo "Error: Checksum verification failed"
    exit 1
  fi
else
  echo "Warning: No checksum tool found, skipping verification"
fi

# Install
chmod +x "${TMP_DIR}/${BINARY}"
if [ -w "$INSTALL_DIR" ]; then
  mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
  echo "Installing to ${INSTALL_DIR} (requires sudo)..."
  sudo mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

echo ""
echo "ZeptoClaw installed successfully!"
echo ""
echo "Get started:"
echo "  zeptoclaw onboard          # Interactive setup"
echo "  zeptoclaw agent -m 'Hello' # Talk to your agent"
echo ""
echo "Docs: https://github.com/${REPO}"
