# Tool Conformance, Edit Fuzzy Matching, Output Truncation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add conformance fixture testing, fuzzy edit matching, and shared output truncation to ZeptoClaw's coding tools.

**Architecture:** Three independent features that compose: (1) shared truncation utility extracted from shell.rs, applied to 5 tools; (2) tiered fuzzy matching in EditFileTool with exact → Unicode NFC → whitespace fallback; (3) JSON fixture runner for tool regression testing. Each task is independently committable.

**Tech Stack:** Rust, `unicode-normalization` crate, `serde_json` for fixtures, `cargo nextest` for test execution.

**Spec:** `docs/superpowers/specs/2026-03-21-tool-conformance-edit-truncation-design.md`

**Issue:** #391

---

## File Structure

| File | Responsibility |
|------|---------------|
| `src/tools/output.rs` | **New.** Shared `truncate_tool_output()` function + `DEFAULT_MAX_LINES` / `DEFAULT_MAX_BYTES` constants |
| `src/tools/mod.rs` | Add `pub mod output;` |
| `src/tools/shell.rs` | Remove private truncation function + constants, import shared |
| `src/tools/filesystem.rs` | Add `find_unique_match()`, `EditMatchError`, `MatchTier`, `UniqueMatch`. Update `EditFileTool::execute()` |
| `src/tools/grep.rs` | Add `truncate_tool_output()` call after match-limit step |
| `src/tools/find.rs` | Add `truncate_tool_output()` call on final output |
| `tests/conformance.rs` | **New.** Top-level test entry point |
| `tests/conformance/mod.rs` | **New.** Re-exports runner |
| `tests/conformance/runner.rs` | **New.** Generic fixture runner: deserialize, setup, execute, validate |
| `tests/conformance/fixtures/edit_tool.json` | **New.** 12-15 edit tool test cases |
| `tests/conformance/fixtures/shell_tool.json` | **New.** 5 shell tool test cases |
| `tests/conformance/fixtures/read_tool.json` | **New.** 5 read tool test cases |
| `tests/conformance/fixtures/grep_tool.json` | **New.** 5 grep tool test cases |
| `tests/conformance/fixtures/find_tool.json` | **New.** 5 find tool test cases |
| `Cargo.toml` | Add `unicode-normalization` dependency |

---

### Task 1: Shared output truncation utility

**Files:**
- Create: `src/tools/output.rs`
- Modify: `src/tools/mod.rs:59-101`
- Modify: `src/tools/shell.rs:1-67`

- [ ] **Step 1: Write tests for `truncate_tool_output` in `output.rs`**

Create `src/tools/output.rs` with the constants, function signature returning empty string for now, and test module:

```rust
pub const DEFAULT_MAX_LINES: usize = 2_000;
pub const DEFAULT_MAX_BYTES: usize = 50_000;

/// Truncate text by line count and byte count (whichever hits first).
/// Char-boundary-safe. Appends reason suffix if truncated.
pub fn truncate_tool_output(output: &str, max_lines: usize, max_bytes: usize) -> String {
    output.to_string() // stub
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_truncation_small_input() {
        let input = "line 1\nline 2\nline 3\n";
        let result = truncate_tool_output(input, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert_eq!(result, input);
    }

    #[test]
    fn test_truncate_by_lines() {
        let input: String = (0..=DEFAULT_MAX_LINES)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_tool_output(&input, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(result.contains("[output truncated at"));
        assert!(result.contains("lines"));
        assert!(!result.contains(&format!("line {}", DEFAULT_MAX_LINES)));
    }

    #[test]
    fn test_truncate_by_bytes() {
        let input = "x".repeat(DEFAULT_MAX_BYTES + 1);
        let result = truncate_tool_output(&input, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(result.contains("[output truncated at"));
        assert!(result.contains("bytes"));
    }

    #[test]
    fn test_empty_input() {
        let result = truncate_tool_output("", DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert_eq!(result, "");
    }

    #[test]
    fn test_char_boundary_safety() {
        // Multi-byte UTF-8: each char is 4 bytes
        let input = "\u{1F600}".repeat(DEFAULT_MAX_BYTES / 4 + 10);
        let result = truncate_tool_output(&input, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        // Must not panic and must be valid UTF-8
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn test_both_limits_hit() {
        // Many short lines that also exceed byte limit
        let input: String = (0..DEFAULT_MAX_LINES + 100)
            .map(|_| "x".repeat(100))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_tool_output(&input, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(result.contains("[output truncated at"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run --lib output::tests -p zeptoclaw`
Expected: 4 of 6 tests FAIL (stub returns untruncated output)

- [ ] **Step 3: Implement `truncate_tool_output`**

Replace the stub with the real implementation (move logic from `shell.rs:20-67`, parameterize the limits):

