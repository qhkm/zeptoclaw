---
title: Installation
description: Install ZeptoClaw on your system
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 4
---

ZeptoClaw is distributed as a single static binary. Choose the installation method that works best for your platform.

## Prerequisites

- **macOS or Linux** — ZeptoClaw runs on both platforms (x86_64 and ARM64)
- **No runtime dependencies** — The binary is fully self-contained

## Install with script (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/qhkm/zeptoclaw/main/install.sh | sh
```

This downloads the latest release binary for your platform and places it in your PATH.

## Install with Homebrew (macOS/Linux)

```bash
brew install qhkm/tap/zeptoclaw
```

## Install with Cargo

Build from source using Rust's package manager:

```bash
cargo install zeptoclaw --git https://github.com/qhkm/zeptoclaw
```

## Docker

Run ZeptoClaw in a container:

```bash
docker pull ghcr.io/qhkm/zeptoclaw:latest

# Run agent mode
docker run --rm ghcr.io/qhkm/zeptoclaw:latest agent -m "Hello"

# Run gateway mode with config
docker run -d \
  -v ~/.zeptoclaw:/root/.zeptoclaw \
  -e ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-... \
  ghcr.io/qhkm/zeptoclaw:latest gateway
```

## Download binary

Pre-built binaries are available on the [releases page](https://github.com/qhkm/zeptoclaw/releases):

```bash
# Linux x86_64
curl -L https://github.com/qhkm/zeptoclaw/releases/latest/download/zeptoclaw-linux-x64.tar.gz | tar xz

# macOS (Apple Silicon)
curl -L https://github.com/qhkm/zeptoclaw/releases/latest/download/zeptoclaw-darwin-arm64.tar.gz | tar xz

# macOS (Intel)
curl -L https://github.com/qhkm/zeptoclaw/releases/latest/download/zeptoclaw-darwin-x64.tar.gz | tar xz
```

## Build from source

To build from source, you need Rust 1.70+:

```bash
git clone https://github.com/qhkm/zeptoclaw.git
cd zeptoclaw

# Build release binary (~5MB)
cargo build --release

# Verify
./target/release/zeptoclaw --version
```

## Verify installation

```bash
zeptoclaw --version
# zeptoclaw 0.2.0

zeptoclaw --help
# Shows available commands
```

## Next steps

Now that ZeptoClaw is installed, follow the [quick start guide](/getting-started/quick-start/) to run your first agent interaction.
