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

## Branching (Important for Forks)

**Always branch from `upstream/main`**, not your fork's `main`:

```bash
# One-time setup: add upstream remote
git remote add upstream https://github.com/qhkm/zeptoclaw.git

# Start every new feature branch from upstream
git fetch upstream
git checkout -b feat/my-feature upstream/main
# ... work ...
git push origin feat/my-feature
# Open PR against qhkm/zeptoclaw main
```

**Do not** merge your feature branches into your fork's `main`. Keep your fork's `main` as a clean mirror of upstream:

```bash
# Sync your fork's main (don't merge feature branches into it)
git checkout main
git fetch upstream
git reset --hard upstream/main
git push origin main --force-with-lease
```

This ensures each PR only contains its own commits. PRs that include unrelated commits from other branches will be asked to rebase.

## Issues Before PRs

For anything beyond a trivial fix (typo, one-line bug), **open an issue first**:

1. **Bugs** — Use the "Bug Report" template
2. **Features** — Use the "Feature Request" template. Describe the problem, proposed solution, and scope estimate
3. **Design discussions** — Open a blank issue with the `rfc` label for larger changes

This lets maintainers weigh in on scope and approach before you invest time coding. PRs without a linked issue may be asked to create one.

**Labels we use:**
- **Type:** `bug`, `feat`, `rfc`, `chore`, `docs`
- **Area:** `area:tools`, `area:channels`, `area:providers`, `area:safety`, `area:config`, `area:cli`, `area:memory`
- **Priority:** `P1-critical`, `P2-high`, `P3-normal`

## Pull Request Process

1. Open an issue describing the change (see above)
2. Create a feature branch from `upstream/main` (see Branching section)
3. Make your changes with clear, focused commits
4. Ensure all quality gates pass (see below)
5. Open a PR against `main` — reference the issue with `Closes #N`

## Quality Gates

Every PR must pass:

```bash
cargo test                    # All tests pass
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
4. Register in agent setup in `src/cli/common.rs`
5. Add tests

## Adding a New Channel

1. Create `src/channels/yourchannel.rs`
2. Implement the `Channel` trait
3. Export from `src/channels/mod.rs`
4. Add config struct to `src/config/types.rs`
5. Register in channel factory

## Code of Conduct

This project follows the [Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/) Code of Conduct.
