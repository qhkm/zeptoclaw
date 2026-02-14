# Pre-Launch Checklist Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Prepare ZeptoClaw for its first public GitHub release with professional docs, install infrastructure, and a tagged v0.2.0 release.

**Architecture:** Create 3 missing docs (CHANGELOG, SECURITY, CONTRIBUTING), an install.sh curl script, a Homebrew formula, update Cargo.toml metadata, update README with badges and install methods, then tag v0.2.0.

**Tech Stack:** Markdown, Bash (install script), Ruby (Homebrew formula), TOML (Cargo.toml)

---

### Task 1: Create CHANGELOG.md

**Files:**
- Create: `CHANGELOG.md`

**Step 1: Write CHANGELOG.md**

Create `CHANGELOG.md` at project root following [Keep a Changelog](https://keepachangelog.com/) format. Single release entry for v0.2.0 (first public release).

```markdown
# Changelog

All notable changes to ZeptoClaw will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-02-14

First public release.

### Added
- **Streaming responses** — Token-by-token SSE streaming for Claude and OpenAI providers (`--stream` flag)
- **Agent swarms** — DelegateTool creates specialist sub-agents with role-specific system prompts and tool whitelists
- **Plugin system** — JSON manifest-based plugin discovery and registration with PluginTool adapter
- **Agent templates** — Pre-configured agent profiles (coder, researcher, etc.) with `--template` flag
- **4 channels** — Telegram, Slack (outbound), Discord (Gateway WebSocket + REST), Webhook (HTTP POST inbound)
- **Batch mode** — Process multiple prompts from text/JSONL files with `batch` CLI command
- **Conversation history** — CLI commands to list, search, and clean up past sessions
- **Long-term memory** — Persistent key-value store with categories, tags, and keyword search
- **Token budget** — Per-session token budget tracking with atomic counters
- **Structured output** — JSON and JSON Schema output format support for OpenAI and Claude
- **Tool approval** — Configurable approval gate checked before tool execution
- **Retry provider** — Exponential backoff wrapper for 429/5xx errors
- **Fallback provider** — Automatic primary-to-secondary provider failover
- **Cost tracking** — Per-provider/model cost accumulation with pricing tables for 8 models
- **Telemetry export** — Prometheus text exposition and JSON metrics rendering
- **Hooks system** — Config-driven before_tool, after_tool, on_error hooks with pattern matching
- **17 built-in tools** — shell, filesystem (read/write/list/edit), web search, web fetch, memory, cron, spawn, delegate, WhatsApp, Google Sheets, message, long-term memory, r8r
- **Container isolation** — Native, Docker, and Apple Container runtimes
- **Multi-tenant deployment** — Per-tenant isolation with Docker Compose templates
- **Cross-platform CI/CD** — GitHub Actions for test/lint/fmt, cross-platform release builds (4 targets), Docker image push

### Security
- Shell command blocklist with regex patterns
- Path traversal protection with symlink escape detection
- SSRF prevention with DNS pre-resolution against private IPs
- Workspace-scoped filesystem tools
- Mount allowlist validation
- Cron job caps and spawn recursion prevention

[0.2.0]: https://github.com/qhkm/zeptoclaw/releases/tag/v0.2.0
```

**Step 2: Verify file exists and is well-formed**

Run: `head -5 CHANGELOG.md`
Expected: Shows "# Changelog" header

**Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs: add CHANGELOG.md for v0.2.0 release"
```

---

### Task 2: Create SECURITY.md

**Files:**
- Create: `SECURITY.md`

**Step 1: Write SECURITY.md**

Create `SECURITY.md` at project root.

```markdown
# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.2.x   | :white_check_mark: |
| < 0.2   | :x:                |

## Security Features

ZeptoClaw implements defense-in-depth:

1. **Runtime Isolation** — Configurable Native, Docker, or Apple Container runtimes for shell execution
2. **Containerized Gateway** — Full agent isolation per request with semaphore concurrency limiting
3. **Shell Blocklist** — Regex patterns blocking dangerous commands (rm -rf, reverse shells, etc.)
4. **Path Traversal Protection** — Symlink escape detection, workspace-scoped filesystem tools
5. **SSRF Prevention** — DNS pre-resolution against private IPs, redirect host validation
6. **Input Validation** — URL path injection prevention, spreadsheet ID validation, mount allowlist
7. **Rate Limiting** — Cron job caps (50 active, 60s minimum interval), spawn recursion prevention

See `src/security/` for implementation details.

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly:

1. **Email:** security@kitakod.com
2. **Do not** open a public GitHub issue for security vulnerabilities
3. Include steps to reproduce, affected versions, and potential impact

**Response timeline:**
- Acknowledgment: within 48 hours
- Assessment: within 7 days
- Fix or mitigation: within 30 days for critical issues

## Scope

The following are in scope for security reports:
- Shell command injection bypassing the blocklist
- Path traversal escaping the workspace sandbox
- SSRF bypassing private IP checks
- Container escape vulnerabilities
- Plugin system sandbox bypasses
- Authentication/authorization issues in channels

## Out of Scope

- Vulnerabilities in upstream dependencies (report to the dependency maintainer)
- Issues requiring physical access to the host machine
- Social engineering attacks
```

**Step 2: Verify**

Run: `head -5 SECURITY.md`
Expected: Shows "# Security Policy" header

**Step 3: Commit**

```bash
git add SECURITY.md
git commit -m "docs: add SECURITY.md with vulnerability reporting process"
```

---

### Task 3: Create CONTRIBUTING.md

**Files:**
- Create: `CONTRIBUTING.md`

**Step 1: Write CONTRIBUTING.md**

Create `CONTRIBUTING.md` at project root.

```markdown
# Contributing to ZeptoClaw

Thanks for your interest in contributing! Here's how to get started.

## Quick Start

```bash
# Fork and clone
git clone https://github.com/YOUR_USERNAME/zeptoclaw.git
cd zeptoclaw

# Build
cargo build

# Run tests
cargo test

# Run lints
cargo clippy -- -D warnings
cargo fmt --check
```

## Pull Request Process

1. Create a feature branch from `main`
2. Make your changes with clear, focused commits
3. Ensure all quality gates pass (see below)
4. Open a PR against `main` with a description of what and why

## Quality Gates

Every PR must pass:

```bash
cargo test          # All 1,119+ tests pass
cargo clippy -- -D warnings  # No warnings
cargo fmt --check   # Properly formatted
```

## Commit Messages

Use conventional commits:

- `feat:` — New feature
- `fix:` — Bug fix
- `docs:` — Documentation changes
- `refactor:` — Code restructuring (no behavior change)
- `test:` — Adding or fixing tests
- `chore:` — Build, CI, dependency updates

## Architecture Guide

- **CLAUDE.md** — Full architecture reference, module descriptions, design patterns
- **AGENTS.md** — Coding guidelines, post-implementation checklist, file ownership

## Adding a New Tool

1. Create `src/tools/yourtool.rs`
2. Implement the `Tool` trait with `async fn execute()`
3. Register in `src/tools/mod.rs` and `src/lib.rs`
4. Register in agent setup in `src/cli/agent.rs`
5. Add tests

## Adding a New Channel

1. Create `src/channels/yourchannel.rs`
2. Implement the `Channel` trait
3. Export from `src/channels/mod.rs`
4. Add config struct to `src/config/types.rs`
5. Register in channel factory

## Code of Conduct

This project follows the [Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/) Code of Conduct.
```

**Step 2: Verify**

Run: `head -5 CONTRIBUTING.md`
Expected: Shows "# Contributing to ZeptoClaw" header

**Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING.md with PR process and quality gates"
```

---

### Task 4: Create install.sh

**Files:**
- Create: `install.sh`

**Step 1: Write install.sh**

Create `install.sh` at project root. This is a curl-piped installer that detects OS/arch, downloads the correct binary from GitHub Releases, verifies the SHA256 checksum, and installs to `/usr/local/bin`.

```bash
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
echo "  zeptoclaw onboard     # Interactive setup"
echo "  zeptoclaw agent -m 'Hello'  # Talk to your agent"
echo ""
echo "Docs: https://github.com/${REPO}"
```

**Step 2: Verify script is valid**

Run: `sh -n install.sh`
Expected: No output (syntax check passes)

**Step 3: Commit**

```bash
git add install.sh
git commit -m "feat: add install.sh curl installer for macOS and Linux"
```

---

### Task 5: Create Homebrew Formula

**Files:**
- Create: `deploy/homebrew/zeptoclaw.rb`

**Step 1: Write Homebrew formula**

Create `deploy/homebrew/zeptoclaw.rb`. This formula will be copied to the `qhkm/homebrew-tap` repo on GitHub. It downloads pre-built binaries from GitHub Releases.

```ruby
class Zeptoclaw < Formula
  desc "Ultra-lightweight AI assistant framework written in Rust"
  homepage "https://github.com/qhkm/zeptoclaw"
  version "0.2.0"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/qhkm/zeptoclaw/releases/download/v#{version}/zeptoclaw-macos-aarch64"
      sha256 "PLACEHOLDER_SHA256_MACOS_AARCH64"
    else
      url "https://github.com/qhkm/zeptoclaw/releases/download/v#{version}/zeptoclaw-macos-x86_64"
      sha256 "PLACEHOLDER_SHA256_MACOS_X86_64"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/qhkm/zeptoclaw/releases/download/v#{version}/zeptoclaw-linux-aarch64"
      sha256 "PLACEHOLDER_SHA256_LINUX_AARCH64"
    else
      url "https://github.com/qhkm/zeptoclaw/releases/download/v#{version}/zeptoclaw-linux-x86_64"
      sha256 "PLACEHOLDER_SHA256_LINUX_X86_64"
    end
  end

  def install
    bin.install stable.url.split("/").last => "zeptoclaw"
  end

  test do
    assert_match "zeptoclaw", shell_output("#{bin}/zeptoclaw --version")
  end
end
```

**Note:** The `PLACEHOLDER_SHA256_*` values must be updated after the v0.2.0 release artifacts are built. After tagging, download each artifact, compute `sha256sum`, and update the formula. Then push the formula to the `qhkm/homebrew-tap` repo.

**Step 2: Verify Ruby syntax**

Run: `ruby -c deploy/homebrew/zeptoclaw.rb`
Expected: "Syntax OK"

**Step 3: Commit**

```bash
git add deploy/homebrew/zeptoclaw.rb
git commit -m "feat: add Homebrew formula for tap distribution"
```

---

### Task 6: Update Cargo.toml Metadata

**Files:**
- Modify: `Cargo.toml:1-9`

**Step 1: Update version and add metadata**

Change version from `0.1.0` to `0.2.0`. Add `homepage`, `keywords`, and `categories` fields.

Before:
```toml
[package]
name = "zeptoclaw"
version = "0.1.0"
edition = "2021"
authors = ["ZeptoClaw Contributors"]
description = "Ultra-lightweight personal AI assistant framework"
license = "Apache-2.0"
readme = "README.md"
repository = "https://github.com/qhkm/zeptoclaw"
```

After:
```toml
[package]
name = "zeptoclaw"
version = "0.2.0"
edition = "2021"
authors = ["ZeptoClaw Contributors"]
description = "Ultra-lightweight personal AI assistant framework"
license = "Apache-2.0"
readme = "README.md"
repository = "https://github.com/qhkm/zeptoclaw"
homepage = "https://github.com/qhkm/zeptoclaw"
keywords = ["ai", "agent", "llm", "cli", "rust"]
categories = ["command-line-utilities"]
```

**Step 2: Verify Cargo.toml parses**

Run: `cargo check 2>&1 | head -5`
Expected: No TOML parse errors (may show compilation warnings, that's fine)

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 0.2.0, add Cargo.toml metadata"
```

---

### Task 7: Update README.md with Badges and Install Methods

**Files:**
- Modify: `README.md:1-69`

**Step 1: Add badges after the header**

Insert badges between the header block and the `---` separator (after line 17, before line 19).

Add these badges:
```markdown
<p align="center">
  <a href="https://github.com/qhkm/zeptoclaw/actions/workflows/ci.yml"><img src="https://github.com/qhkm/zeptoclaw/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/qhkm/zeptoclaw/releases/latest"><img src="https://img.shields.io/github/v/release/qhkm/zeptoclaw?color=blue" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License"></a>
</p>
```

**Step 2: Replace Quick Start section**

Replace the current Quick Start section (lines 52-69) with all 4 install methods:

```markdown
## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://raw.githubusercontent.com/qhkm/zeptoclaw/main/install.sh | sh

# Homebrew
brew install qhkm/tap/zeptoclaw

# Docker
docker pull ghcr.io/qhkm/zeptoclaw:latest

# Build from source
cargo install zeptoclaw --git https://github.com/qhkm/zeptoclaw
```

## Quick Start

```bash
# Interactive setup (walks you through API keys, channels, workspace)
zeptoclaw onboard

# Talk to your agent
zeptoclaw agent -m "Hello, set up my workspace"

# Stream responses token-by-token
zeptoclaw agent -m "Explain async Rust" --stream

# Use a template
zeptoclaw agent --template researcher -m "Search for Rust agent frameworks"

# Start as a Telegram/Slack/Discord/Webhook gateway
zeptoclaw gateway

# With full container isolation per request
zeptoclaw gateway --containerized
```
```

**Step 3: Verify README renders**

Run: `head -30 README.md`
Expected: Shows mascot, badges, and install section

**Step 4: Commit**

```bash
git add README.md
git commit -m "docs: add badges, install methods, and quick start to README"
```

---

### Task 8: Final Quality Gates + Tag Release

**Step 1: Run all quality gates**

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Expected: All pass (1,119+ tests, no warnings, properly formatted)

**Step 2: Verify git is clean**

```bash
git status
git log --oneline -10
```

Expected: Clean working tree, recent commits for tasks 1-7 visible

**Step 3: Tag v0.2.0**

```bash
git tag -a v0.2.0 -m "v0.2.0 — First public release"
```

**Step 4: Verify tag**

```bash
git tag -l "v0.2*"
```

Expected: Shows `v0.2.0`

**NOTE:** Do NOT push the tag yet. The user should push when ready:
```bash
git push origin main --tags
```

This will trigger:
- `.github/workflows/release.yml` — Builds 4 platform binaries + creates GitHub Release
- `.github/workflows/docker.yml` — Builds and pushes Docker image to ghcr.io

After the release, update the Homebrew formula SHA256 hashes with actual artifact checksums.
