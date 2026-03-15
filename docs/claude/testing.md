# Testing Reference

## Test Commands

Uses `cargo nextest` (process-per-test isolation, avoids OOM on low-RAM machines).
Install: `cargo install cargo-nextest --locked`

```bash
cargo nextest run --lib          # Unit tests
cargo nextest run --bin zeptoclaw # Main binary tests
cargo nextest run --test cli_smoke
cargo nextest run --test e2e
cargo nextest run --test integration
cargo nextest run                # All (excludes doc tests)
cargo nextest run test_name      # Specific test
cargo nextest run --no-capture   # With output

# Doc tests (separate)
cargo test --doc

# Fallback (may OOM on low-RAM)
cargo test --lib -- --test-threads=1
```

## Test Counts

lib 3163 total (3157 passed, 6 ignored), main 92, cli_smoke 24, e2e 13, integration 70, doc 127 passed (27 ignored). Optional features like `whatsapp-web` add feature-gated coverage.

## Manual Stabilization Smoke

Use this when stabilizing rather than adding surface area. The minimum path:

```bash
./target/release/zeptoclaw config check
./target/release/zeptoclaw provider status
./target/release/zeptoclaw agent -m "Hello"
```

Priority checks:

1. **Fresh install**: build, `--help`, `--version`, first run without panic
2. **Config**: `config check` handles missing, invalid, and valid config clearly
3. **Provider**: `provider status` shows one usable provider or specific failure reason
4. **Core agent**: `agent -m "Hello"` returns a response on repeated runs
5. **Streaming**: `agent -m "Hello"` streams by default and exits cleanly
6. **Interactive**: `agent` accepts input and exits with `quit`
7. **Error path**: missing API key or bad model fails cleanly with actionable stderr
8. **Tool safety**: `agent --dry-run -m "..."` works and tool failure does not crash
9. **Batch**: a tiny `batch --input prompts.txt` run succeeds and reports failures clearly
10. **Persistence**: history and memory commands do not panic on empty state

Turn any panic, hang, misleading success, inconsistent repeated run, or broken documented command into a GitHub issue.

## Benchmarks (Apple Silicon, release build)

- Binary size: ~6MB (stripped, macos-aarch64)
- Startup time: ~50ms
- Memory (RSS): ~6MB
