#!/bin/sh
set -eu

# ZeptoClaw Universal VPS Setup Script
# Usage: curl -fsSL https://zeptoclaw.com/setup.sh | sh
#    or: curl -fsSL https://zeptoclaw.com/setup.sh | sh -s -- --docker
#    or: bash deploy/setup.sh --help

# ─── Constants ────────────────────────────────────────────────────────────────

REPO="qhkm/zeptoclaw"
BINARY="zeptoclaw"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="${HOME}/.zeptoclaw"
SERVICE_NAME="zeptoclaw"
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
DOCKER_IMAGE="ghcr.io/qhkm/zeptoclaw:latest"
CONTAINER_NAME="zeptoclaw"

# ─── Colors (only if terminal) ───────────────────────────────────────────────

if [ -t 1 ]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[1;33m'
  BLUE='\033[0;34m'
  BOLD='\033[1m'
  RESET='\033[0m'
else
  RED=''
  GREEN=''
  YELLOW=''
  BLUE=''
  BOLD=''
  RESET=''
fi

# ─── Helpers ──────────────────────────────────────────────────────────────────

info()  { printf "${BLUE}[info]${RESET}  %s\n" "$1"; }
ok()    { printf "${GREEN}[ok]${RESET}    %s\n" "$1"; }
warn()  { printf "${YELLOW}[warn]${RESET}  %s\n" "$1"; }
err()   { printf "${RED}[error]${RESET} %s\n" "$1" >&2; }
die()   { err "$1"; exit 1; }

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    die "Required command not found: $1"
  fi
}

need_sudo() {
  if [ "$(id -u)" -ne 0 ]; then
    if ! command -v sudo >/dev/null 2>&1; then
      die "This operation requires root. Please run as root or install sudo."
    fi
    SUDO="sudo"
  else
    SUDO=""
  fi
}

# ─── Usage ────────────────────────────────────────────────────────────────────

usage() {
  cat <<EOF
${BOLD}ZeptoClaw VPS Setup${RESET}

Usage:
  curl -fsSL https://zeptoclaw.com/setup.sh | sh
  curl -fsSL https://zeptoclaw.com/setup.sh | sh -s -- --docker
  bash deploy/setup.sh [OPTIONS]

Options:
  --docker      Use Docker instead of native binary
  --uninstall   Remove ZeptoClaw from this system
  --help        Show this help message

Examples:
  # Install binary
  curl -fsSL https://zeptoclaw.com/setup.sh | sh

  # Docker mode
  curl -fsSL https://zeptoclaw.com/setup.sh | sh -s -- --docker

  # Uninstall
  bash deploy/setup.sh --uninstall
EOF
}

# ─── System checks ───────────────────────────────────────────────────────────

check_system() {
  OS="$(uname -s)"
  case "$OS" in
    Linux) ;;
    *) die "This setup script is for Linux only. Detected: $OS" ;;
  esac

  ARCH="$(uname -m)"
  case "$ARCH" in
    x86_64|amd64)   ARCH_LABEL="x86_64" ;;
    aarch64|arm64)   ARCH_LABEL="aarch64" ;;
    *) die "Unsupported architecture: $ARCH" ;;
  esac

  # Detect distro family
  DISTRO="unknown"
  if [ -f /etc/os-release ]; then
    . /etc/os-release
    case "${ID:-}" in
      ubuntu|debian|pop|linuxmint|elementary|zorin)
        DISTRO="debian"
        ;;
      rhel|centos|fedora|rocky|alma|amzn|ol)
        DISTRO="rhel"
        ;;
      *)
        # Check ID_LIKE as fallback
        case "${ID_LIKE:-}" in
          *debian*|*ubuntu*) DISTRO="debian" ;;
          *rhel*|*fedora*|*centos*) DISTRO="rhel" ;;
        esac
        ;;
    esac
  fi

  if [ "$DISTRO" = "unknown" ]; then
    warn "Could not detect Linux distribution. Proceeding anyway."
  fi

  info "System: Linux/${ARCH_LABEL} (${DISTRO})"

  # Check for required commands
  need_cmd curl
}

# ─── Uninstall ───────────────────────────────────────────────────────────────

do_uninstall() {
  info "Uninstalling ZeptoClaw..."
  need_sudo

  # Stop and disable systemd service
  if [ -f "$SERVICE_FILE" ]; then
    info "Stopping systemd service..."
    $SUDO systemctl stop "$SERVICE_NAME" 2>/dev/null || true
    $SUDO systemctl disable "$SERVICE_NAME" 2>/dev/null || true
    $SUDO rm -f "$SERVICE_FILE"
    $SUDO systemctl daemon-reload 2>/dev/null || true
    ok "Systemd service removed"
  fi

  # Remove binary
  if [ -f "${INSTALL_DIR}/${BINARY}" ]; then
    $SUDO rm -f "${INSTALL_DIR}/${BINARY}"
    ok "Binary removed from ${INSTALL_DIR}"
  fi

  # Stop and remove Docker container
  if command -v docker >/dev/null 2>&1; then
    if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -q "^${CONTAINER_NAME}$"; then
      info "Stopping Docker container..."
      docker stop "$CONTAINER_NAME" 2>/dev/null || true
      docker rm "$CONTAINER_NAME" 2>/dev/null || true
      ok "Docker container removed"
    fi
    # Remove image
    if docker images --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | grep -q "^${DOCKER_IMAGE}$"; then
      docker rmi "$DOCKER_IMAGE" 2>/dev/null || true
      ok "Docker image removed"
    fi
  fi

  # Remove config directory
  if [ -d "$CONFIG_DIR" ]; then
    info "Config directory preserved at ${CONFIG_DIR}"
    info "Remove manually with: rm -rf ${CONFIG_DIR}"
  fi

  ok "ZeptoClaw has been uninstalled"
  exit 0
}

