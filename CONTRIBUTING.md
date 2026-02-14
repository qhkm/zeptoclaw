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
cargo test                    # All 1,119+ tests pass
cargo clippy -- -D warnings   # No warnings
cargo fmt --check             # Properly formatted
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
