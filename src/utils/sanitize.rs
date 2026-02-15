//! Tool result sanitization.
//!
//! Strips base64 data URIs, long hex blobs, and truncates oversized
//! results before feeding them back to the LLM. This saves tokens
//! without losing meaningful information.

use once_cell::sync::Lazy;
use regex::Regex;

/// Default maximum result size in bytes (50 KB).
pub const DEFAULT_MAX_RESULT_BYTES: usize = 51_200;

/// Minimum tool result budget in bytes (1 KB).
pub const MIN_RESULT_BUDGET: usize = 1024;

/// Approximate bytes per token for budget estimation.
const BYTES_PER_TOKEN: usize = 4;

/// Minimum length of a contiguous hex string to be stripped.
const MIN_HEX_BLOB_LEN: usize = 200;

static BASE64_URI_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"data:[a-zA-Z0-9/+\-\.]+;base64,[A-Za-z0-9+/=]+").unwrap());

static HEX_BLOB_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"[0-9a-fA-F]{{{},}}", MIN_HEX_BLOB_LEN)).unwrap());

/// Sanitize a tool result string.
///
/// 1. Replace `data:...;base64,...` URIs with a placeholder.
/// 2. Replace hex blobs (>= 200 hex chars) with a placeholder.
/// 3. Truncate to `max_bytes` if still too large.
pub fn sanitize_tool_result(result: &str, max_bytes: usize) -> String {
    let mut out = BASE64_URI_RE
        .replace_all(result, |caps: &regex::Captures| {
            let len = caps[0].len();
            format!("[base64 data removed, {} bytes]", len)
        })
        .into_owned();

    out = HEX_BLOB_RE
        .replace_all(&out, |caps: &regex::Captures| {
            let len = caps[0].len();
            format!("[hex data removed, {} chars]", len)
        })
        .into_owned();

    if out.len() > max_bytes {
        let total = out.len();
        out.truncate(max_bytes);
        // Ensure we don't split a multi-byte char
        while !out.is_char_boundary(out.len()) {
            out.pop();
        }
        out.push_str(&format!("\n...[truncated, {} total bytes]", total));
    }

    out
}

