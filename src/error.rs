//! Error types for ZeptoClaw
//!
//! This module defines all error types used throughout the ZeptoClaw framework.
//! Uses `thiserror` for ergonomic error handling with automatic `Display` and
//! `Error` trait implementations.

use std::fmt;
use thiserror::Error;

// ============================================================================
// Provider Error Classification
// ============================================================================

/// Structured provider error classification.
///
/// Provides fine-grained categorization of LLM provider HTTP errors,
/// enabling intelligent retry and fallback decisions without string matching.
#[derive(Debug)]
pub enum ProviderError {
    /// 401 — Invalid API key or authentication failure
    Auth(String),
    /// 429 — Rate limit or quota exceeded
    RateLimit(String),
    /// 402 — Payment required or billing issue
    Billing(String),
    /// 500/502/503/504 — Server-side errors
    ServerError(String),
    /// 400 — Bad request, invalid JSON, malformed parameters
    InvalidRequest(String),
    /// 404 — Model not found or endpoint not available
    ModelNotFound(String),
    /// Connection or read timeout
    Timeout(String),
    /// Catch-all for unrecognized errors
    Unknown(String),
    /// Provider is overloaded (e.g. Anthropic `overloaded_error`) — retry with backoff
    Overloaded(String),
    /// Request format error (e.g. malformed tool_use.id) — do not retry
    Format(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Auth(msg) => write!(f, "Authentication error: {}", msg),
            ProviderError::RateLimit(msg) => write!(f, "Rate limit error: {}", msg),
            ProviderError::Billing(msg) => write!(f, "Billing error: {}", msg),
            ProviderError::ServerError(msg) => write!(f, "Server error: {}", msg),
            ProviderError::InvalidRequest(msg) => write!(f, "Invalid request: {}", msg),
            ProviderError::ModelNotFound(msg) => write!(f, "Model not found: {}", msg),
            ProviderError::Timeout(msg) => write!(f, "Timeout: {}", msg),
            ProviderError::Unknown(msg) => write!(f, "Unknown provider error: {}", msg),
            ProviderError::Overloaded(msg) => write!(f, "Overloaded error: {}", msg),
            ProviderError::Format(msg) => write!(f, "Format error: {}", msg),
        }
    }
}

impl ProviderError {
    /// Returns `true` if this error is transient and the request should be retried.
    ///
    /// Retryable errors: RateLimit, ServerError, Timeout.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimit(_)
                | ProviderError::ServerError(_)
                | ProviderError::Timeout(_)
                | ProviderError::Overloaded(_)
        )
    }

    /// Returns `true` if this error should trigger a fallback to a secondary provider.
    ///
    /// Non-recoverable errors (Auth, InvalidRequest, Billing) should NOT trigger
    /// fallback because the same request would fail against any provider.
    pub fn should_fallback(&self) -> bool {
        !matches!(
            self,
            ProviderError::Auth(_)
                | ProviderError::InvalidRequest(_)
                | ProviderError::Billing(_)
                | ProviderError::Format(_)
        )
    }

    /// Returns the HTTP status code associated with this error, if applicable.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            ProviderError::Auth(_) => Some(401),
            ProviderError::RateLimit(_) => Some(429),
            ProviderError::Billing(_) => Some(402),
            ProviderError::ServerError(_) => Some(500),
            ProviderError::InvalidRequest(_) => Some(400),
            ProviderError::ModelNotFound(_) => Some(404),
            ProviderError::Timeout(_) => None,
            ProviderError::Overloaded(_) => Some(503),
            ProviderError::Format(_) => Some(400),
            ProviderError::Unknown(_) => None,
        }
    }
}

impl From<ProviderError> for ZeptoError {
    fn from(err: ProviderError) -> Self {
        ZeptoError::ProviderTyped(err)
    }
}

// ============================================================================
// Primary Error Type
// ============================================================================