# ─── Binary install ──────────────────────────────────────────────────────────

install_binary() {
  ARTIFACT="${BINARY}-linux-${ARCH_LABEL}"
  BASE_URL="https://github.com/${REPO}/releases/latest/download"

  need_sudo

  # Check for pre-existing install
  if [ -f "${INSTALL_DIR}/${BINARY}" ]; then
    warn "Existing installation found at ${INSTALL_DIR}/${BINARY}"
    EXISTING_VERSION="$(${INSTALL_DIR}/${BINARY} --version 2>/dev/null || echo 'unknown')"
    info "Existing version: ${EXISTING_VERSION}"
    info "Upgrading to latest..."
  fi

  # Create temp directory
  TMP_DIR="$(mktemp -d)"
  trap 'rm -rf "$TMP_DIR"' EXIT

  info "Downloading ${ARTIFACT}..."
  curl -fsSL "${BASE_URL}/${ARTIFACT}" -o "${TMP_DIR}/${BINARY}" || \
    die "Failed to download binary. Check your internet connection and that releases exist at ${BASE_URL}/${ARTIFACT}"
  curl -fsSL "${BASE_URL}/${ARTIFACT}.sha256" -o "${TMP_DIR}/${BINARY}.sha256" || \
    die "Failed to download checksum file"

  # Verify checksum
  info "Verifying SHA256 checksum..."
  cd "$TMP_DIR"
  EXPECTED="$(awk '{print $1}' "${BINARY}.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "${BINARY}" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    ACTUAL="$(shasum -a 256 "${BINARY}" | awk '{print $1}')"
  else
    warn "No checksum tool found (sha256sum or shasum). Skipping verification."
    ACTUAL="$EXPECTED"
  fi

  if [ "$EXPECTED" != "$ACTUAL" ]; then
    die "Checksum verification failed!\n  Expected: ${EXPECTED}\n  Actual:   ${ACTUAL}"
  fi
  ok "Checksum verified"

  # Install binary
  chmod +x "${TMP_DIR}/${BINARY}"
  if [ -w "$INSTALL_DIR" ]; then
    mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
  else
    $SUDO mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
  fi
  ok "Binary installed to ${INSTALL_DIR}/${BINARY}"
}

# ─── Docker install ──────────────────────────────────────────────────────────

install_docker() {
  # Install Docker if not present
  if ! command -v docker >/dev/null 2>&1; then
    info "Docker not found. Installing Docker CE..."
    need_sudo
    curl -fsSL https://get.docker.com | $SUDO sh || \
      die "Failed to install Docker. Please install it manually: https://docs.docker.com/engine/install/"
    # Add current user to docker group if not root
    if [ "$(id -u)" -ne 0 ]; then
      $SUDO usermod -aG docker "$(whoami)" 2>/dev/null || true
      warn "You may need to log out and back in for Docker group membership to take effect."
    fi
    ok "Docker installed"
  else
    ok "Docker already installed"
  fi

  # Verify Docker is running
  if ! docker info >/dev/null 2>&1; then
    need_sudo
    $SUDO systemctl start docker 2>/dev/null || \
      $SUDO service docker start 2>/dev/null || \
      die "Docker is installed but not running. Please start Docker and re-run this script."
  fi

  # Pull image
  info "Pulling ${DOCKER_IMAGE}..."
  docker pull "$DOCKER_IMAGE" || \
    die "Failed to pull Docker image. Check your internet connection."
  ok "Docker image pulled"
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
  MODE="binary"

  # Parse arguments
  while [ $# -gt 0 ]; do
    case "$1" in
      --docker)
        MODE="docker"
        shift
        ;;
      --uninstall)
        check_system
        do_uninstall
        ;;
      --help|-h)
        usage
        exit 0
        ;;
      *)
        die "Unknown option: $1 (see --help)"
        ;;
    esac
  done

  printf "\n${BOLD}ZeptoClaw Setup${RESET} (%s mode)\n\n" "$MODE"

  check_system

  # Install
  if [ "$MODE" = "docker" ]; then
    install_docker
  else
    install_binary
  fi

  # Print next step
  printf "\n${BOLD}=== Installation Complete ===${RESET}\n\n"

  if [ "$MODE" = "docker" ]; then
    VERSION="$(docker run --rm "$DOCKER_IMAGE" zeptoclaw --version 2>/dev/null || echo 'installed')"
    ok "ZeptoClaw ${VERSION}"
    printf "\n${BOLD}Next step:${RESET}\n"
    printf "  docker run --rm -it -v %s:/data %s zeptoclaw onboard\n\n" "$CONFIG_DIR" "$DOCKER_IMAGE"
  else
    VERSION="$(${INSTALL_DIR}/${BINARY} --version 2>/dev/null || echo 'installed')"
    ok "ZeptoClaw ${VERSION}"
    printf "\n${BOLD}Next step:${RESET}\n"
    printf "  zeptoclaw onboard\n\n"
  fi
}

main "$@"
