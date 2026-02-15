//! Providers module - LLM providers (OpenAI, Anthropic, etc.)
//!
//! This module defines the `LLMProvider` trait and common types for
//! interacting with various LLM providers. Each provider (OpenAI, Claude, etc.)
//! implements the `LLMProvider` trait to provide a consistent interface.
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::providers::{LLMProvider, ChatOptions, ToolDefinition};
//! use zeptoclaw::providers::claude::ClaudeProvider;
//! use zeptoclaw::session::Message;
//!
//! async fn example() {
//!     let provider = ClaudeProvider::new("your-api-key");
//!     let messages = vec![Message::user("Hello!")];
//!     let options = ChatOptions::new().with_max_tokens(1000);
//!
//!     let response = provider.chat(messages, vec![], None, options).await.unwrap();
//!     println!("Response: {}", response.content);
//! }
//! ```

pub mod claude;
pub mod fallback;
pub mod openai;
mod registry;
pub mod retry;
pub mod structured;
mod types;

/// Provider IDs currently supported by the runtime.
pub const RUNTIME_SUPPORTED_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "openrouter",
    "groq",
    "zhipu",
    "vllm",
    "gemini",
    "ollama",
];

use crate::error::ProviderError;

pub use claude::ClaudeProvider;
pub use fallback::FallbackProvider;
pub use openai::OpenAIProvider;
pub use registry::{
    configured_provider_names, configured_unsupported_provider_names, resolve_runtime_provider,
    resolve_runtime_providers, ProviderSpec, RuntimeProviderSelection, PROVIDER_REGISTRY,
};
pub use retry::RetryProvider;
pub use structured::{validate_json_response, OutputFormat};
pub use types::{
    ChatOptions, LLMProvider, LLMResponse, LLMToolCall, StreamEvent, ToolDefinition, Usage,
};

/// Parse an HTTP status code and response body into a structured [`ProviderError`].
///
/// This centralizes the mapping from HTTP status codes to error classifications
/// so that both Claude and OpenAI providers produce consistent typed errors.
pub fn parse_provider_error(status: u16, body: &str) -> ProviderError {
    match status {
        401 => ProviderError::Auth(body.to_string()),
        402 => ProviderError::Billing(body.to_string()),
        404 => ProviderError::ModelNotFound(body.to_string()),
        429 => ProviderError::RateLimit(body.to_string()),
        400 => ProviderError::InvalidRequest(body.to_string()),
        500..=599 => ProviderError::ServerError(body.to_string()),
        _ => ProviderError::Unknown(format!("HTTP {}: {}", status, body)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_provider_error_401() {
        let err = parse_provider_error(401, "invalid api key");
        assert!(matches!(err, ProviderError::Auth(_)));
        assert_eq!(err.status_code(), Some(401));
    }

    #[test]
    fn test_parse_provider_error_402() {
        let err = parse_provider_error(402, "payment required");
        assert!(matches!(err, ProviderError::Billing(_)));
        assert_eq!(err.status_code(), Some(402));
    }

    #[test]
    fn test_parse_provider_error_404() {
        let err = parse_provider_error(404, "model not found");
        assert!(matches!(err, ProviderError::ModelNotFound(_)));
        assert_eq!(err.status_code(), Some(404));
    }

    #[test]
    fn test_parse_provider_error_429() {
        let err = parse_provider_error(429, "rate limited");
        assert!(matches!(err, ProviderError::RateLimit(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn test_parse_provider_error_400() {
        let err = parse_provider_error(400, "bad json");
        assert!(matches!(err, ProviderError::InvalidRequest(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_parse_provider_error_500() {
        let err = parse_provider_error(500, "internal server error");
        assert!(matches!(err, ProviderError::ServerError(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn test_parse_provider_error_502() {
        let err = parse_provider_error(502, "bad gateway");
        assert!(matches!(err, ProviderError::ServerError(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn test_parse_provider_error_503() {
        let err = parse_provider_error(503, "service unavailable");
        assert!(matches!(err, ProviderError::ServerError(_)));
    }

    #[test]
    fn test_parse_provider_error_504() {
        let err = parse_provider_error(504, "gateway timeout");
        assert!(matches!(err, ProviderError::ServerError(_)));
    }

    #[test]
    fn test_parse_provider_error_unknown() {
        let err = parse_provider_error(418, "i'm a teapot");
        assert!(matches!(err, ProviderError::Unknown(_)));
        assert!(err.to_string().contains("HTTP 418"));
    }
}