/// The primary error type for ZeptoClaw operations.
#[derive(Error, Debug)]
pub enum ZeptoError {
    /// Configuration-related errors (invalid config, missing required fields, etc.)
    #[error("Configuration error: {0}")]
    Config(String),

    /// Provider errors (API failures, rate limits, model errors, etc.)
    /// Kept for backward compatibility — new code should prefer `ProviderTyped`.
    #[error("Provider error: {0}")]
    Provider(String),

    /// Structured provider error with classification for retry/fallback decisions.
    #[error("Provider error: {0}")]
    ProviderTyped(ProviderError),

    /// Channel errors (connection failures, message routing issues, etc.)
    #[error("Channel error: {0}")]
    Channel(String),

    /// Tool execution errors (invalid parameters, execution failures, etc.)
    #[error("Tool error: {0}")]
    Tool(String),

    /// Session management errors (invalid state, persistence failures, etc.)
    #[error("Session error: {0}")]
    Session(String),

    /// Standard I/O errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// HTTP request errors
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Message bus channel closed unexpectedly
    #[error("Bus error: channel closed")]
    BusClosed,

    /// Resource not found (sessions, tools, providers, etc.)
    #[error("Not found: {0}")]
    NotFound(String),

    /// Authentication or authorization failures
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// Security violations (path traversal attempts, blocked commands, etc.)
    #[error("Security violation: {0}")]
    SecurityViolation(String),

    /// Safety layer violations (prompt injection, credential leaks, policy violations, etc.)
    #[error("Safety violation: {0}")]
    Safety(String),

    /// MCP (Model Context Protocol) errors (server communication, tool execution, etc.)
    #[error("MCP error: {0}")]
    Mcp(String),
}