```rust
pub fn truncate_tool_output(output: &str, max_lines: usize, max_bytes: usize) -> String {
    if output.is_empty() {
        return String::new();
    }

    let mut truncated = String::new();
    let mut byte_count = 0usize;
    let mut hit_line_limit = false;
    let mut hit_byte_limit = false;

    for (line_count, segment) in output.split_inclusive('\n').enumerate() {
        if line_count + 1 > max_lines {
            hit_line_limit = true;
            break;
        }

        if byte_count + segment.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(byte_count);
            if remaining > 0 {
                let mut cut = remaining;
                while cut > 0 && !segment.is_char_boundary(cut) {
                    cut -= 1;
                }
                truncated.push_str(&segment[..cut]);
            }
            hit_byte_limit = true;
            break;
        }

        truncated.push_str(segment);
        byte_count += segment.len();
    }

    if hit_line_limit || hit_byte_limit {
        if !truncated.ends_with('\n') && !truncated.is_empty() {
            truncated.push('\n');
        }
        let reason = match (hit_line_limit, hit_byte_limit) {
            (true, true) => format!("{} lines and {} bytes", max_lines, max_bytes),
            (true, false) => format!("{} lines", max_lines),
            (false, true) => format!("{} bytes", max_bytes),
            (false, false) => unreachable!(),
        };
        truncated.push_str(&format!("... [output truncated at {}]", reason));
    }

    truncated
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run --lib output::tests -p zeptoclaw`
Expected: All 6 PASS

- [ ] **Step 5: Wire module in `mod.rs`**

In `src/tools/mod.rs`, add `pub mod output;` between `pub mod message;` and `pub mod pdf_read;` (alphabetical order):

```rust
pub mod output;
```

- [ ] **Step 6: Migrate shell.rs to use shared utility**

In `src/tools/shell.rs`:
- Remove lines 17-67 (the private `MAX_OUTPUT_LINES`, `MAX_OUTPUT_BYTES`, and `truncate_formatted_output` function)
- Add import: `use super::output::{truncate_tool_output, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES};`
- Replace the call at line 225 (`truncate_formatted_output(...)`) with:
  `truncate_tool_output(&output.format(), DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES)`

- [ ] **Step 7: Run existing shell tests**

Note: Step 6 removes the private `truncate_formatted_output` function. The existing tests `test_truncate_formatted_output_by_lines` and `test_truncate_formatted_output_by_bytes` in shell.rs reference this function — remove these two tests from shell.rs in Step 6 (they are now covered by equivalent tests in output.rs).

Run: `cargo nextest run --lib shell::tests -p zeptoclaw`
Expected: All remaining shell tests PASS

- [ ] **Step 8: Commit**

```bash
git add src/tools/output.rs src/tools/mod.rs src/tools/shell.rs
git commit -m "feat(tools): extract shared output truncation utility

Move truncation logic from ShellTool into shared truncate_tool_output()
in src/tools/output.rs. Parameterized by max_lines and max_bytes.

Part of #391"
```

---

### Task 2: Apply truncation to remaining tools

**Files:**
- Modify: `src/tools/filesystem.rs:175-190` (ReadFileTool::execute)
- Modify: `src/tools/filesystem.rs:340-363` (ListDirTool::execute)
- Modify: `src/tools/grep.rs:148-160`
- Modify: `src/tools/find.rs:93-99`

- [ ] **Step 1: Add truncation to ReadFileTool**

In `src/tools/filesystem.rs`, add import at top:

```rust
use super::output::{truncate_tool_output, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES};
```

Change `ReadFileTool::execute` return (line 189) from:

```rust
Ok(ToolOutput::llm_only(content))
```

to:

```rust
Ok(ToolOutput::llm_only(truncate_tool_output(
    &content,
    DEFAULT_MAX_LINES,
    DEFAULT_MAX_BYTES,
)))
```

- [ ] **Step 2: Add truncation to ListDirTool**

Change `ListDirTool::execute` return (line 362) from:

```rust
Ok(ToolOutput::llm_only(items.join("\n")))
```

to:

```rust
let joined = items.join("\n");
Ok(ToolOutput::llm_only(truncate_tool_output(
    &joined,
    DEFAULT_MAX_LINES,
    DEFAULT_MAX_BYTES,
)))
```

- [ ] **Step 3: Add truncation to GrepTool**

In `src/tools/grep.rs`, add import:

```rust
use super::output::{truncate_tool_output, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES};
```

Change line 160 from:

```rust
Ok(ToolOutput::llm_only(result))
```

to:

```rust
Ok(ToolOutput::llm_only(truncate_tool_output(
    &result,
    DEFAULT_MAX_LINES,
    DEFAULT_MAX_BYTES,
)))
```

- [ ] **Step 4: Add truncation to FindTool**

In `src/tools/find.rs`, add import:

```rust
use super::output::{truncate_tool_output, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES};
```

