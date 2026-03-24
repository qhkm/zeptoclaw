## Summary

<!-- 1-3 bullet points describing what this PR does. DO NOT leave this empty. -->

## Related Issue

<!-- Link the issue this PR addresses. Use "Closes #N" to auto-close on merge. -->
<!-- If no issue exists, explain why (e.g. typo fix, trivial refactor). -->

Closes #

## Pre-submit Checklist

- [ ] I branched from `upstream/main`
- [ ] This PR contains only commits related to this change
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo nextest run --lib` passes
- [ ] I added or updated tests for my changes
- [ ] New constants/limits are shared (not duplicated across files)
- [ ] No new dependencies unless necessary (we target ~6 MB binary)

## Security Considerations

<!-- Delete this section if not applicable (e.g. docs-only change). -->
<!-- Otherwise, briefly note any security-relevant aspects of your change: -->
<!-- - Does it handle untrusted input? (validate/sanitize/cap lengths) -->
<!-- - Does it add network listeners or endpoints? (auth, rate limiting) -->
<!-- - Does it store secrets? (constant-time comparison, no logging) -->
<!-- - Does it add unbounded collections? (cap size, add eviction) -->

N/A

## Test Plan

<!-- How did you verify this works? List specific tests or manual steps. DO NOT leave this empty. -->

> **Note:** [CodeRabbit](https://coderabbit.ai) will automatically review your PR. Please check and address its feedback before requesting maintainer review.
