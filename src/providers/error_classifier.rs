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

    // Context overflow — prompt exceeds model's context window (retriable via compaction).
    //
    // Patterns must be specific enough not to collide with rate-limit messages
    // like "you've hit the token limit for your plan tier", which is a quota
    // event, not an oversized prompt. A bare "token limit" substring is too
    // broad and would loop the compaction retry on rate-limit responses, so
    // we require a stronger qualifier.
    if contains_any(
        &lower,
        &[
            "context_length_exceeded",
            "context length exceeded",
            "context window exceeded",
            "prompt is too long",
            "maximum context length",
            "too many tokens",
            "request too large",
            "token limit exceeded",
            "input is too long",
            "exceeds the model",
        ],
    ) {
        return ProviderError::ContextOverflow(msg.to_string());
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

    #[test]
    fn test_context_overflow_prompt_too_long() {
        let e = classify_error_message("prompt is too long: 201530 tokens > 200000 maximum");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_context_length() {
        let e = classify_error_message(
            "This model's maximum context length is 200000 tokens. However, you requested 201530 tokens.",
        );
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_request_too_large() {
        let e = classify_error_message("request too large for model");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_context_length_exceeded() {
        let e = classify_error_message("context_length_exceeded");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_too_many_tokens() {
        let e = classify_error_message("too many tokens in the request");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_token_limit_exceeded() {
        let e = classify_error_message("token limit exceeded: 201530 > 200000");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_window_exceeded() {
        let e = classify_error_message("context window exceeded for this model");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_rate_limit_token_phrasing_not_misclassified() {
        // Rate-limit messages can mention "token limit" too. The classifier
        // must not treat plan-tier token quotas as a context overflow,
        // otherwise the agent loop's compaction retry would loop on a quota
        // event that compaction can't fix.
        let e =
            classify_error_message("You've hit the token limit for your plan tier (rate limit)");
        assert!(
            matches!(e, ProviderError::RateLimit(_)),
            "plan-tier token limit must classify as RateLimit, not ContextOverflow"
        );

        // Even without the explicit "rate limit" qualifier, a bare
        // "token limit" string must not be treated as ContextOverflow.
        let e = classify_error_message("You've hit the token limit for your plan tier");
        assert!(
            !matches!(e, ProviderError::ContextOverflow(_)),
            "bare 'token limit' must not be classified as ContextOverflow"
        );
    }

    #[test]
    fn test_context_overflow_input_too_long() {
        let e = classify_error_message("input is too long for this model");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_context_overflow_exceeds_model() {
        let e = classify_error_message("Your input exceeds the model's context window");
        assert!(matches!(e, ProviderError::ContextOverflow(_)));
    }

    #[test]
    fn test_max_tokens_validation_not_misclassified_as_overflow() {
        // "max_tokens must be at least 1" is a config error, not context overflow
        let e = classify_error_message("max_tokens must be at least 1");
        assert!(
            !matches!(e, ProviderError::ContextOverflow(_)),
            "max_tokens validation errors should not be classified as ContextOverflow"
        );
    }
}