Change lines 94-98 from:

```rust
Ok(ToolOutput::llm_only(format!(
    "{}\n({} files)",
    entries.join("\n"),
    count
)))
```

to:

```rust
let output = format!("{}\n({} files)", entries.join("\n"), count);
Ok(ToolOutput::llm_only(truncate_tool_output(
    &output,
    DEFAULT_MAX_LINES,
    DEFAULT_MAX_BYTES,
)))
```

- [ ] **Step 5: Run all tool tests**

Run: `cargo nextest run --lib -p zeptoclaw -- filesystem::tests grep::tests find::tests`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/tools/filesystem.rs src/tools/grep.rs src/tools/find.rs
git commit -m "feat(tools): apply shared truncation to read, listdir, grep, find

All text-producing tools now truncate at 2000 lines / 50KB via shared
truncate_tool_output(). Prevents context blowout from large files or
grep results.

Part of #391"
```

---

### Task 3: Add `unicode-normalization` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add dependency**

```bash
cargo add unicode-normalization
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Clean compile

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add unicode-normalization dependency for edit fuzzy matching

Part of #391"
```

---

### Task 4: Edit fuzzy matching — `find_unique_match()`

**Files:**
- Modify: `src/tools/filesystem.rs`

- [ ] **Step 1: Write tests for `find_unique_match`**

Add to the bottom of the `#[cfg(test)] mod tests` block in `src/tools/filesystem.rs`:

```rust
    // --- find_unique_match tests ---

    use super::{find_unique_match, MatchTier};

    #[test]
    fn test_exact_single_match() {
        let content = "fn main() {}";
        let result = find_unique_match(content, "main").unwrap();
        assert_eq!(&content[result.start..result.end], "main");
        assert!(matches!(result.tier, MatchTier::Exact));
    }

    #[test]
    fn test_exact_multi_match_errors() {
        let content = "foo bar foo baz foo";
        let result = find_unique_match(content, "foo");
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("3"));
    }

    #[test]
    fn test_not_found_errors() {
        let content = "fn main() {}";
        let result = find_unique_match(content, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_unicode_nfc_fallback() {
        // e + combining acute (NFD) in content, but precomposed e-acute (NFC) in search
        let content = "caf\u{0065}\u{0301}"; // NFD: e + combining acute = 6 bytes
        let search = "caf\u{00E9}"; // NFC: precomposed e-acute
        let result = find_unique_match(content, search).unwrap();
        assert!(matches!(result.tier, MatchTier::UnicodeNormalized));
        assert_eq!(result.start, 0);
        assert_eq!(result.end, content.len()); // must cover full NFD string
    }

    #[test]
    fn test_unicode_nfc_mid_string() {
        // NFC match in the middle of a longer string
        let content = "hello caf\u{0065}\u{0301} world";
        let search = "caf\u{00E9}";
        let result = find_unique_match(content, search).unwrap();
        assert!(matches!(result.tier, MatchTier::UnicodeNormalized));
        assert_eq!(result.start, 6); // "hello " is 6 bytes
        assert_eq!(&content[result.start..result.end], "caf\u{0065}\u{0301}");
    }

    #[test]
    fn test_whitespace_tabs_vs_spaces() {
        let content = "fn\tmain()\t{}";
        let search = "fn main() {}";
        let result = find_unique_match(content, search).unwrap();
        assert!(matches!(result.tier, MatchTier::WhitespaceNormalized));
        assert_eq!(result.start, 0);
        assert_eq!(result.end, content.len());
    }

    #[test]
    fn test_whitespace_trailing() {
        let content = "hello world   ";
        let search = "hello world";
        let result = find_unique_match(content, search).unwrap();
        assert!(matches!(result.tier, MatchTier::WhitespaceNormalized));
        assert_eq!(result.start, 0);
        // Matches "hello world" (11 bytes), trailing spaces outside match range
        assert_eq!(result.end, 11);
    }

    #[test]
    fn test_whitespace_crlf_normalization() {
        let content = "line1\r\nline2";
        let search = "line1\nline2";
        let result = find_unique_match(content, search).unwrap();
        assert!(matches!(result.tier, MatchTier::WhitespaceNormalized));
        assert_eq!(result.start, 0);
        assert_eq!(result.end, content.len());
    }

    #[test]
    fn test_fuzzy_multi_match_errors() {
        // Two locations that normalize to the same thing
        let content = "fn\tmain() {}\nfn\t\tmain() {}";
        let search = "fn main() {}";
        let result = find_unique_match(content, search);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_content() {
        let result = find_unique_match("", "search");
        assert!(result.is_err());
    }
```

- [ ] **Step 2: Add types and stub function**

Add above the `#[cfg(test)]` block in `src/tools/filesystem.rs`:

```rust
// --- Edit fuzzy matching ---

use unicode_normalization::UnicodeNormalization;

#[derive(Debug)]
enum MatchTier {
    Exact,
    UnicodeNormalized,
    WhitespaceNormalized,
}

#[derive(Debug)]
struct UniqueMatch {
    start: usize,
    end: usize,
    tier: MatchTier,
}

#[derive(Debug)]
enum EditMatchError {
    NotFound,
    MultipleMatches(usize),
}

impl std::fmt::Display for EditMatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditMatchError::NotFound => write!(f, "not found"),
            EditMatchError::MultipleMatches(n) => write!(f, "Found {} occurrences", n),
        }
    }
}

fn find_unique_match(_content: &str, _old_text: &str) -> std::result::Result<UniqueMatch, EditMatchError> {
    Err(EditMatchError::NotFound)
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo nextest run --lib filesystem::tests::test_exact -p zeptoclaw`
Expected: FAIL (stub returns Err for everything)

- [ ] **Step 4: Implement `find_unique_match`**

Replace the stub with:

```rust
/// Normalize whitespace: collapse runs of spaces/tabs to single space,
/// trim trailing whitespace per line, normalize \r\n to \n.
/// Note: .lines() strips the final newline, so "foo\n" and "foo" normalize
/// identically. This is acceptable for edit matching — the distinction
/// is irrelevant for substring search within file content.
fn normalize_whitespace(s: &str) -> String {
    s.replace("\r\n", "\n")
        .lines()
        .map(|line| {
            let mut result = String::new();
            let mut prev_ws = false;
            for ch in line.chars() {
                if ch == ' ' || ch == '\t' {
                    if !prev_ws {
                        result.push(' ');
                    }
                    prev_ws = true;
                } else {
                    result.push(ch);
                    prev_ws = false;
                }
            }
            // Trim trailing whitespace (the collapsed space at end)
            result.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Count non-overlapping occurrences and collect their byte offsets.
fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        positions.push(start + pos);
        start += pos + needle.len();
    }
    positions
}

fn find_unique_match(content: &str, old_text: &str) -> std::result::Result<UniqueMatch, EditMatchError> {
    // Tier 1: Exact match
    let exact_positions = find_all_occurrences(content, old_text);
    match exact_positions.len() {
        1 => {
            return Ok(UniqueMatch {
                start: exact_positions[0],
                end: exact_positions[0] + old_text.len(),
                tier: MatchTier::Exact,
            });
        }
        n if n > 1 => {
            return Err(EditMatchError::MultipleMatches(n));
        }
        _ => {} // 0 matches, try next tier
    }

    // Tier 2: Unicode NFC normalized match
    let nfc_content: String = content.nfc().collect();
    let nfc_old: String = old_text.nfc().collect();
    if nfc_content != content.to_string() || nfc_old != old_text.to_string() {
        let nfc_positions = find_all_occurrences(&nfc_content, &nfc_old);
        match nfc_positions.len() {
            1 => {
                // Map NFC byte offset back to original byte offset
                let nfc_start = nfc_positions[0];
                let nfc_end = nfc_start + nfc_old.len();

                // Walk both strings in parallel to find original byte range
                let (orig_start, orig_end) = map_nfc_range_to_original(content, &nfc_content, nfc_start, nfc_end);

                return Ok(UniqueMatch {
                    start: orig_start,
                    end: orig_end,
                    tier: MatchTier::UnicodeNormalized,
                });
            }
            n if n > 1 => {
                return Err(EditMatchError(format!(
                    "Found {} occurrences", n
                )));
            }
            _ => {}
        }
    }

    // Tier 3: Whitespace normalized match
    let ws_content = normalize_whitespace(content);
    let ws_old = normalize_whitespace(old_text);
    let ws_positions = find_all_occurrences(&ws_content, &ws_old);
    match ws_positions.len() {
        1 => {
            let ws_start = ws_positions[0];
            let ws_end = ws_start + ws_old.len();
            let (orig_start, orig_end) = map_ws_range_to_original(content, &ws_content, ws_start, ws_end);
            return Ok(UniqueMatch {
                start: orig_start,
                end: orig_end,
                tier: MatchTier::WhitespaceNormalized,
            });
        }
        n if n > 1 => {
            return Err(EditMatchError::MultipleMatches(n));
        }
        _ => {}
    }

    Err(EditMatchError::NotFound)
}

/// Map a byte range in the NFC-normalized string back to the original string.
/// NFC can merge multiple original chars into one (e.g. e + combining acute → é).
/// We rebuild the NFC char-by-char from the original, tracking which original
/// byte ranges produce which NFC byte ranges.
fn map_nfc_range_to_original(
    original: &str,
    _nfc: &str,
    nfc_start: usize,
    nfc_end: usize,
) -> (usize, usize) {
    let mut orig_start = 0;
    let mut orig_end = original.len();
    let mut nfc_byte_pos = 0;
    let mut found_start = false;

    // Walk original chars, NFC-normalize each one, track cumulative NFC byte position
    for (orig_byte, ch) in original.char_indices() {
        let orig_next = orig_byte + ch.len_utf8();

        // How many NFC bytes does this original char produce?
        let nfc_len: usize = ch.nfc().map(|c| c.len_utf8()).sum();

        if !found_start && nfc_byte_pos + nfc_len > nfc_start {
            orig_start = orig_byte;
            found_start = true;
        }
        nfc_byte_pos += nfc_len;
        if found_start && nfc_byte_pos >= nfc_end {
            orig_end = orig_next;
            break;
        }
    }

    (orig_start, orig_end)
}

/// Map a byte range in whitespace-normalized string back to original.
/// Walk both strings tracking correspondence.
fn map_ws_range_to_original(
    original: &str,
    normalized: &str,
    norm_start: usize,
    norm_end: usize,
) -> (usize, usize) {
    let orig_bytes = original.as_bytes();
    let norm_bytes = normalized.as_bytes();

    let mut orig_i = 0;
    let mut norm_i = 0;
    let mut result_start = 0;
    let mut result_end = original.len();

    while norm_i < norm_bytes.len() && orig_i < orig_bytes.len() {
        if norm_i == norm_start {
            result_start = orig_i;
        }
        if norm_i == norm_end {
            result_end = orig_i;
            break;
        }

        // Handle \r\n → \n mapping
        if orig_bytes[orig_i] == b'\r'
            && orig_i + 1 < orig_bytes.len()
            && orig_bytes[orig_i + 1] == b'\n'
            && norm_bytes[norm_i] == b'\n'
        {
            orig_i += 2;
            norm_i += 1;
            continue;
        }

        // Handle whitespace collapse: skip extra whitespace in original
        if (orig_bytes[orig_i] == b' ' || orig_bytes[orig_i] == b'\t')
            && norm_bytes[norm_i] == b' '
        {
            orig_i += 1;
            norm_i += 1;
            // Skip remaining whitespace in original
            while orig_i < orig_bytes.len()
                && (orig_bytes[orig_i] == b' ' || orig_bytes[orig_i] == b'\t')
            {
                orig_i += 1;
            }
            continue;
        }

        // Handle trailing whitespace in original (trimmed in normalized)
        if (orig_bytes[orig_i] == b' ' || orig_bytes[orig_i] == b'\t')
            && (norm_i >= norm_bytes.len() || norm_bytes[norm_i] == b'\n')
        {
            orig_i += 1;
            continue;
        }

        orig_i += 1;
        norm_i += 1;
    }

    if norm_end >= norm_bytes.len() && result_end == original.len() {
        result_end = original.len();
    }

    (result_start, result_end)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run --lib filesystem::tests::test_exact -p zeptoclaw && cargo nextest run --lib filesystem::tests::test_unicode -p zeptoclaw && cargo nextest run --lib filesystem::tests::test_whitespace -p zeptoclaw && cargo nextest run --lib filesystem::tests::test_fuzzy -p zeptoclaw && cargo nextest run --lib filesystem::tests::test_not_found -p zeptoclaw && cargo nextest run --lib filesystem::tests::test_empty -p zeptoclaw`
