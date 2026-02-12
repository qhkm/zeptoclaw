# AGENTS.md

Project-level guidance for coding agents working in this repository.

## Scope

These instructions apply to the entire `rust/` project.

## Project Snapshot

- Language: Rust (edition 2021)
- Core binary: `zeptoclaw` (`src/main.rs`)
- Extra binary: `benchmark` (`src/bin/benchmark.rs`)
- Benchmarks: `benches/message_bus.rs`
- Integration tests: `tests/integration.rs`

## Required Quality Gates

Before finishing any non-trivial change, run:

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```

If benchmark-related code is changed, also run:

```bash
cargo bench --bench message_bus --no-run
```

## Coding Rules

- Keep changes minimal and focused.
- Prefer small, composable functions over large blocks.
- Do not add `unwrap()`/`expect()` in production paths unless failure is truly unrecoverable.
- Preserve existing module boundaries and public APIs unless explicitly requested.
- Keep comments short and only where intent is non-obvious.

## Runtime and Provider Notes

- Runtime isolation features must remain opt-in and degrade safely to native runtime.
- Provider wiring should remain consistent across config, onboarding, status output, and runtime behavior.
- Do not hardcode a single provider path when multiple providers are supported.

## Documentation Rules

- Keep README/docs claims aligned with executable behavior.
- Do not add performance numbers unless they are reproducible with repository commands.
- If adding new commands or workflows, include a runnable example.

## Change Hygiene

- Do not revert unrelated local changes.
- If you detect unexpected file modifications during work, pause and ask before proceeding.
- Include file/line references when reporting review findings.
