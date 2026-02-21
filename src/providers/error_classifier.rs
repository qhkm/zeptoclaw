//! Pattern-based error classification for LLM provider errors.
//!
//! Checks patterns in priority order: billing > auth > rate_limit > overloaded > timeout > format.
//! Falls back to `Unknown` if no pattern matches.

use crate::error::ProviderError;

/// Classify an error message string into a `ProviderError`.
pub fn classify_error_message(msg: &str) -> ProviderError {
    let lower = msg.to_lowercase();

    // Billing — check before auth (402 can appear in both)
    if contains_any(
        &lower,
        &[
            "402",
            "payment required",
            "insufficient credits",
            "credit balance",
            "plans & billing",
            "insufficient balance",
            "billing",
        ],
    ) {
        return ProviderError::Billing(msg.to_string());
    }

    // Auth
    if contains_any(
        &lower,
        &[
            "invalid_api_key",
            "invalid api key",
            "incorrect api key",
            "invalid token",
            "authentication",
            "re-authenticate",
            "oauth token refresh failed",
            "unauthorized",
            "forbidden",
            "access denied",
            "expired",
            "token has expired",
            "401",
            "403",
            "no credentials found",
            "no api key found",
        ],
    ) {
        return ProviderError::Auth(msg.to_string());
    }

    // Rate limit
    if contains_any(
        &lower,
        &[
            "rate_limit",
            "rate limit",
            "too many requests",
            "429",
            "exceeded your current quota",
            "resource has been exhausted",
            "resource_exhausted",
            "quota exceeded",
            "usage limit",
        ],
    ) {
        return ProviderError::RateLimit(msg.to_string());
    }

    // Overloaded (Anthropic-specific JSON body pattern)
    if contains_any(
        &lower,
        &[
            "overloaded_error",
            "\"type\":\"overloaded_error\"",
            "overloaded",
        ],
    ) {
        return ProviderError::Overloaded(msg.to_string());
    }

    // Timeout
    if contains_any(
        &lower,
        &[
            "timeout",
            "timed out",
            "deadline exceeded",
            "context deadline exceeded",
        ],
    ) {
        return ProviderError::Timeout(msg.to_string());
    }

    // Format (non-retriable request structure errors)
    if contains_any(
        &lower,
        &[
            "string should match pattern",
            "tool_use.id",
            "tool_use_id",
            "messages.1.content.1.tool_use.id",
            "invalid request format",
        ],
    ) {
        return ProviderError::Format(msg.to_string());
    }

    ProviderError::Unknown(msg.to_string())
}

fn contains_any(haystack: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| haystack.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_429() {
        let e = classify_error_message("HTTP 429: Too many requests");
        assert!(matches!(e, ProviderError::RateLimit(_)));
    }

    #[test]
    fn test_rate_limit_quota() {
        let e = classify_error_message("You exceeded your current quota");
        assert!(matches!(e, ProviderError::RateLimit(_)));
    }

    #[test]
    fn test_overloaded_anthropic_body() {
        let e = classify_error_message(r#"{"type":"overloaded_error","message":"overloaded"}"#);
        assert!(matches!(e, ProviderError::Overloaded(_)));
    }

    #[test]
    fn test_billing_402() {
        let e = classify_error_message("HTTP 402: payment required");
        assert!(matches!(e, ProviderError::Billing(_)));
    }

    #[test]
    fn test_billing_insufficient_credits() {
        let e = classify_error_message("Insufficient credits in your account");
        assert!(matches!(e, ProviderError::Billing(_)));
    }

    #[test]
    fn test_auth_invalid_key() {
        let e = classify_error_message("invalid_api_key: The API key is invalid");
        assert!(matches!(e, ProviderError::Auth(_)));
    }

    #[test]
    fn test_auth_401() {
        let e = classify_error_message("HTTP 401: unauthorized");
        assert!(matches!(e, ProviderError::Auth(_)));
    }

    #[test]
    fn test_timeout() {
        let e = classify_error_message("request timed out after 120s");
        assert!(matches!(e, ProviderError::Timeout(_)));
    }

    #[test]
    fn test_format_tool_use_id() {
        let e =
            classify_error_message("messages.1.content.1.tool_use.id: string should match pattern");
        assert!(matches!(e, ProviderError::Format(_)));
    }

    #[test]
    fn test_unknown_fallback() {
        let e = classify_error_message("something completely unrecognized happened");
        assert!(matches!(e, ProviderError::Unknown(_)));
    }

    #[test]
    fn test_billing_wins_over_auth_on_402() {
        let e = classify_error_message("HTTP 402 payment required");
        assert!(
            matches!(e, ProviderError::Billing(_)),
            "402 should be Billing, not Auth"
        );
    }

    #[test]
    fn test_rate_limit_resource_exhausted() {
        let e = classify_error_message("resource has been exhausted");
        assert!(matches!(e, ProviderError::RateLimit(_)));
    }
}
