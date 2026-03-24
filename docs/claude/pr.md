# Pull Request Guidelines

When creating a PR, follow the project's contribution guidelines in `CONTRIBUTING.md` and use the PR template in `.github/PULL_REQUEST_TEMPLATE.md`.

## Before Creating a PR

1. **Create an issue first** (unless trivial). See CLAUDE.md "New Work" section.
2. **Run all quality gates:**
   ```bash
   cargo fmt && cargo clippy -- -D warnings && cargo nextest run --lib && cargo test --doc && cargo fmt -- --check
   ```
3. **Ensure no duplicated constants/limits** — check for existing constants before introducing magic numbers.
4. **Avoid unnecessary new dependencies** — the project targets ~6 MB binary size.

## PR Template Sections

The PR template at `.github/PULL_REQUEST_TEMPLATE.md` is the source of truth. Key sections:

### Summary
- Do NOT leave empty
- Explain the **problem** (what was broken and why), the **fix** (what you did), and the **outcome** (what works now)
- Write for humans — explain context, don't just list code changes
- 1-3 paragraphs is fine for non-trivial changes; don't be terse

### Related Issue
- Always include `Closes #N` to auto-close the issue on merge
- If no issue exists, explain why (e.g. typo fix, trivial refactor)

### Pre-submit Checklist
- All boxes must be checked before requesting review
- Includes: branched from `upstream/main`, focused commits, fmt/clippy/nextest pass, tests added/updated, no duplicated constants, no unnecessary deps

### Security Considerations
- Delete this section for docs-only or clearly non-security changes
- Otherwise note: untrusted input handling, new network endpoints, secret storage, unbounded collections

### Test Plan
- Do NOT leave empty
- List specific manual test steps or automated tests
- Include both happy path and failure/edge cases

## PR Body Style

- Lead with the problem and its impact on users, not the code change
- Explain root causes — why was it broken, not just what was broken
- Describe the fix in terms a reviewer unfamiliar with the code can follow
- Use "What changed" subsections for multi-part changes to make review easier

## After Creating

- CodeRabbit will automatically review the PR — address its feedback before requesting maintainer review
- **NEVER merge without explicit user approval** — wait for CI, present the URL, merge only after user says to
- Merge command: `gh pr merge <number> --squash --delete-branch --admin`
