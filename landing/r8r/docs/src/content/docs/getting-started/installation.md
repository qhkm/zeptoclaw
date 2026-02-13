---
title: Installation
description: Install r8r on your system
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 4
---

r8r is distributed as a single static binary. Choose the installation method that works best for your platform.

## Prerequisites

- **Linux/macOS/Windows**: r8r runs on all major platforms
- **No runtime dependencies**: The binary is fully self-contained

## Install with Cargo

The recommended way to install r8r is via Rust's package manager:

```bash
cargo install r8r
```

This installs the latest release from [crates.io](https://crates.io/crates/r8r).

## Install with Homebrew (macOS/Linux)

```bash
brew tap r8r/tap
brew install r8r
```

## Install with script

For the latest development version:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://r8r.dev/install.sh | sh
```

## Download binary

Pre-built binaries are available on the [releases page](https://github.com/r8r/r8r/releases):

```bash
# Linux x86_64
curl -L https://github.com/r8r/r8r/releases/latest/download/r8r-linux-x64.tar.gz | tar xz

# macOS (Apple Silicon)
curl -L https://github.com/r8r/r8r/releases/latest/download/r8r-darwin-arm64.tar.gz | tar xz

# macOS (Intel)
curl -L https://github.com/r8r/r8r/releases/latest/download/r8r-darwin-x64.tar.gz | tar xz
```

## Build from source

To build from source, you'll need Rust 1.70+:

```bash
# Clone the repository
git clone https://github.com/r8r/r8r.git
cd r8r

# Build release binary
cargo build --release

# Binary will be at target/release/r8r
./target/release/r8r --version
```

## Docker

Run r8r in a container:

```bash
docker run -v $(pwd)/workflows:/workflows ghcr.io/r8r/r8r:latest
```

## Verify installation

Check that r8r is installed correctly:

```bash
r8r --version
# r8r 0.1.0

r8r --help
# Shows available commands
```

## Shell completions

Enable tab completion for your shell:

```bash
# Bash
r8r completions bash > /etc/bash_completion.d/r8r

# Zsh
r8r completions zsh > /usr/local/share/zsh/site-functions/_r8r

# Fish
r8r completions fish > ~/.config/fish/completions/r8r.fish
```

## Next steps

Now that r8r is installed, [create your first workflow](/getting-started/quick-start/).