Expected: All 9 PASS

- [ ] **Step 6: Commit**

```bash
git add src/tools/filesystem.rs
git commit -m "feat(tools): add find_unique_match with tiered fuzzy matching

Three-tier matching: exact → Unicode NFC → whitespace normalized.
Errors on 0 or 2+ matches at any tier. Maps byte ranges back to
original content for safe replacement.

Part of #391"
```

---

### Task 5: Wire `find_unique_match` into EditFileTool

**Files:**
- Modify: `src/tools/filesystem.rs:487-528`

- [ ] **Step 1: Write test for new multi-match error behavior**

Add test to `filesystem.rs` tests:

```rust
    #[tokio::test]
    async fn test_edit_file_tool_multi_match_without_expected_errors() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "foo bar foo baz foo").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "test.txt",
                    "old_text": "foo",
                    "new_text": "qux"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("3 occurrences"));
        assert!(err.contains("more surrounding context"));
    }

    #[tokio::test]
    async fn test_edit_file_tool_fuzzy_whitespace_match() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "fn\tmain()\t{}").unwrap();

        let tool = EditFileTool;
        let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

        let result = tool
            .execute(
                json!({
                    "path": "test.txt",
                    "old_text": "fn main() {}",
                    "new_text": "fn run() {}"
                }),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "fn run() {}");
    }
```

- [ ] **Step 2: Run new tests to verify they fail**

Run: `cargo nextest run --lib filesystem::tests::test_edit_file_tool_multi_match_without -p zeptoclaw`
Expected: FAIL (current code replaces all 3 and succeeds)

