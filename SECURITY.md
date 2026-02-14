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
