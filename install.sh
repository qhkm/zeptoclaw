#!/bin/sh
set -eu

REPO="qhkm/zeptoclaw"
BINARY="zeptoclaw"

# --- Detect platform ---

OS="$(uname -s)"
case "$OS" in
  Linux)  OS_LABEL="linux" ;;
  Darwin) OS_LABEL="macos" ;;
  *)      echo "Error: Unsupported OS: $OS"; exit 1 ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)  ARCH_LABEL="x86_64" ;;
  aarch64|arm64)  ARCH_LABEL="aarch64" ;;
  *)              echo "Error: Unsupported architecture: $ARCH"; exit 1 ;;
esac

ARTIFACT="${BINARY}-${OS_LABEL}-${ARCH_LABEL}"

# --- Resolve version ---

if [ -n "${ZEPTOCLAW_VERSION:-}" ]; then
  VERSION="$ZEPTOCLAW_VERSION"
  BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
else
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
  VERSION="latest"
fi

# --- Pick install directory ---
# Prefer ~/.local/bin (no sudo), fall back to /usr/local/bin.

if [ -n "${ZEPTOCLAW_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$ZEPTOCLAW_INSTALL_DIR"
elif [ -d "$HOME/.local/bin" ] || mkdir -p "$HOME/.local/bin" 2>/dev/null; then
  INSTALL_DIR="$HOME/.local/bin"
else
  INSTALL_DIR="/usr/local/bin"
fi

# --- Hardware support note ---

HW_NOTE=""
if [ "$OS_LABEL" = "linux" ] && [ "$ARCH_LABEL" = "aarch64" ]; then
  HW_NOTE=" (includes ESP32 + Raspberry Pi hardware support)"
else
  HW_NOTE=" (includes ESP32 hardware support)"
fi

echo "Installing ZeptoClaw ${VERSION} for ${OS_LABEL}/${ARCH_LABEL}${HW_NOTE}..."

# --- Download ---

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading ${ARTIFACT}..."
curl -fsSL "${BASE_URL}/${ARTIFACT}" -o "${TMP_DIR}/${BINARY}"
curl -fsSL "${BASE_URL}/${ARTIFACT}.sha256" -o "${TMP_DIR}/${BINARY}.sha256"

# --- Verify checksum ---

echo "Verifying checksum..."
cd "$TMP_DIR"
EXPECTED="$(awk '{print $1}' "${BINARY}.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "${BINARY}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL="$(shasum -a 256 "${BINARY}" | awk '{print $1}')"
else
  echo "Warning: No checksum tool found, skipping verification"
  ACTUAL="$EXPECTED"
fi
if [ "$EXPECTED" != "$ACTUAL" ]; then
  echo "Error: Checksum verification failed!"
  echo "  Expected: $EXPECTED"
  echo "  Actual:   $ACTUAL"
  exit 1
fi
echo "Checksum OK."

# --- Install binary ---

chmod +x "${TMP_DIR}/${BINARY}"
if [ -w "$INSTALL_DIR" ]; then
  mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
  echo "Installing to ${INSTALL_DIR} (requires sudo)..."
  sudo mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

# --- Verify installation ---

INSTALLED_VERSION=""
if command -v "${BINARY}" >/dev/null 2>&1; then
  INSTALLED_VERSION="$("${BINARY}" --version 2>/dev/null || true)"
fi

# --- PATH check ---

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo ""
    echo "Warning: ${INSTALL_DIR} is not in your PATH."
    echo "Add it with:"
    echo ""
    if [ -n "${ZSH_VERSION:-}" ] || [ "$(basename "${SHELL:-}")" = "zsh" ]; then
      echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc && source ~/.zshrc"
    else
      echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc && source ~/.bashrc"
    fi
    ;;
esac

# --- Done ---

echo ""
echo "ZeptoClaw installed successfully! ${INSTALLED_VERSION}"
echo ""
echo "Get started:"
echo "  zeptoclaw onboard          # Interactive setup"
echo "  zeptoclaw agent -m 'Hello' # Talk to your agent"
echo ""
echo "Update later:  zeptoclaw update"
echo "Docs:          https://github.com/${REPO}"