- [ ] **Step 3: Update EditFileTool::execute to use `find_unique_match`**

Replace the string replacement block in `EditFileTool::execute` (lines 487-528) with:

```rust
        } else if let (Some(old_text), Some(new_text)) = (old_text, new_text) {
            // --- String replacement mode ---
            revalidate_path(full_path_ref, &workspace)?;

            if old_text.is_empty() {
                return Err(ZeptoError::Tool("'old_text' must not be empty".into()));
            }

            let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| {
                ZeptoError::Tool(format!("Failed to read file '{}': {}", full_path, e))
            })?;

            if let Some(expected) = expected_replacements {
                // Guarded multi-match: use exact matching with count check
                let replacements = content.matches(old_text).count();
                if replacements == 0 {
                    return Err(ZeptoError::Tool(format!(
                        "Text '{}' not found in file '{}'",
                        crate::utils::string::preview(old_text, 50),
                        full_path
                    )));
                }
                if replacements != expected {
                    return Err(ZeptoError::Tool(format!(
                        "Expected {} replacement(s) for '{}' in '{}', found {}",
                        expected,
                        crate::utils::string::preview(old_text, 50),
                        full_path,
                        replacements
                    )));
                }
                let new_content = content.replace(old_text, new_text);
                write_file_secure(full_path_ref, &workspace, new_content.as_bytes()).await?;
                Ok(ToolOutput::llm_only(format!(
                    "Successfully replaced {} occurrence(s) in {}",
                    replacements, full_path
                )))
            } else {
                // Unique match: use tiered fuzzy matching
                match find_unique_match(&content, old_text) {
                    Ok(m) => {
                        let mut new_content = String::with_capacity(content.len());
                        new_content.push_str(&content[..m.start]);
                        new_content.push_str(new_text);
                        new_content.push_str(&content[m.end..]);
                        write_file_secure(full_path_ref, &workspace, new_content.as_bytes())
                            .await?;
                        Ok(ToolOutput::llm_only(format!(
                            "Successfully replaced 1 occurrence in {}",
                            full_path
                        )))
                    }
                    Err(EditMatchError::MultipleMatches(n)) => {
                        Err(ZeptoError::Tool(format!(
                            "Found {} occurrences of text in '{}'. Provide more surrounding context to uniquely identify the location.",
                            n, full_path
                        )))
                    }
                    Err(EditMatchError::NotFound) => {
                        Err(ZeptoError::Tool(format!(
                            "Text '{}' not found in file '{}'",
                            crate::utils::string::preview(old_text, 50),
                            full_path
                        )))
                    }
                }
            }
```

- [ ] **Step 4: Run all filesystem tests**

Run: `cargo nextest run --lib filesystem::tests -p zeptoclaw`
Expected: All PASS (including existing tests — `test_edit_file_tool` still works because single exact match, `test_edit_file_tool_expected_replacements_match` still works via guarded path)

- [ ] **Step 5: Commit**

```bash
git add src/tools/filesystem.rs
git commit -m "feat(tools): wire fuzzy matching into EditFileTool

Multi-match without expected_replacements now errors instead of
replacing all. Fuzzy fallback (Unicode NFC, whitespace normalization)
catches LLM whitespace/encoding hallucinations.

Breaking: multi-match edits require expected_replacements parameter.

Part of #391"
```

---

### Task 6: Conformance fixture runner

**Files:**
- Create: `tests/conformance.rs`
- Create: `tests/conformance/mod.rs`
- Create: `tests/conformance/runner.rs`

- [ ] **Step 1: Create the fixture runner types and execution logic**

Create `tests/conformance/runner.rs`:

