//! Retry provider - decorator that adds automatic retry with exponential backoff.
//!
//! Wraps any [`LLMProvider`] to transparently retry transient errors such as
//! HTTP 429 (rate limit), 5xx (server errors), and provider overload conditions.
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::providers::retry::RetryProvider;
//! use zeptoclaw::providers::claude::ClaudeProvider;
//!
//! let inner = ClaudeProvider::new("api-key");
//! let provider = RetryProvider::new(Box::new(inner))
//!     .with_max_retries(5)
//!     .with_base_delay_ms(500);
//!
//! // Use `provider` as any other LLMProvider — retries happen automatically.
//! ```

use async_trait::async_trait;
use tracing::warn;

use crate::error::{Result, ZeptoError};
use crate::session::Message;

use super::{ChatOptions, LLMProvider, LLMResponse, StreamEvent, ToolDefinition};

/// Patterns in error messages that indicate a transient, retryable failure.
const RETRYABLE_PATTERNS: &[&str] = &[
    "429",
    "500",
    "502",
    "503",
    "504",
    "rate limit",
    "rate_limit",
    "overloaded",
    "too many requests",
    "server error",
    "internal server error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
];

/// A decorator provider that retries transient LLM errors with exponential backoff.
///
/// `RetryProvider` wraps an inner [`LLMProvider`] and intercepts errors from
/// `chat()` and `chat_stream()`. When a transient error is detected (e.g., rate
/// limiting, server errors), the request is retried up to `max_retries` times
/// with exponential backoff and jitter between attempts.
///
/// Non-transient errors (400, 401, 403, 404) are returned immediately without retry.
pub struct RetryProvider {
    /// The wrapped provider that performs actual LLM requests.
    inner: Box<dyn LLMProvider>,
    /// Maximum number of retry attempts before giving up. Default: 3.
    max_retries: u32,
    /// Base delay in milliseconds for exponential backoff. Default: 1000 (1 second).
    base_delay_ms: u64,
    /// Maximum delay cap in milliseconds. Default: 30000 (30 seconds).
    max_delay_ms: u64,
}

impl std::fmt::Debug for RetryProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryProvider")
            .field("inner", &self.inner.name())
            .field("max_retries", &self.max_retries)
            .field("base_delay_ms", &self.base_delay_ms)
            .field("max_delay_ms", &self.max_delay_ms)
            .finish()
    }
}

impl RetryProvider {
    /// Create a new `RetryProvider` wrapping the given inner provider.
    ///
    /// Uses default retry settings:
    /// - `max_retries`: 3
    /// - `base_delay_ms`: 1000 (1 second)
    /// - `max_delay_ms`: 30000 (30 seconds)
    ///
    /// # Arguments
    /// * `inner` - The provider to wrap with retry logic
    pub fn new(inner: Box<dyn LLMProvider>) -> Self {
        Self {
            inner,
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30_000,
        }
    }

    /// Set the maximum number of retry attempts.
    ///
    /// # Arguments
    /// * `max_retries` - Maximum retries before propagating the error
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the base delay in milliseconds for exponential backoff.
    ///
    /// The actual delay for attempt `n` is:
    /// `min(base_delay_ms * 2^n + jitter, max_delay_ms)`
    ///
    /// # Arguments
    /// * `base_delay_ms` - Base delay in milliseconds
    pub fn with_base_delay_ms(mut self, base_delay_ms: u64) -> Self {
        self.base_delay_ms = base_delay_ms;
        self
    }

    /// Set the maximum delay cap in milliseconds.
    ///
    /// Prevents exponential backoff from growing unbounded.
    ///
    /// # Arguments
    /// * `max_delay_ms` - Maximum delay in milliseconds
    pub fn with_max_delay_ms(mut self, max_delay_ms: u64) -> Self {
        self.max_delay_ms = max_delay_ms;
        self
    }
}

/// Check whether a [`ZeptoError`] represents a transient failure that should be retried.
///
/// For structured [`ProviderError`](crate::error::ProviderError) errors, delegates
/// to [`ProviderError::is_retryable`]. For legacy `Provider(String)` errors, falls
/// back to substring matching against known retryable patterns.
pub fn is_retryable(err: &ZeptoError) -> bool {
    match err {
        ZeptoError::ProviderTyped(pe) => pe.is_retryable(),
        _ => {
            // Fallback: keep old string matching for backward compatibility
            let msg = err.to_string().to_lowercase();

            // Explicitly exclude non-retryable client errors
            let non_retryable = ["400", "401", "403", "404"];
            for pattern in &non_retryable {
                if msg.contains(pattern) {
                    return false;
                }
            }

            for pattern in RETRYABLE_PATTERNS {
                if msg.contains(pattern) {
                    return true;
                }
            }

            false
        }
    }
}

