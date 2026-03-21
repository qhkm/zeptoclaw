//! Shared output truncation utilities for tool results.
//!
//! Tools that produce potentially large output (shell commands, file reads, etc.)
//! should use [`truncate_tool_output`] to cap output size before returning it to
//! the LLM. This prevents context window exhaustion from runaway commands.

/// Default maximum number of lines before truncation.
pub const DEFAULT_MAX_LINES: usize = 2_000;

/// Default maximum number of bytes before truncation.
pub const DEFAULT_MAX_BYTES: usize = 50_000;

/// Truncate tool output that exceeds either a line count or byte count limit.
///
/// Iterates lines preserving original line endings (via `split_inclusive('\n')`),
/// tracking both line count and cumulative byte length. When either limit is hit
/// the output is cut and a `"... [output truncated at {reason}]"` trailer is
/// appended.
///
/// Byte cutting is char-boundary-safe: if the byte limit falls inside a
/// multi-byte character, the cut point is moved backward to the nearest
/// boundary.
///
/// # Arguments
///
/// * `output` - The raw tool output string.
/// * `max_lines` - Maximum number of lines to keep.
/// * `max_bytes` - Maximum number of bytes to keep.
///
/// # Examples
///
/// ```
/// use zeptoclaw::tools::output::{truncate_tool_output, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES};
///
/// let small = "hello\nworld\n";
/// assert_eq!(truncate_tool_output(small, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES), small);
///
/// let big = "x\n".repeat(3);
/// let truncated = truncate_tool_output(&big, 2, 1000);
/// assert!(truncated.contains("[output truncated"));
/// ```
pub fn truncate_tool_output(output: &str, max_lines: usize, max_bytes: usize) -> String {
    if output.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    let mut byte_count: usize = 0;

    for (line_index, segment) in output.split_inclusive('\n').enumerate() {
        let segment_bytes = segment.len();

        // Check line limit (each segment from split_inclusive is one line)
        if line_index >= max_lines {
            let remaining_lines = output.split_inclusive('\n').count() - line_index;
            result.push_str(&format!(
                "... [output truncated at {max_lines} lines, {remaining_lines} lines omitted]"
            ));
            return result;
        }

        // Check byte limit
        if byte_count + segment_bytes > max_bytes {
            // How many bytes we can still take from this segment
            let remaining_budget = max_bytes.saturating_sub(byte_count);
            if remaining_budget > 0 {
                // Walk backward to a char boundary within the budget
                let mut end = remaining_budget;
                while end > 0 && !segment.is_char_boundary(end) {
                    end -= 1;
                }
                if end > 0 {
                    result.push_str(&segment[..end]);
                }
            }
            let total_bytes = output.len();
            let kept_bytes = byte_count + remaining_budget;
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&format!(
                "... [output truncated at {max_bytes} bytes, {} bytes omitted]",
                total_bytes.saturating_sub(kept_bytes)
            ));
            return result;
        }

        result.push_str(segment);
        byte_count += segment_bytes;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_truncation_small_input() {
        let input = "line 1\nline 2\nline 3\n";
        let result = truncate_tool_output(input, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert_eq!(result, input);
    }

    #[test]
    fn truncate_by_lines() {
        // 10 lines, limit to 3
        let input: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        let result = truncate_tool_output(&input, 3, DEFAULT_MAX_BYTES);
        assert!(result.starts_with("line 1\nline 2\nline 3\n"));
        assert!(result.contains("[output truncated at 3 lines"));
        assert!(result.contains("7 lines omitted"));
        // Must NOT contain line 4+
        assert!(!result.contains("line 4\n"));
    }

    #[test]
    fn truncate_by_bytes() {
        // Each 'x' line is 2 bytes ("x\n"), build 100 of them = 200 bytes
        let input = "x\n".repeat(100);
        let result = truncate_tool_output(&input, DEFAULT_MAX_LINES, 10);
        assert!(result.contains("[output truncated at 10 bytes"));
        // Result body (before the trailer) should not exceed the byte limit
        let before_marker = result.split("... [output truncated").next().unwrap();
        assert!(before_marker.len() <= 10);
    }

    #[test]
    fn empty_input() {
        let result = truncate_tool_output("", 100, 100);
        assert_eq!(result, "");
    }

    #[test]
    fn char_boundary_safety() {
        // Multi-byte chars: each is 4 bytes, total 16 bytes
        let input = "\u{1F600}\u{1F600}\u{1F600}\u{1F600}";
        assert_eq!(input.len(), 16);

        // Byte limit 6 falls inside the second emoji (bytes 4..8).
        // The function must NOT panic, and should cut cleanly at a boundary.
        let result = truncate_tool_output(input, DEFAULT_MAX_LINES, 6);
        assert!(result.contains("[output truncated"));
        // The kept portion must be valid UTF-8 (implicit -- we got here without panic)
        // and should contain exactly one emoji (4 bytes <= budget of 6)
        let before_trailer = result.split("\n... [output truncated").next().unwrap();
        assert_eq!(before_trailer, "\u{1F600}");
    }

    #[test]
    fn both_limits_hit() {
        // 5 lines, each "abcdefghij\n" = 11 bytes per line = 55 bytes total
        let input: String = (1..=5).map(|_| "abcdefghij\n").collect();
        assert_eq!(input.len(), 55);

        // Line limit 3 -> would keep 33 bytes (3 * 11)
        // Byte limit 25 -> would cut partway through line 3
        // Byte limit 25 is more restrictive, so byte truncation fires first
        let result = truncate_tool_output(&input, 3, 25);
        assert!(result.contains("[output truncated at 25 bytes"));

        // Now test the opposite: line limit fires first
        // Byte limit 100 (permissive), line limit 2
        let result2 = truncate_tool_output(&input, 2, 100);
        assert!(result2.contains("[output truncated at 2 lines"));
    }
}