```rust
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use tempfile::tempdir;
use tokio::fs;

use zeptoclaw::tools::{Tool, ToolContext, ToolOutput};
use zeptoclaw::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool};
use zeptoclaw::tools::find::FindTool;
use zeptoclaw::tools::grep::GrepTool;
use zeptoclaw::tools::shell::ShellTool;

#[derive(Deserialize)]
pub struct FixtureFile {
    pub tool: String,
    pub cases: Vec<TestCase>,
}

#[derive(Deserialize)]
pub struct TestCase {
    pub name: String,
    #[serde(default)]
    pub setup: Vec<SetupStep>,
    pub input: Value,
    pub expected: Expected,
}

#[derive(Deserialize)]
pub struct SetupStep {
    #[serde(rename = "type")]
    pub step_type: String,
    pub path: String,
    pub content: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Expected {
    pub is_error: Option<bool>,
    pub error_contains: Option<String>,
    pub output_contains: Option<String>,
    pub output_exact: Option<String>,
    pub file_contains: Option<String>,
    pub file_not_contains: Option<String>,
    pub file_exact: Option<String>,
}

fn tool_by_name(name: &str) -> Box<dyn Tool> {
    match name {
        "edit_file" => Box::new(EditFileTool),
        "read_file" => Box::new(ReadFileTool),
        "shell" => Box::new(ShellTool::default()),
        "grep" => Box::new(GrepTool),
        "find" => Box::new(FindTool),
        "list_dir" => Box::new(ListDirTool),
        _ => panic!("Unknown fixture tool: {}", name),
    }
}

pub async fn run_fixture_file(fixture_path: &str) -> Vec<String> {
    let content = std::fs::read_to_string(fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {}", fixture_path, e));
    let fixture: FixtureFile = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse fixture {}: {}", fixture_path, e));

    let tool = tool_by_name(&fixture.tool);
    let mut failures = Vec::new();

    for case in &fixture.cases {
        let case_name = format!("{}::{}", fixture.tool, case.name);
        if let Err(msg) = run_single_case(&*tool, case).await {
            failures.push(format!("{} failed: {}", case_name, msg));
        }
    }

    failures
}

async fn run_single_case(tool: &dyn Tool, case: &TestCase) -> Result<(), String> {
    let dir = tempdir().map_err(|e| format!("tempdir: {}", e))?;
    let workspace = dir.path().to_str().unwrap();

    // Setup
    for step in &case.setup {
        let full = dir.path().join(&step.path);
        match step.step_type.as_str() {
            "create_file" => {
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent).await
                        .map_err(|e| format!("setup mkdir: {}", e))?;
                }
                let content = step.content.as_deref().unwrap_or("");
                fs::write(&full, content).await
                    .map_err(|e| format!("setup write: {}", e))?;
            }
            "create_dir" => {
                fs::create_dir_all(&full).await
                    .map_err(|e| format!("setup mkdir: {}", e))?;
            }
            other => return Err(format!("unknown setup type: {}", other)),
        }
    }

    let ctx = ToolContext::new().with_workspace(workspace);
    let result = tool.execute(case.input.clone(), &ctx).await;

    // Validate
    let expect_error = case.expected.is_error.unwrap_or(false);

    match (&result, expect_error) {
        (Err(e), true) => {
            if let Some(ref contains) = case.expected.error_contains {
                let msg = format!("{}", e);
                if !msg.contains(contains) {
                    return Err(format!(
                        "error message '{}' does not contain '{}'",
                        msg, contains
                    ));
                }
            }
        }
        (Err(e), false) => {
            return Err(format!("unexpected error: {}", e));
        }
        (Ok(output), true) => {
            if output.is_error {
                // Tool returned is_error=true via ToolOutput (not Err)
                if let Some(ref contains) = case.expected.error_contains {
                    if !output.for_llm.contains(contains) {
                        return Err(format!(
                            "error output '{}' does not contain '{}'",
                            output.for_llm, contains
                        ));
                    }
                }
            } else {
                return Err("expected error but tool succeeded".into());
            }
        }
        (Ok(output), false) => {
            if output.is_error {
                return Err(format!("tool returned is_error: {}", output.for_llm));
            }
            if let Some(ref contains) = case.expected.output_contains {
                if !output.for_llm.contains(contains) {
                    return Err(format!(
                        "output '{}' does not contain '{}'",
                        output.for_llm, contains
                    ));
                }
            }
            if let Some(ref exact) = case.expected.output_exact {
                if output.for_llm != *exact {
                    return Err(format!(
                        "output '{}' != expected '{}'",
                        output.for_llm, exact
                    ));
                }
            }
        }
    }

    // File assertions (only if we have them)
    if case.expected.file_contains.is_some()
        || case.expected.file_not_contains.is_some()
        || case.expected.file_exact.is_some()
    {
        // Determine which file to check — use the "path" from input
        if let Some(path) = case.input.get("path").and_then(|v| v.as_str()) {
            let full = dir.path().join(path);
            let file_content = fs::read_to_string(&full).await
                .map_err(|e| format!("reading file '{}' for assertion: {}", path, e))?;

            if let Some(ref contains) = case.expected.file_contains {
                if !file_content.contains(contains) {
                    return Err(format!(
                        "file '{}' does not contain '{}'",
                        path, contains
                    ));
                }
            }
            if let Some(ref not_contains) = case.expected.file_not_contains {
                if file_content.contains(not_contains) {
                    return Err(format!(
                        "file '{}' should not contain '{}' but does",
                        path, not_contains
                    ));
                }
            }
            if let Some(ref exact) = case.expected.file_exact {
                if file_content != *exact {
                    return Err(format!(
                        "file '{}' content '{}' != expected '{}'",
                        path, file_content, exact
                    ));
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Create `tests/conformance/mod.rs`**

```rust
pub mod runner;
```

- [ ] **Step 3: Create `tests/conformance.rs` entry point**

```rust
mod conformance;

use conformance::runner::run_fixture_file;