/// Compute and sleep for the backoff delay for a given retry attempt.
///
/// Delay formula: `min(base_delay_ms * 2^attempt + jitter, max_delay_ms)`
///
/// Jitter is derived from the current system time (nanosecond component) to
/// avoid adding the `rand` crate as a dependency. This provides sufficient
/// decorrelation for retry storms while keeping dependencies minimal.
///
/// # Arguments
/// * `attempt` - The current retry attempt (0-indexed)
/// * `base_delay_ms` - Base delay in milliseconds
/// * `max_delay_ms` - Maximum delay cap in milliseconds
pub async fn delay_with_jitter(attempt: u32, base_delay_ms: u64, max_delay_ms: u64) {
    let exponential = base_delay_ms.saturating_mul(1u64 << attempt.min(16));

    // Use nanosecond component of system time as a lightweight jitter source.
    // This avoids adding the `rand` crate while still decorrelating concurrent retries.
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % (base_delay_ms.max(1)))
        .unwrap_or(0);

    let delay = exponential.saturating_add(jitter_ms).min(max_delay_ms);

    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
}

/// Compute the backoff delay for a given attempt (without sleeping).
///
/// Useful for testing the exponential backoff calculation.
///
/// # Arguments
/// * `attempt` - The current retry attempt (0-indexed)
/// * `base_delay_ms` - Base delay in milliseconds
/// * `max_delay_ms` - Maximum delay cap in milliseconds
/// * `jitter_ms` - Jitter value to add
///
/// # Returns
/// The computed delay in milliseconds.
pub fn compute_delay(attempt: u32, base_delay_ms: u64, max_delay_ms: u64, jitter_ms: u64) -> u64 {
    let exponential = base_delay_ms.saturating_mul(1u64 << attempt.min(16));
    exponential.saturating_add(jitter_ms).min(max_delay_ms)
}

#[async_trait]
impl LLMProvider for RetryProvider {
    fn name(&self) -> &str {
        // Delegate to the inner provider. The trait requires `&str` with lifetime
        // tied to `&self`, so we cannot return a formatted string like
        // `format!("retry({})", ...)` without leaking or storing it. Delegation
        // is the cleanest approach; the wrapping is evident from the type itself.
        self.inner.name()
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        let mut last_err: Option<ZeptoError> = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                if let Some(ref err) = last_err {
                    warn!(
                        provider = self.inner.name(),
                        attempt = attempt,
                        max_retries = self.max_retries,
                        error = %err,
                        "Retrying chat request after transient error"
                    );
                }
                delay_with_jitter(attempt - 1, self.base_delay_ms, self.max_delay_ms).await;
            }

            match self
                .inner
                .chat(messages.clone(), tools.clone(), model, options.clone())
                .await
            {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if !is_retryable(&err) || attempt == self.max_retries {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
            }
        }

