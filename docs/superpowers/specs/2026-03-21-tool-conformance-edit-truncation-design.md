# Tool Conformance Fixtures, Edit Fuzzy Matching, Output Truncation

**Issue:** #391
**Date:** 2026-03-21
**Status:** Draft
**Approach:** B — Shared utilities + fixture harness

## Context

Cherry-picked from pi_agent_rust evaluation. Three improvements to ZeptoClaw's coding tools:
1. Fixture-based conformance testing for tool regression protection
2. Fuzzy edit matching to reduce failed edits from LLM whitespace/Unicode hallucination
3. Consistent output truncation across all tools

## 1. Edit Fuzzy Matching

### Current behavior

`EditFileTool` in `src/tools/filesystem.rs` uses exact `content.matches(old_text).count()` to find matches, then `content.replace(old_text, new_text)` to apply. Optional `expected_replacements` guard exists. Rejects empty `old_text`.

### New behavior

Replace the match+replace logic with a `find_unique_match()` function that tries three tiers:

1. **Exact match** — `content.find(old_text)`. If exactly 1 match, use it. If 2+, error with count and actionable message.
2. **Unicode-normalized match** — NFC-normalize both content and old_text, find match, map byte offset back to original string. Catches composed vs decomposed characters (e.g. `é` vs `e` + combining accent).
3. **Whitespace-normalized match** — Collapse whitespace runs to single space, trim trailing whitespace per line. Catches tabs-vs-spaces and trailing whitespace from LLM output.

Each tier fires only if the previous tier found 0 matches. If any tier finds 2+ matches, error immediately.

Replacement always happens on the **original content** using the mapped byte range — normalized content is never written to disk.

`expected_replacements` guards exact-match tier only. Fuzzy tiers already enforce uniqueness (exactly 1 match required), so `expected_replacements` is only meaningful for the exact tier where multiple matches could exist. When `expected_replacements` is provided and matches the exact count, the tool performs all replacements (preserving current multi-match-with-guard behavior). When `expected_replacements` is absent and exact matches > 1, the tool errors.

**Breaking change:** Multi-match exact edits that previously succeeded without `expected_replacements` will now error. Users/LLMs must either provide more specific `old_text` or pass `expected_replacements` to opt into multi-match replacement.

### Whitespace normalization scope

The whitespace tier normalizes:
- Runs of spaces/tabs within lines → single space
- Trailing whitespace per line
- `\r\n` → `\n` (Windows line endings)

Deliberately NOT normalized (too loose, risk of false positives):
- Leading indentation differences
- Extra/missing blank lines
- Differences in indentation level

### Error messages

- 0 matches (all tiers exhausted): `"Text '{preview}' not found in file '{path}'"`
- 2+ matches: `"Found {n} occurrences of text in '{path}'. Provide more surrounding context to uniquely identify the location."`

### Function signature

```rust
enum MatchTier { Exact, UnicodeNormalized, WhitespaceNormalized }

struct UniqueMatch {
    start: usize,        // byte offset in original content
    end: usize,          // byte offset in original content
    tier: MatchTier,     // which tier matched
}

/// Returns a single unique match. For multi-match with guard, the caller
/// checks `expected_replacements` before calling this and uses
/// `content.matches().count()` + `content.replace()` instead.
fn find_unique_match(content: &str, old_text: &str) -> Result<UniqueMatch, EditMatchError>
```

When `expected_replacements` is provided, the existing `content.matches(old_text).count()` + `content.replace()` path is used (exact tier only, no fuzzy fallback). `find_unique_match()` is only called when `expected_replacements` is absent.

### Dependencies

- `unicode-normalization` crate for NFC — must be added to `[dependencies]` in `Cargo.toml` (present in `Cargo.lock` as transitive dep of `idna` but not directly usable)

### Location

`find_unique_match()` lives in `src/tools/filesystem.rs` as a private function. No new files needed.

## 2. Output Truncation

### Current state

- `shell.rs`: Private `truncate_formatted_output()` with `MAX_OUTPUT_LINES = 2000`, `MAX_OUTPUT_BYTES = 50_000`
- `docx_read.rs` / `pdf_read.rs`: Own `truncate_output()` methods (grapheme-aware, character-level)
- `grep.rs`: 100-match default limit (user-overridable via `limit` param)
- `find.rs`: 200-result default limit (user-overridable via `limit` param)
- `custom.rs`: 50KB byte truncation
- `read_file`, `list_dir`: No limits

### Design

Extract shell's truncation logic into a shared utility.

**New file:** `src/tools/output.rs`

```rust
pub const DEFAULT_MAX_LINES: usize = 2_000;
pub const DEFAULT_MAX_BYTES: usize = 50_000;

/// Truncate text output by line count and byte count.
/// Char-boundary-safe. Appends truncation reason if truncated.
pub fn truncate_tool_output(output: &str, max_lines: usize, max_bytes: usize) -> String
```

Same logic as shell's current `truncate_formatted_output()`: iterate lines, track byte count, break on either limit, append `"... [output truncated at {reason}]"`.

**Application:**

| Tool | Change |
|------|--------|
| `ShellTool` | Replace private function + constants with `output::truncate_tool_output(DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES)` |
| `ReadFileTool` | Wrap `for_llm` output in `truncate_tool_output()` |
| `GrepTool` | Keep 100-match default limit. Add `truncate_tool_output()` on final string |
| `FindTool` | Keep 200-result default limit. Add `truncate_tool_output()` on final string |
| `ListDirTool` | Wrap `for_llm` output in `truncate_tool_output()` |