/// Compute a dynamic tool result byte budget based on remaining context capacity.
///
/// The budget scales with available context space:
/// - Takes the remaining token capacity (context_limit - current_usage)
/// - Converts tokens to approximate bytes (multiply by 4)
/// - Divides by the number of pending results to share budget fairly
/// - Clamps to [`MIN_RESULT_BUDGET`, `DEFAULT_MAX_RESULT_BYTES`]
///
/// # Arguments
/// * `context_limit` - Maximum token capacity of the context window
/// * `current_usage_tokens` - Current estimated token usage
/// * `pending_result_count` - Number of tool results about to be inserted
///
/// # Returns
/// The byte budget for each tool result.
pub fn compute_tool_result_budget(
    context_limit: usize,
    current_usage_tokens: usize,
    pending_result_count: usize,
) -> usize {
    let remaining_tokens = context_limit.saturating_sub(current_usage_tokens);
    let remaining_bytes = remaining_tokens * BYTES_PER_TOKEN;
    let count = pending_result_count.max(1);
    let per_result = remaining_bytes / count;
    per_result.clamp(MIN_RESULT_BUDGET, DEFAULT_MAX_RESULT_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_change_for_normal_text() {
        let input = "Hello, world! This is a normal tool result.";
        assert_eq!(sanitize_tool_result(input, DEFAULT_MAX_RESULT_BYTES), input);
    }

    #[test]
    fn test_strips_base64_data_uri() {
        let b64 = "A".repeat(500);
        let input = format!("before data:image/png;base64,{} after", b64);
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(!result.contains(&b64));
        assert!(result.contains("[base64 data removed,"));
        assert!(result.contains("before"));
        assert!(result.contains("after"));
    }

    #[test]
    fn test_strips_hex_blob() {
        let hex = "a1b2c3d4e5f6".repeat(40); // 480 hex chars
        let input = format!("prefix {} suffix", hex);
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(!result.contains(&hex));
        assert!(result.contains("[hex data removed,"));
        assert!(result.contains("prefix"));
        assert!(result.contains("suffix"));
    }

    #[test]
    fn test_short_hex_not_stripped() {
        let hex = "abcdef1234"; // 10 chars, below threshold
        let input = format!("hash: {}", hex);
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(result.contains(hex));
    }

    #[test]
    fn test_truncation() {
        let input = "x".repeat(1000);
        let result = sanitize_tool_result(&input, 100);
        assert!(result.len() < 200); // 100 + truncation message
        assert!(result.contains("[truncated, 1000 total bytes]"));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(sanitize_tool_result("", DEFAULT_MAX_RESULT_BYTES), "");
    }

    #[test]
    fn test_multiple_base64_uris() {
        let b64 = "Q".repeat(100);
        let input = format!(
            "img1: data:image/png;base64,{} and img2: data:application/pdf;base64,{}",
            b64, b64
        );
        let result = sanitize_tool_result(&input, DEFAULT_MAX_RESULT_BYTES);
        assert!(!result.contains(&b64));
        // Should have two replacement markers
        assert_eq!(result.matches("[base64 data removed,").count(), 2);
    }

    // --- compute_tool_result_budget tests ---

    #[test]
    fn test_compute_budget_plenty_of_space() {
        // 100k limit, 10k used => 90k remaining => 90k * 4 = 360k bytes
        // Single result => 360k, clamped to DEFAULT_MAX_RESULT_BYTES (50KB)
        let budget = compute_tool_result_budget(100_000, 10_000, 1);
        assert_eq!(budget, DEFAULT_MAX_RESULT_BYTES);
    }

    #[test]
    fn test_compute_budget_tight_space() {
        // 100k limit, 99_000 used => 1000 remaining => 1000 * 4 = 4000 bytes
        // Single result => 4000 bytes
        let budget = compute_tool_result_budget(100_000, 99_000, 1);
        assert_eq!(budget, 4000);
        assert!(budget > MIN_RESULT_BUDGET);
        assert!(budget < DEFAULT_MAX_RESULT_BYTES);
    }

    #[test]
    fn test_compute_budget_no_space() {
        // Usage >= limit => 0 remaining => clamped to MIN_RESULT_BUDGET
        let budget = compute_tool_result_budget(100_000, 100_000, 1);
        assert_eq!(budget, MIN_RESULT_BUDGET);

        // Usage exceeds limit
        let budget = compute_tool_result_budget(100_000, 120_000, 1);
        assert_eq!(budget, MIN_RESULT_BUDGET);
    }

    #[test]
    fn test_compute_budget_multiple_results() {
        // 100k limit, 90k used => 10k remaining => 10k * 4 = 40k bytes
        // 4 results => 40k / 4 = 10k each
        let budget = compute_tool_result_budget(100_000, 90_000, 4);
        assert_eq!(budget, 10_000);
    }

    #[test]
    fn test_compute_budget_single_result() {
        // 100k limit, 95_000 used => 5k remaining => 5k * 4 = 20k bytes
        // 1 result => 20k
        let budget = compute_tool_result_budget(100_000, 95_000, 1);
        assert_eq!(budget, 20_000);
    }

    #[test]
    fn test_compute_budget_zero_results() {
        // pending_result_count=0 should not panic; treated as 1
        let budget = compute_tool_result_budget(100_000, 50_000, 0);
        // 50k remaining => 50k * 4 = 200k bytes / 1 => clamped to 51_200
        assert_eq!(budget, DEFAULT_MAX_RESULT_BYTES);
    }

    #[test]
    fn test_compute_budget_never_below_minimum() {
        // Even with very little space and many results, never below MIN
        // 1000 limit, 999 used => 1 remaining => 1 * 4 = 4 bytes / 10 results = 0
        // Clamped to MIN_RESULT_BUDGET
        let budget = compute_tool_result_budget(1000, 999, 10);
        assert_eq!(budget, MIN_RESULT_BUDGET);

        // Zero remaining, many results
        let budget = compute_tool_result_budget(1000, 1000, 100);
        assert_eq!(budget, MIN_RESULT_BUDGET);
    }
}