/// A specialized `Result` type for ZeptoClaw operations.
pub type Result<T> = std::result::Result<T, ZeptoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ZeptoError::Config("missing API key".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing API key");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let zepto_err: ZeptoError = io_err.into();
        assert!(matches!(zepto_err, ZeptoError::Io(_)));
    }

    #[test]
    fn test_result_type() {
        fn returns_result() -> Result<i32> {
            Ok(42)
        }
        assert_eq!(returns_result().unwrap(), 42);
    }

    #[test]
    fn test_error_variants() {
        // Ensure all variants can be created
        let _ = ZeptoError::Config("test".into());
        let _ = ZeptoError::Provider("test".into());
        let _ = ZeptoError::ProviderTyped(ProviderError::Auth("test".into()));
        let _ = ZeptoError::Channel("test".into());
        let _ = ZeptoError::Tool("test".into());
        let _ = ZeptoError::Session("test".into());
        let _ = ZeptoError::BusClosed;
        let _ = ZeptoError::NotFound("test".into());
        let _ = ZeptoError::Unauthorized("test".into());
        let _ = ZeptoError::SecurityViolation("test".into());
        let _ = ZeptoError::Safety("test".into());
        let _ = ZeptoError::Mcp("test".into());
    }

    #[test]
    fn test_security_violation_display() {
        let err = ZeptoError::SecurityViolation("path traversal attempt detected".to_string());
        assert_eq!(
            err.to_string(),
            "Security violation: path traversal attempt detected"
        );
    }

    // ====================================================================
    // ProviderError tests
    // ====================================================================

    #[test]
    fn test_provider_error_display() {
        assert!(ProviderError::Auth("bad key".into())
            .to_string()
            .contains("Authentication error"));
        assert!(ProviderError::RateLimit("quota".into())
            .to_string()
            .contains("Rate limit error"));
        assert!(ProviderError::Billing("no funds".into())
            .to_string()
            .contains("Billing error"));
        assert!(ProviderError::ServerError("500".into())
            .to_string()
            .contains("Server error"));
        assert!(ProviderError::InvalidRequest("bad json".into())
            .to_string()
            .contains("Invalid request"));
        assert!(ProviderError::ModelNotFound("gpt-99".into())
            .to_string()
            .contains("Model not found"));
        assert!(ProviderError::Timeout("30s".into())
            .to_string()
            .contains("Timeout"));
        assert!(ProviderError::Unknown("???".into())
            .to_string()
            .contains("Unknown provider error"));
        assert!(ProviderError::Overloaded("busy".into())
            .to_string()
            .contains("Overloaded error"));
        assert!(ProviderError::Format("bad id".into())
            .to_string()
            .contains("Format error"));
    }

    #[test]
    fn test_provider_error_is_retryable() {
        // Retryable
        assert!(ProviderError::RateLimit("429".into()).is_retryable());
        assert!(ProviderError::ServerError("500".into()).is_retryable());
        assert!(ProviderError::Timeout("timeout".into()).is_retryable());

        // Also retryable
        assert!(ProviderError::Overloaded("busy".into()).is_retryable());

        // Not retryable
        assert!(!ProviderError::Auth("401".into()).is_retryable());
        assert!(!ProviderError::Billing("402".into()).is_retryable());
        assert!(!ProviderError::InvalidRequest("400".into()).is_retryable());
        assert!(!ProviderError::ModelNotFound("404".into()).is_retryable());
        assert!(!ProviderError::Unknown("???".into()).is_retryable());
        assert!(!ProviderError::Format("bad id".into()).is_retryable());
    }

    #[test]
    fn test_provider_error_should_fallback() {
        // Should fallback
        assert!(ProviderError::RateLimit("429".into()).should_fallback());
        assert!(ProviderError::ServerError("500".into()).should_fallback());
        assert!(ProviderError::Timeout("timeout".into()).should_fallback());
        assert!(ProviderError::ModelNotFound("404".into()).should_fallback());
        assert!(ProviderError::Unknown("???".into()).should_fallback());

        // Also fallbacks
        assert!(ProviderError::Overloaded("busy".into()).should_fallback());

        // Should NOT fallback
        assert!(!ProviderError::Auth("401".into()).should_fallback());
        assert!(!ProviderError::InvalidRequest("400".into()).should_fallback());
        assert!(!ProviderError::Billing("402".into()).should_fallback());
        assert!(!ProviderError::Format("bad id".into()).should_fallback());
    }

    #[test]
    fn test_provider_error_status_code() {
        assert_eq!(ProviderError::Auth("x".into()).status_code(), Some(401));
        assert_eq!(
            ProviderError::RateLimit("x".into()).status_code(),
            Some(429)
        );
        assert_eq!(ProviderError::Billing("x".into()).status_code(), Some(402));
        assert_eq!(
            ProviderError::ServerError("x".into()).status_code(),
            Some(500)
        );
        assert_eq!(
            ProviderError::InvalidRequest("x".into()).status_code(),
            Some(400)
        );
        assert_eq!(
            ProviderError::ModelNotFound("x".into()).status_code(),
            Some(404)
        );
        assert_eq!(ProviderError::Timeout("x".into()).status_code(), None);
        assert_eq!(
            ProviderError::Overloaded("x".into()).status_code(),
            Some(503)
        );
        assert_eq!(ProviderError::Format("x".into()).status_code(), Some(400));
        assert_eq!(ProviderError::Unknown("x".into()).status_code(), None);
    }

    #[test]
    fn test_provider_error_into_zepto_error() {
        let pe = ProviderError::RateLimit("too fast".into());
        let ze: ZeptoError = pe.into();
        assert!(matches!(ze, ZeptoError::ProviderTyped(_)));
        assert!(ze.to_string().contains("Rate limit error"));
    }

    #[test]
    fn test_provider_typed_display() {
        let err = ZeptoError::ProviderTyped(ProviderError::Auth("invalid key".into()));
        assert_eq!(
            err.to_string(),
            "Provider error: Authentication error: invalid key"
        );
    }
}