**Not changed:**
- `DocxReadTool` / `PdfReadTool` — grapheme-aware truncation is a different concern, keep as-is
- `CustomTool` — own 50KB path, different code path
- `HttpRequestTool` — configurable byte limit, different code path

**No config knob.** Constants only. Config can be added later if users request it.

### Module wiring

Add `pub mod output;` to `src/tools/mod.rs`.

## 3. Conformance Fixture Testing

### Structure

```
tests/
├── conformance.rs                  # Top-level test entry point
└── conformance/
    ├── mod.rs                      # Re-exports
    ├── runner.rs                   # Generic fixture runner
    └── fixtures/
        ├── edit_tool.json          # 12-15 cases
        ├── shell_tool.json         # 5 cases
        ├── read_tool.json          # 5 cases
        ├── grep_tool.json          # 5 cases
        └── find_tool.json          # 5 cases
```

### Fixture format

```json
{
  "tool": "edit_file",
  "cases": [
    {
      "name": "exact_match_single",
      "setup": [
        { "type": "create_file", "path": "test.rs", "content": "fn main() {}" }
      ],
      "input": { "path": "test.rs", "old_text": "main", "new_text": "run" },
      "expected": {
        "is_error": false,
        "output_contains": "Successfully replaced 1 occurrence"
      }
    }
  ]
}
```

### Data types

```rust
#[derive(Deserialize)]
struct FixtureFile {
    tool: String,
    cases: Vec<TestCase>,
}

#[derive(Deserialize)]
struct TestCase {
    name: String,
    setup: Vec<SetupStep>,
    input: Value,
    expected: Expected,
}

#[derive(Deserialize)]
struct SetupStep {
    #[serde(rename = "type")]
    step_type: String,     // "create_file" | "create_dir"
    path: String,
    content: Option<String>,
}

#[derive(Deserialize)]
struct Expected {
    is_error: Option<bool>,
    error_contains: Option<String>,
    output_contains: Option<String>,
    output_exact: Option<String>,
    file_contains: Option<String>,
    file_not_contains: Option<String>,
    file_exact: Option<String>,
}
```

### Runner logic

1. Deserialize fixture JSON into `FixtureFile`
2. For each case:
   a. Create tempdir
   b. Execute setup steps (create files/dirs)
   c. Map `fixture.tool` → tool instance via `tool_by_name()` match block
   d. Build `ToolContext` with tempdir as workspace
   e. Call `tool.execute(case.input, &ctx).await`
   f. Validate against `case.expected` fields (all optional, multiple combine)
3. Each case runs independently — collect all failures, report at end for better CI diagnostics. Format: `"{tool}::{case_name} failed: {reason}"`

### Tool mapping

```rust
fn tool_by_name(name: &str) -> Box<dyn Tool> {
    match name {
        "edit_file" => Box::new(EditFileTool),
        "read_file" => Box::new(ReadFileTool),
        "shell" => Box::new(ShellTool::default()), // runs with default security; test cases should respect blocklist
        "grep" => Box::new(GrepTool),
        "find" => Box::new(FindTool),
        "list_dir" => Box::new(ListDirTool),
        _ => panic!("Unknown fixture tool: {}", name),
    }
}
```

### Run command

```bash
cargo nextest run --test conformance
```

### Initial fixture coverage

**edit_tool.json (12-15 cases):**
- Exact single match
- Exact multi-match → error with count
- Text not found → error
- Empty old_text → error
- expected_replacements match
- expected_replacements mismatch → error
- Unicode normalization fallback (composed vs decomposed)
- Whitespace normalization fallback (tabs vs spaces)
- Whitespace normalization fallback (trailing whitespace)
- Fuzzy match with 2+ normalized matches → error
- Unified diff mode (basic hunk)
- Path traversal → error

**shell_tool.json (5 cases):**
- Simple command output
- Output exceeding 2000 lines → truncated
- Output exceeding 50KB → truncated
- Command failure → is_error
- Empty command output

**read_tool.json, grep_tool.json, find_tool.json (5 cases each):**
- Basic success case
- File/pattern not found
- Truncation behavior (read/grep)
- Error paths

## Files changed

| File | Change |
|------|--------|
| `src/tools/output.rs` | **New** — shared `truncate_tool_output()` |
| `src/tools/mod.rs` | Add `pub mod output;` |
| `src/tools/filesystem.rs` | Add `find_unique_match()`, update `EditFileTool::execute()` to use it, change multi-match to error |
| `src/tools/shell.rs` | Remove private truncation, import shared |
| `src/tools/grep.rs` | Add `truncate_tool_output()` call |
| `src/tools/find.rs` | Add `truncate_tool_output()` call |
| `tests/conformance.rs` | **New** — test entry point |
| `tests/conformance/mod.rs` | **New** — re-exports |
| `tests/conformance/runner.rs` | **New** — fixture runner |
| `tests/conformance/fixtures/*.json` | **New** — 5 fixture files |
| `Cargo.toml` | Add `unicode-normalization` to `[dependencies]` |

## Dependencies

- `unicode-normalization` — NFC normalization for edit fuzzy matching
- `serde_json` — already present, for fixture deserialization

## What this does NOT include

- Config knobs for truncation limits
- RPC/headless coding mode (separate future work)
- Session branching
- AST-aware editing or language-specific refactoring
- Changes to DocxRead/PdfRead/CustomTool/HttpRequest truncation
