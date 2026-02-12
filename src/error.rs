//! Error types for ZeptoClaw
//!
//! This module defines all error types used throughout the ZeptoClaw framework.
//! Uses `thiserror` for ergonomic error handling with automatic `Display` and
//! `Error` trait implementations.

use thiserror::Error;

/// The primary error type for ZeptoClaw operations.
#[derive(Error, Debug)]
pub enum PicoError {
    /// Configuration-related errors (invalid config, missing required fields, etc.)
    #[error("Configuration error: {0}")]
    Config(String),

    /// Provider errors (API failures, rate limits, model errors, etc.)
    #[error("Provider error: {0}")]
    Provider(String),

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
}

/// A specialized `Result` type for ZeptoClaw operations.
pub type Result<T> = std::result::Result<T, PicoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = PicoError::Config("missing API key".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing API key");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let pico_err: PicoError = io_err.into();
        assert!(matches!(pico_err, PicoError::Io(_)));
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
        let _ = PicoError::Config("test".into());
        let _ = PicoError::Provider("test".into());
        let _ = PicoError::Channel("test".into());
        let _ = PicoError::Tool("test".into());
        let _ = PicoError::Session("test".into());
        let _ = PicoError::BusClosed;
        let _ = PicoError::NotFound("test".into());
        let _ = PicoError::Unauthorized("test".into());
        let _ = PicoError::SecurityViolation("test".into());
    }

    #[test]
    fn test_security_violation_display() {
        let err = PicoError::SecurityViolation("path traversal attempt detected".to_string());
        assert_eq!(
            err.to_string(),
            "Security violation: path traversal attempt detected"
        );
    }
}
