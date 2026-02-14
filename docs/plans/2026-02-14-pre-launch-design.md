# ZeptoClaw Pre-Launch Checklist — Design

**Goal:** Prepare the ZeptoClaw repo for its first public appearance on GitHub with professional docs, multiple install methods, and a tagged v0.2.0 release.

**Repo:** `github.com/qhkm/zeptoclaw`

**Current state:** Code is production-ready (1,119 tests, CI/CD, Docker, landing page). Missing: CHANGELOG, SECURITY, CONTRIBUTING, install infrastructure.

---

## 1. Missing Documentation (3 files)

### CHANGELOG.md
- Follow [Keep a Changelog](https://keepachangelog.com/) format
- Single release: v0.2.0 (skip v0.1.0 since this is the first public release)
- Categories: Added, Changed, Security
- Cover all features: streaming, agent swarms (DelegateTool), plugins, templates, 4 channels, batch mode, cost tracking, telemetry, tool approval, token budget, conversation history, long-term memory, retry/fallback providers, structured output, 17 tools

### SECURITY.md
- Security features summary (7 defense layers from README)
- Supported versions table
- Reporting vulnerabilities: email + expected response time
- Link to `src/security/` module

### CONTRIBUTING.md
- Quick start (fork, branch, PR)
- Quality gates: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
- Commit convention (feat/fix/docs/refactor)
- Point to CLAUDE.md for architecture, AGENTS.md for coding guidelines
- Code of conduct reference (use Contributor Covenant)

---

## 2. Install Infrastructure

### install.sh (curl installer)
- Detect OS (Linux/macOS) and arch (x86_64/aarch64)
- Map to GitHub Release artifact names (e.g., `zeptoclaw-macos-aarch64`)
- Download from `https://github.com/qhkm/zeptoclaw/releases/latest/download/`
- Verify SHA256 checksum
- Install to `/usr/local/bin/zeptoclaw`
- Print success message with next steps

### Homebrew tap
- Create formula file for `qhkm/homebrew-tap` repo
- Formula downloads platform-specific binary from GitHub Releases
- User runs: `brew install qhkm/tap/zeptoclaw`
- NOTE: The tap repo (`homebrew-tap`) must be created on GitHub separately — we just prepare the formula file here

### Docker (already done)
- `docker.yml` CI already pushes to `ghcr.io/qhkm/zeptoclaw` on tags
- No changes needed

### cargo install (already works)
- `cargo install zeptoclaw --git https://github.com/qhkm/zeptoclaw`
- No changes needed

---

## 3. Cargo.toml Updates

- Version: `0.1.0` → `0.2.0`
- Add: `homepage = "https://github.com/qhkm/zeptoclaw"`
- Add: `keywords = ["ai", "agent", "llm", "cli", "rust"]`
- Add: `categories = ["command-line-utilities"]`

---

## 4. README.md Updates

- Add badges at top: CI status, license, version/release
- Replace "Quick Start" build-from-source with all 4 install methods
- Keep existing content otherwise (it's already good)

---

## 5. Tag + Release

- Commit all changes
- `git tag v0.2.0`
- Push tag → CI auto-builds 4 platform binaries + Docker image
- GitHub Release auto-created with artifacts
