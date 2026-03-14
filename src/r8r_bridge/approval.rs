//! Approval routing and response parsing for r8r bridge.
//!
//! Provides:
//! - [`ApprovalDecision`] — parsed result of a user's approval reply.
//! - [`parse_approval_response`] — convert free-text into an `ApprovalDecision`.
//! - [`format_approval_message`] — channel message asking a user to approve/reject.
//! - [`format_timeout_message`] — channel message notifying that approval expired.
//! - [`format_help_message`] — brief help for unrecognized replies.

/// A parsed approval decision returned by [`parse_approval_response`].
pub struct ApprovalDecision {
    /// Either `"approved"` or `"rejected"`.
    pub decision: String,
    /// Free-text reason provided by the user; empty string when none was given.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Keyword tables
// ---------------------------------------------------------------------------

const APPROVE_KEYWORDS: &[&str] = &["approve", "approved", "yes", "y", "lgtm", "ok"];
const REJECT_KEYWORDS: &[&str] = &["reject", "rejected", "no", "n", "deny", "denied"];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a user's free-text reply into an [`ApprovalDecision`].
///
/// The first whitespace-delimited token is matched case-insensitively against
/// the approve / reject keyword tables.  Any remaining text becomes the reason.
///
/// Returns `None` when the input is empty or the first token is not recognised.
pub fn parse_approval_response(text: &str) -> Option<ApprovalDecision> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // Split on the first whitespace boundary.
    let (first, rest) = match text.find(char::is_whitespace) {
        Some(idx) => (&text[..idx], text[idx..].trim()),
        None => (text, ""),
    };

    let keyword = first.to_lowercase();

    if APPROVE_KEYWORDS.contains(&keyword.as_str()) {
        return Some(ApprovalDecision {
            decision: "approved".to_string(),
            reason: rest.to_string(),
        });
    }

    if REJECT_KEYWORDS.contains(&keyword.as_str()) {
        return Some(ApprovalDecision {
            decision: "rejected".to_string(),
            reason: rest.to_string(),
        });
    }

    None
}

/// Format a channel message requesting approval for a workflow execution.
///
/// ```text
/// [r8r] Approval needed for {workflow} ({short_id})
/// > {message}
/// Reply: approve / reject [reason]
/// ```
///
/// `short_id` is the first 8 characters of `execution_id`.
pub fn format_approval_message(workflow: &str, execution_id: &str, message: &str) -> String {
    let short_id = short_id(execution_id);
    format!(
        "[r8r] Approval needed for {workflow} ({short_id})\n> {message}\nReply: approve / reject [reason]"
    )
}

/// Format a channel message notifying that an approval window has expired.
///
/// ```text
/// [r8r] Approval for {workflow} ({short_id}) expired after {elapsed}.
/// Re-run the workflow to try again.
/// ```
///
/// Elapsed formatting:
/// - `>= 3600 s` → `"Xh Ym"`
/// - `>= 60 s`   → `"Xm"`
/// - otherwise   → `"Xs"`
pub fn format_timeout_message(workflow: &str, execution_id: &str, elapsed_secs: u64) -> String {
    let short_id = short_id(execution_id);
    let elapsed = format_elapsed(elapsed_secs);
    format!(
        "[r8r] Approval for {workflow} ({short_id}) expired after {elapsed}.\nRe-run the workflow to try again."
    )
}

/// Return a brief help string for unrecognised replies.
pub fn format_help_message() -> String {
    "I didn't understand that. Reply **approve** or **reject [reason]**.".to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the first 8 characters of `execution_id` (or the whole string if
/// it is shorter than 8 characters).
fn short_id(execution_id: &str) -> &str {
    let end = execution_id
        .char_indices()
        .nth(8)
        .map(|(i, _)| i)
        .unwrap_or(execution_id.len());
    &execution_id[..end]
}

/// Format a duration in seconds as a human-readable elapsed string.
fn format_elapsed(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        format!("{h}h {m}m")
    } else if secs >= 60 {
        let m = secs / 60;
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

// ---------------------------------------------------------------------------
// Unit tests (inline)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_id_truncates() {
        assert_eq!(short_id("abcdefgh12345678"), "abcdefgh");
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id(""), "");
    }

    #[test]
    fn test_format_elapsed() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(45), "45s");
        assert_eq!(format_elapsed(60), "1m");
        assert_eq!(format_elapsed(90), "1m");
        assert_eq!(format_elapsed(3600), "1h 0m");
        assert_eq!(format_elapsed(3661), "1h 1m");
        assert_eq!(format_elapsed(7384), "2h 3m");
    }
}