macro_rules! fixture_test {
    ($name:ident, $path:expr) => {
        #[tokio::test]
        async fn $name() {
            let failures = run_fixture_file($path).await;
            if !failures.is_empty() {
                panic!(
                    "\n{} conformance failure(s):\n  - {}\n",
                    failures.len(),
                    failures.join("\n  - ")
                );
            }
        }
    };
}

fixture_test!(conformance_edit_tool, "tests/conformance/fixtures/edit_tool.json");
fixture_test!(conformance_shell_tool, "tests/conformance/fixtures/shell_tool.json");
fixture_test!(conformance_read_tool, "tests/conformance/fixtures/read_tool.json");
fixture_test!(conformance_grep_tool, "tests/conformance/fixtures/grep_tool.json");
fixture_test!(conformance_find_tool, "tests/conformance/fixtures/find_tool.json");
```

- [ ] **Step 4: Verify runner compiles**

Run: `cargo check --tests`
Expected: Compiles (no fixture files yet, but the macro won't fail until runtime)

- [ ] **Step 5: Commit**

```bash
git add tests/conformance.rs tests/conformance/
git commit -m "feat(tests): add conformance fixture runner infrastructure

JSON fixture-based testing for tools. Each fixture defines setup,
input, and expected output. Runner collects all failures per fixture
file for better CI diagnostics.

Part of #391"
```

---

### Task 7: Write fixture files

**Files:**
- Create: `tests/conformance/fixtures/edit_tool.json`
- Create: `tests/conformance/fixtures/shell_tool.json`
- Create: `tests/conformance/fixtures/read_tool.json`
- Create: `tests/conformance/fixtures/grep_tool.json`
- Create: `tests/conformance/fixtures/find_tool.json`

- [ ] **Step 1: Create `tests/conformance/fixtures/` directory**

```bash
mkdir -p tests/conformance/fixtures
```

- [ ] **Step 2: Write `edit_tool.json`**

Cover: exact single, exact multi-match error, not found, empty old_text, expected_replacements match, expected_replacements mismatch, whitespace tabs-vs-spaces, whitespace trailing, CRLF, fuzzy multi-match error, diff mode, file_not_contains after replacement.

Write 12 cases. Each case has `setup`, `input`, `expected`. See spec for format.

- [ ] **Step 3: Write `shell_tool.json`**

Cover: simple echo, large output truncation by lines (generate >2000 lines via `seq`), large output truncation by bytes, command failure (exit code), empty output.

Write 5 cases. Note: avoid commands on the shell blocklist.

- [ ] **Step 4: Write `read_tool.json`**

Cover: read existing file, read nonexistent file error, read file with unicode, read empty file, read large file with truncation.

Write 5 cases.

- [ ] **Step 5: Write `grep_tool.json`**

Cover: basic match, no match, pattern in multiple files, case-insensitive with flag, glob filter.

Write 5 cases.

- [ ] **Step 6: Write `find_tool.json`**

Cover: basic glob match, no match, nested directory, limit parameter, multiple matches.

Write 5 cases.

- [ ] **Step 7: Run all conformance tests**

Run: `cargo nextest run --test conformance`
Expected: All 5 fixture tests PASS

- [ ] **Step 8: Commit**

```bash
git add tests/conformance/fixtures/
git commit -m "feat(tests): add conformance fixture files for 5 tools

12 edit cases (exact, fuzzy, error paths), 4 shell cases (truncation,
errors), 3 each for read/grep/find. All passing.

Part of #391"
```

---

### Task 8: Run full test suite and finalize

**Files:**
- None new

- [ ] **Step 1: Run full pre-push checklist**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo nextest run --lib && cargo test --doc && cargo fmt -- --check
```

Expected: All pass, no warnings, no format issues.

- [ ] **Step 2: Run conformance tests specifically**

```bash
cargo nextest run --test conformance
```

Expected: All 5 PASS.

- [ ] **Step 3: Run full integration tests**

```bash
cargo nextest run --test integration
```

Expected: All PASS.

- [ ] **Step 4: Final commit (if any fixups needed)**

Fix any clippy/fmt issues from subagent work and commit:

```bash
git add -A && git commit -m "chore: fmt + clippy fixups for #391"
```

---

## Task Dependency Graph

```
Task 1 (shared truncation) ─────→ Task 2 (apply to tools) ──┐
                                                              ├──→ Task 7 (fixture files) ──→ Task 8 (finalize)
Task 3 (unicode-normalization) ──→ Task 4 (find_unique_match) ──→ Task 5 (wire into EditFileTool) ──┘
                                                              │
Task 6 (fixture runner) ─────────────────────────────────────┘
```

Tasks 1+3+6 can all run in parallel (no deps between them). Task 7 depends on Tasks 2, 5, AND 6 (fixtures test the new behavior with the runner infrastructure). Task 8 runs last.