        // This is unreachable because the loop always returns, but the compiler
        // cannot prove it. Provide a sensible fallback.
        Err(last_err.unwrap_or_else(|| {
            ZeptoError::Provider("Retry loop exited without result".to_string())
        }))
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let mut last_err: Option<ZeptoError> = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                if let Some(ref err) = last_err {
                    warn!(
                        provider = self.inner.name(),
                        attempt = attempt,
                        max_retries = self.max_retries,
                        error = %err,
                        "Retrying chat_stream request after transient error"
                    );
                }
                delay_with_jitter(attempt - 1, self.base_delay_ms, self.max_delay_ms).await;
            }

            match self
                .inner
                .chat_stream(messages.clone(), tools.clone(), model, options.clone())
                .await
            {
                Ok(receiver) => return Ok(receiver),
                Err(err) => {
                    if !is_retryable(&err) || attempt == self.max_retries {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ZeptoError::Provider("Retry loop exited without result".to_string())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock LLM provider for testing retry behavior.
    struct MockProvider {
        name: &'static str,
        model: &'static str,
    }

    impl MockProvider {
        fn new(name: &'static str, model: &'static str) -> Self {
            Self { name, model }
        }
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn default_model(&self) -> &str {
            self.model
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Ok(LLMResponse::text("mock response"))
        }
    }

    #[test]
    fn test_retry_provider_creation() {
        let mock = MockProvider::new("test-provider", "test-model-v1");
        let provider = RetryProvider::new(Box::new(mock));

        assert_eq!(provider.name(), "test-provider");
        assert_eq!(provider.default_model(), "test-model-v1");
        assert_eq!(provider.max_retries, 3);
        assert_eq!(provider.base_delay_ms, 1000);
        assert_eq!(provider.max_delay_ms, 30_000);
    }

    #[test]
    fn test_retry_provider_builder() {
        let mock = MockProvider::new("test", "model");
        let provider = RetryProvider::new(Box::new(mock))
            .with_max_retries(5)
            .with_base_delay_ms(500)
            .with_max_delay_ms(60_000);

        assert_eq!(provider.max_retries, 5);
        assert_eq!(provider.base_delay_ms, 500);
        assert_eq!(provider.max_delay_ms, 60_000);
    }

    #[test]
    fn test_is_retryable_429() {
        let err = ZeptoError::Provider("HTTP 429 Too Many Requests".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_500() {
        let err = ZeptoError::Provider("HTTP 500 Internal Server Error".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_502() {
        let err = ZeptoError::Provider("HTTP 502 Bad Gateway".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_503() {
        let err = ZeptoError::Provider("HTTP 503 Service Unavailable".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_504() {
        let err = ZeptoError::Provider("HTTP 504 Gateway Timeout".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_rate_limit() {
        let err = ZeptoError::Provider("Rate limit exceeded, please retry".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_rate_limit_underscore() {
        let err = ZeptoError::Provider("rate_limit_exceeded".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_overloaded() {
        let err = ZeptoError::Provider("Model is overloaded, try again later".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_400() {
        let err = ZeptoError::Provider("HTTP 400 Bad Request: invalid JSON".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_401() {
        let err = ZeptoError::Provider("HTTP 401 Unauthorized: invalid API key".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_403() {
        let err = ZeptoError::Provider("HTTP 403 Forbidden".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_404() {
        let err = ZeptoError::Provider("HTTP 404 Not Found: model not available".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_generic_error() {
        let err = ZeptoError::Provider("Connection reset by peer".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_non_provider_error() {
        let err = ZeptoError::Config("Missing API key".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_delay_calculation_attempt_0() {
        // attempt 0: base_delay * 2^0 = 1000 * 1 = 1000
        let delay = compute_delay(0, 1000, 30_000, 0);
        assert_eq!(delay, 1000);
    }

    #[test]
    fn test_delay_calculation_attempt_1() {
        // attempt 1: base_delay * 2^1 = 1000 * 2 = 2000
        let delay = compute_delay(1, 1000, 30_000, 0);
        assert_eq!(delay, 2000);
    }

    #[test]
    fn test_delay_calculation_attempt_2() {
        // attempt 2: base_delay * 2^2 = 1000 * 4 = 4000
        let delay = compute_delay(2, 1000, 30_000, 0);
        assert_eq!(delay, 4000);
    }

    #[test]
    fn test_delay_calculation_attempt_3() {
        // attempt 3: base_delay * 2^3 = 1000 * 8 = 8000
        let delay = compute_delay(3, 1000, 30_000, 0);
        assert_eq!(delay, 8000);
    }

    #[test]
    fn test_delay_calculation_with_jitter() {
        // attempt 1 with 200ms jitter: 2000 + 200 = 2200
        let delay = compute_delay(1, 1000, 30_000, 200);
        assert_eq!(delay, 2200);
    }

    #[test]
    fn test_delay_calculation_capped_at_max() {
        // attempt 10: base_delay * 2^10 = 1000 * 1024 = 1024000, capped at 30000
        let delay = compute_delay(10, 1000, 30_000, 0);
        assert_eq!(delay, 30_000);
    }

    #[test]
    fn test_delay_calculation_max_with_jitter_still_capped() {
        // Even with jitter, delay should not exceed max
        let delay = compute_delay(10, 1000, 30_000, 5000);
        assert_eq!(delay, 30_000);
    }

    #[test]
    fn test_delay_calculation_custom_base() {
        // attempt 0 with 500ms base: 500 * 1 = 500
        let delay = compute_delay(0, 500, 30_000, 0);
        assert_eq!(delay, 500);

        // attempt 2 with 500ms base: 500 * 4 = 2000
        let delay = compute_delay(2, 500, 30_000, 0);
        assert_eq!(delay, 2000);
    }

    #[tokio::test]
    async fn test_retry_provider_chat_success() {
        let mock = MockProvider::new("test", "model");
        let provider = RetryProvider::new(Box::new(mock));

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "mock response");
    }

    #[tokio::test]
    async fn test_retry_provider_chat_stream_success() {
        let mock = MockProvider::new("test", "model");
        let provider = RetryProvider::new(Box::new(mock));

        let result = provider
            .chat_stream(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        let mut rx = result.unwrap();
        let event = rx.recv().await.unwrap();
        match event {
            StreamEvent::Done { content, .. } => {
                assert_eq!(content, "mock response");
            }
            _ => panic!("Expected Done event"),
        }
    }

    /// A mock provider that fails a configurable number of times before succeeding.
    struct FailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicU32,
        target_failures: u32,
        error_message: String,
    }

    impl FailThenSucceedProvider {
        fn new(target_failures: u32, error_message: &str) -> Self {
            Self {
                fail_count: std::sync::atomic::AtomicU32::new(0),
                target_failures,
                error_message: error_message.to_string(),
            }
        }
    }

    #[async_trait]
    impl LLMProvider for FailThenSucceedProvider {
        fn name(&self) -> &str {
            "fail-then-succeed"
        }

        fn default_model(&self) -> &str {
            "test-model"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            let count = self
                .fail_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count < self.target_failures {
                Err(ZeptoError::Provider(self.error_message.clone()))
            } else {
                Ok(LLMResponse::text("recovered"))
            }
        }
    }

    #[tokio::test]
    async fn test_retry_provider_retries_on_429() {
        let inner = FailThenSucceedProvider::new(2, "HTTP 429 Too Many Requests");
        let provider = RetryProvider::new(Box::new(inner))
            .with_max_retries(3)
            .with_base_delay_ms(1) // Use tiny delays for fast tests
            .with_max_delay_ms(10);

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "recovered");
    }

    #[tokio::test]
    async fn test_retry_provider_retries_on_500() {
        let inner = FailThenSucceedProvider::new(1, "HTTP 500 Internal Server Error");
        let provider = RetryProvider::new(Box::new(inner))
            .with_max_retries(3)
            .with_base_delay_ms(1)
            .with_max_delay_ms(10);

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "recovered");
    }

    #[tokio::test]
    async fn test_retry_provider_no_retry_on_401() {
        let inner = FailThenSucceedProvider::new(1, "HTTP 401 Unauthorized");
        let provider = RetryProvider::new(Box::new(inner))
            .with_max_retries(3)
            .with_base_delay_ms(1)
            .with_max_delay_ms(10);

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Should fail immediately without retry
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("401"));
    }

    #[tokio::test]
    async fn test_retry_provider_exhausts_retries() {
        // Fail more times than max_retries allows
        let inner = FailThenSucceedProvider::new(10, "HTTP 429 Too Many Requests");
        let provider = RetryProvider::new(Box::new(inner))
            .with_max_retries(2)
            .with_base_delay_ms(1)
            .with_max_delay_ms(10);

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Should fail after exhausting retries
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("429"));
    }

    // ====================================================================
    // ProviderTyped error tests
    // ====================================================================

    #[test]
    fn test_is_retryable_typed_rate_limit() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::RateLimit("quota exceeded".into()));
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_typed_server_error() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::ServerError("internal error".into()));
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_typed_timeout() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::Timeout("connection timed out".into()));
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_typed_auth() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::Auth("invalid api key".into()));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_typed_billing() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::Billing("payment required".into()));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_typed_invalid_request() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::InvalidRequest("bad json".into()));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_typed_model_not_found() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::ModelNotFound("gpt-99".into()));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_typed_unknown() {
        use crate::error::ProviderError;
        let err = ZeptoError::ProviderTyped(ProviderError::Unknown("something".into()));
        assert!(!is_retryable(&err));
    }

    /// A mock provider that fails with ProviderTyped errors before succeeding.
    struct TypedFailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicU32,
        target_failures: u32,
    }

    impl TypedFailThenSucceedProvider {
        fn new(target_failures: u32) -> Self {
            Self {
                fail_count: std::sync::atomic::AtomicU32::new(0),
                target_failures,
            }
        }
    }

    #[async_trait]
    impl LLMProvider for TypedFailThenSucceedProvider {
        fn name(&self) -> &str {
            "typed-fail-then-succeed"
        }

        fn default_model(&self) -> &str {
            "test-model"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            use crate::error::ProviderError;
            let count = self
                .fail_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count < self.target_failures {
                Err(ZeptoError::ProviderTyped(ProviderError::RateLimit(
                    "quota exceeded".into(),
                )))
            } else {
                Ok(LLMResponse::text("recovered"))
            }
        }
    }

    #[tokio::test]
    async fn test_retry_provider_retries_typed_rate_limit() {
        let inner = TypedFailThenSucceedProvider::new(2);
        let provider = RetryProvider::new(Box::new(inner))
            .with_max_retries(3)
            .with_base_delay_ms(1)
            .with_max_delay_ms(10);

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "recovered");
    }
}
