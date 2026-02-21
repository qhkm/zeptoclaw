//! Fallback LLM provider for ZeptoClaw
//!
//! This module provides a [`FallbackProvider`] that chains two LLM providers:
//! if the primary provider fails, the request is automatically retried against
//! a secondary (fallback) provider. This is useful for high-availability
//! configurations where one provider may experience intermittent outages.
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::providers::fallback::FallbackProvider;
//! use zeptoclaw::providers::claude::ClaudeProvider;
//! use zeptoclaw::providers::openai::OpenAIProvider;
//!
//! let primary = Box::new(ClaudeProvider::new("claude-key"));
//! let fallback = Box::new(OpenAIProvider::new("openai-key"));
//! let provider = FallbackProvider::new(primary, fallback);
//! // If Claude fails, the request is automatically retried against OpenAI.
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::Result;
use crate::session::Message;

use super::cooldown::{CooldownTracker, FailoverReason};
use super::{ChatOptions, LLMProvider, LLMResponse, StreamEvent, ToolDefinition};

// ============================================================================
// Circuit Breaker
// ============================================================================

/// Circuit breaker states for the primary provider.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CircuitState {
    /// Normal operation -- primary provider is tried first.
    Closed,
    /// Primary provider is unhealthy -- skip directly to fallback.
    Open,
    /// Probing -- allow one request to primary to test recovery.
    HalfOpen,
}

/// Lock-free circuit breaker for tracking primary provider health.
///
/// Uses atomic counters to avoid mutex overhead. The circuit transitions
/// through three states:
///
/// - **Closed**: Normal operation, primary is tried first.
/// - **Open**: Primary is unhealthy after `failure_threshold` consecutive
///   failures. Requests go directly to the fallback provider.
/// - **HalfOpen**: After `cooldown_secs` have elapsed since the last failure,
///   one probe request is sent to the primary. On success the circuit closes;
///   on failure it reopens.
struct CircuitBreaker {
    /// Consecutive failure count.
    failure_count: AtomicU32,
    /// Timestamp (epoch secs) of last failure.
    last_failure_epoch: AtomicU64,
    /// Number of consecutive failures before opening the circuit.
    failure_threshold: u32,
    /// Seconds to wait before transitioning from Open to HalfOpen.
    cooldown_secs: u64,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// # Arguments
    /// * `failure_threshold` - Number of consecutive failures before opening
    /// * `cooldown_secs` - Seconds to wait in Open state before probing
    fn new(failure_threshold: u32, cooldown_secs: u64) -> Self {
        Self {
            failure_count: AtomicU32::new(0),
            last_failure_epoch: AtomicU64::new(0),
            failure_threshold,
            cooldown_secs,
        }
    }

    /// Compute the current circuit state from atomic counters.
    fn state(&self) -> CircuitState {
        let failures = self.failure_count.load(Ordering::Relaxed);
        if failures < self.failure_threshold {
            return CircuitState::Closed;
        }

        // Circuit has tripped -- check if cooldown has elapsed.
        let last_failure = self.last_failure_epoch.load(Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now.saturating_sub(last_failure) >= self.cooldown_secs {
            CircuitState::HalfOpen
        } else {
            CircuitState::Open
        }
    }

    /// Record a successful request -- resets the failure counter.
    fn record_success(&self) {
        let prev = self.failure_count.swap(0, Ordering::Relaxed);
        if prev >= self.failure_threshold {
            info!(
                previous_failures = prev,
                "Circuit breaker closed: primary provider recovered"
            );
        }
    }

    /// Record a failed request -- increments the failure counter and updates
    /// the last-failure timestamp.
    fn record_failure(&self) {
        let prev = self.failure_count.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_failure_epoch.store(now, Ordering::Relaxed);

        // Log the transition to Open when we cross the threshold.
        if prev + 1 == self.failure_threshold {
            info!(
                threshold = self.failure_threshold,
                "Circuit breaker opened: primary provider marked unhealthy"
            );
        }
    }
}

impl fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("state", &self.state())
            .field("failure_count", &self.failure_count.load(Ordering::Relaxed))
            .field("failure_threshold", &self.failure_threshold)
            .field("cooldown_secs", &self.cooldown_secs)
            .finish()
    }
}

/// A provider that chains a primary and a fallback LLM provider.
///
/// When a request to the primary provider fails, the error is logged and the
/// same request is forwarded to the fallback provider. If both providers fail,
/// the fallback provider's error is returned (as the more recent failure).
pub struct FallbackProvider {
    primary: Box<dyn LLMProvider>,
    fallback: Box<dyn LLMProvider>,
    /// Pre-computed composite name in the form `"primary -> fallback"`.
    composite_name: String,
    /// Circuit breaker tracking primary provider health.
    circuit_breaker: CircuitBreaker,
    /// Per-reason cooldown tracker for smarter provider skipping.
    cooldown: CooldownTracker,
}

impl fmt::Debug for FallbackProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FallbackProvider")
            .field("primary", &self.primary.name())
            .field("fallback", &self.fallback.name())
            .field("circuit_breaker", &self.circuit_breaker)
            .field("cooldown", &"CooldownTracker")
            .finish()
    }
}

impl FallbackProvider {
    /// Create a new fallback provider.
    ///
    /// # Arguments
    /// * `primary` - The preferred provider, tried first for every request.
    /// * `fallback` - The backup provider, used only when the primary fails.
    pub fn new(primary: Box<dyn LLMProvider>, fallback: Box<dyn LLMProvider>) -> Self {
        let composite_name = format!("{} -> {}", primary.name(), fallback.name());
        Self {
            primary,
            fallback,
            composite_name,
            circuit_breaker: CircuitBreaker::new(3, 30),
            cooldown: CooldownTracker::new(),
        }
    }
}

#[async_trait]
impl LLMProvider for FallbackProvider {
    fn name(&self) -> &str {
        &self.composite_name
    }

    fn default_model(&self) -> &str {
        self.primary.default_model()
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        let circuit_state = self.circuit_breaker.state();

        // When the circuit is Open or per-reason cooldown is active, skip the primary entirely.
        if circuit_state == CircuitState::Open || self.cooldown.is_in_cooldown(self.primary.name())
        {
            info!(
                primary = self.primary.name(),
                fallback = self.fallback.name(),
                "Circuit open or cooldown active: skipping primary, using fallback directly"
            );
            return self.fallback.chat(messages, tools, model, options).await;
        }

        // Closed or HalfOpen -- try the primary provider.
        match self
            .primary
            .chat(messages.clone(), tools.clone(), model, options.clone())
            .await
        {
            Ok(response) => {
                self.circuit_breaker.record_success();
                self.cooldown.mark_success(self.primary.name());
                Ok(response)
            }
            Err(primary_err) => {
                // Don't fallback for auth/billing/invalid request errors
                let should_fallback = match &primary_err {
                    crate::error::ZeptoError::ProviderTyped(pe) => pe.should_fallback(),
                    _ => true, // Legacy errors always fallback
                };

                if should_fallback {
                    self.circuit_breaker.record_failure();
                    // Classify the error and apply per-reason cooldown.
                    let reason = match &primary_err {
                        crate::error::ZeptoError::ProviderTyped(pe) => {
                            FailoverReason::from_provider_error(pe)
                        }
                        _ => FailoverReason::Unknown,
                    };
                    self.cooldown.mark_failure(self.primary.name(), reason);
                    warn!(
                        primary = self.primary.name(),
                        fallback = self.fallback.name(),
                        error = %primary_err,
                        circuit_state = ?self.circuit_breaker.state(),
                        ?reason,
                        "Primary provider failed, falling back"
                    );
                    self.fallback.chat(messages, tools, model, options).await
                } else {
                    warn!(
                        primary = self.primary.name(),
                        error = %primary_err,
                        "Primary provider error is non-recoverable, skipping fallback"
                    );
                    Err(primary_err)
                }
            }
        }
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let circuit_state = self.circuit_breaker.state();

        // When the circuit is Open or per-reason cooldown is active, skip the primary entirely.
        if circuit_state == CircuitState::Open || self.cooldown.is_in_cooldown(self.primary.name())
        {
            info!(
                primary = self.primary.name(),
                fallback = self.fallback.name(),
                "Circuit open or cooldown active: skipping primary streaming, using fallback directly"
            );
            return self
                .fallback
                .chat_stream(messages, tools, model, options)
                .await;
        }

        // Closed or HalfOpen -- try the primary provider.
        match self
            .primary
            .chat_stream(messages.clone(), tools.clone(), model, options.clone())
            .await
        {
            Ok(receiver) => {
                self.circuit_breaker.record_success();
                self.cooldown.mark_success(self.primary.name());
                Ok(receiver)
            }
            Err(primary_err) => {
                // Don't fallback for auth/billing/invalid request errors
                let should_fallback = match &primary_err {
                    crate::error::ZeptoError::ProviderTyped(pe) => pe.should_fallback(),
                    _ => true, // Legacy errors always fallback
                };

                if should_fallback {
                    self.circuit_breaker.record_failure();
                    // Classify the error and apply per-reason cooldown.
                    let reason = match &primary_err {
                        crate::error::ZeptoError::ProviderTyped(pe) => {
                            FailoverReason::from_provider_error(pe)
                        }
                        _ => FailoverReason::Unknown,
                    };
                    self.cooldown.mark_failure(self.primary.name(), reason);
                    warn!(
                        primary = self.primary.name(),
                        fallback = self.fallback.name(),
                        error = %primary_err,
                        circuit_state = ?self.circuit_breaker.state(),
                        ?reason,
                        "Primary provider streaming failed, falling back"
                    );
                    self.fallback
                        .chat_stream(messages, tools, model, options)
                        .await
                } else {
                    warn!(
                        primary = self.primary.name(),
                        error = %primary_err,
                        "Primary provider streaming error is non-recoverable, skipping fallback"
                    );
                    Err(primary_err)
                }
            }
        }
    }

    /// Delegate embed() to the primary provider.
    ///
    /// Embeddings are not subject to fallback logic — the primary provider is
    /// the authoritative embedding source (its model is the one configured).
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        self.primary.embed(texts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ZeptoError;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // ---------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------

    /// A provider that always returns a successful response.
    struct SuccessProvider {
        name: &'static str,
    }

    impl fmt::Debug for SuccessProvider {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("SuccessProvider")
                .field("name", &self.name)
                .finish()
        }
    }

    #[async_trait]
    impl LLMProvider for SuccessProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn default_model(&self) -> &str {
            "success-model-v1"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Ok(LLMResponse::text(&format!("success from {}", self.name)))
        }
    }

    /// A provider that always returns an error.
    struct FailProvider {
        name: &'static str,
    }

    impl fmt::Debug for FailProvider {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("FailProvider")
                .field("name", &self.name)
                .finish()
        }
    }

    #[async_trait]
    impl LLMProvider for FailProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn default_model(&self) -> &str {
            "fail-model-v1"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Err(ZeptoError::Provider("provider failed".into()))
        }
    }

    /// A provider that counts how many times `chat()` is called and returns success.
    struct CountingProvider {
        name: &'static str,
        call_count: Arc<AtomicU32>,
    }

    impl fmt::Debug for CountingProvider {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("CountingProvider")
                .field("name", &self.name)
                .field("call_count", &self.call_count.load(Ordering::SeqCst))
                .finish()
        }
    }

    #[async_trait]
    impl LLMProvider for CountingProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn default_model(&self) -> &str {
            "counting-model-v1"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(LLMResponse::text(&format!("success from {}", self.name)))
        }
    }

    // ---------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------

    #[test]
    fn test_fallback_provider_name() {
        let provider = FallbackProvider::new(
            Box::new(SuccessProvider { name: "alpha" }),
            Box::new(SuccessProvider { name: "beta" }),
        );

        assert_eq!(provider.name(), "alpha -> beta");
    }

    #[test]
    fn test_fallback_provider_default_model() {
        let provider = FallbackProvider::new(
            Box::new(SuccessProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        // Should delegate to primary's default_model.
        assert_eq!(provider.default_model(), "success-model-v1");
    }

    #[tokio::test]
    async fn test_fallback_uses_primary_when_available() {
        let provider = FallbackProvider::new(
            Box::new(SuccessProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("primary should succeed");

        assert_eq!(response.content, "success from primary");
    }

    #[tokio::test]
    async fn test_fallback_uses_secondary_on_primary_failure() {
        let provider = FallbackProvider::new(
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("fallback should succeed after primary failure");

        assert_eq!(response.content, "success from fallback");
    }

    #[tokio::test]
    async fn test_fallback_returns_error_when_both_fail() {
        let provider = FallbackProvider::new(
            Box::new(FailProvider { name: "primary" }),
            Box::new(FailProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ZeptoError::Provider(_)),
            "expected Provider error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_fallback_primary_not_called_twice() {
        let call_count = Arc::new(AtomicU32::new(0));

        let provider = FallbackProvider::new(
            Box::new(CountingProvider {
                name: "primary",
                call_count: Arc::clone(&call_count),
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("primary should succeed");

        assert_eq!(response.content, "success from primary");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "primary should be called exactly once"
        );
    }

    // ====================================================================
    // ProviderTyped fallback behavior tests
    // ====================================================================

    /// A provider that fails with a specific ProviderTyped error.
    struct TypedFailProvider {
        name: &'static str,
        error: fn() -> ZeptoError,
    }

    #[async_trait]
    impl LLMProvider for TypedFailProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn default_model(&self) -> &str {
            "typed-fail-model"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Err((self.error)())
        }
    }

    #[tokio::test]
    async fn test_fallback_auth_error_does_not_trigger_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || ZeptoError::ProviderTyped(ProviderError::Auth("invalid key".into())),
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Auth error should NOT trigger fallback — request should fail
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Authentication error"));
    }

    #[tokio::test]
    async fn test_fallback_billing_error_does_not_trigger_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || ZeptoError::ProviderTyped(ProviderError::Billing("no funds".into())),
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Billing error"));
    }

    #[tokio::test]
    async fn test_fallback_invalid_request_does_not_trigger_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || {
                    ZeptoError::ProviderTyped(ProviderError::InvalidRequest("bad json".into()))
                },
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid request"));
    }

    #[tokio::test]
    async fn test_fallback_rate_limit_triggers_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || {
                    ZeptoError::ProviderTyped(ProviderError::RateLimit("quota exceeded".into()))
                },
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Rate limit SHOULD trigger fallback
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
    }

    #[tokio::test]
    async fn test_fallback_server_error_triggers_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || {
                    ZeptoError::ProviderTyped(ProviderError::ServerError("internal error".into()))
                },
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
    }

    #[tokio::test]
    async fn test_fallback_timeout_triggers_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || ZeptoError::ProviderTyped(ProviderError::Timeout("timed out".into())),
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
    }

    #[tokio::test]
    async fn test_fallback_model_not_found_triggers_fallback() {
        use crate::error::ProviderError;

        let provider = FallbackProvider::new(
            Box::new(TypedFailProvider {
                name: "primary",
                error: || ZeptoError::ProviderTyped(ProviderError::ModelNotFound("gpt-99".into())),
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // ModelNotFound should trigger fallback (different provider may have the model)
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
    }

    #[tokio::test]
    async fn test_fallback_legacy_provider_error_triggers_fallback() {
        // Legacy Provider(String) errors should always trigger fallback (backward compat)
        let provider = FallbackProvider::new(
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
    }

    // ====================================================================
    // Circuit breaker unit tests
    // ====================================================================

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(3, 30);
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new(3, 30);

        // Record 3 consecutive failures.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();

        // After 3 failures and with cooldown still active, circuit should be Open.
        // (last_failure_epoch is "now", so cooldown has NOT elapsed.)
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let cb = CircuitBreaker::new(3, 30);

        // Record 2 failures (below threshold).
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        // Success resets counter.
        cb.record_success();
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
        assert_eq!(cb.state(), CircuitState::Closed);

        // Even after 2 more failures it should still be Closed (need 3 total).
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_halfopen_after_cooldown() {
        let cb = CircuitBreaker::new(3, 1); // 1 second cooldown for fast test

        // Trip the circuit.
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Simulate cooldown elapsed by back-dating the last failure timestamp.
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 2; // 2 seconds ago
        cb.last_failure_epoch.store(past, Ordering::Relaxed);

        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_circuit_breaker_halfopen_success_closes() {
        let cb = CircuitBreaker::new(3, 1);

        // Trip the circuit and simulate cooldown elapsed.
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 2;
        cb.last_failure_epoch.store(past, Ordering::Relaxed);
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Success in HalfOpen should close the circuit.
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_circuit_breaker_halfopen_failure_reopens() {
        let cb = CircuitBreaker::new(3, 30);

        // Trip the circuit and simulate cooldown elapsed.
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 31;
        cb.last_failure_epoch.store(past, Ordering::Relaxed);
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Another failure in HalfOpen should reopen the circuit.
        cb.record_failure();
        // failure_count is now 4 (>= threshold) and last_failure_epoch is "now"
        // so cooldown has NOT elapsed → Open.
        assert_eq!(cb.state(), CircuitState::Open);
    }

    // ====================================================================
    // Circuit breaker integration tests (with FallbackProvider)
    // ====================================================================

    /// A provider that counts calls and always fails.
    struct CountingFailProvider {
        name: &'static str,
        call_count: Arc<AtomicU32>,
    }

    impl fmt::Debug for CountingFailProvider {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("CountingFailProvider")
                .field("name", &self.name)
                .field("call_count", &self.call_count.load(Ordering::SeqCst))
                .finish()
        }
    }

    #[async_trait]
    impl LLMProvider for CountingFailProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn default_model(&self) -> &str {
            "counting-fail-model"
        }

        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(ZeptoError::Provider("provider failed".into()))
        }
    }

    #[tokio::test]
    async fn test_fallback_skips_primary_when_circuit_open() {
        let primary_calls = Arc::new(AtomicU32::new(0));
        let fallback_calls = Arc::new(AtomicU32::new(0));

        let provider = FallbackProvider::new(
            Box::new(CountingFailProvider {
                name: "primary",
                call_count: Arc::clone(&primary_calls),
            }),
            Box::new(CountingProvider {
                name: "fallback",
                call_count: Arc::clone(&fallback_calls),
            }),
        );

        // One failure is enough: after primary fails, CooldownTracker puts it
        // in cooldown immediately, so subsequent calls bypass primary.
        let _ = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Primary was called once, fallback was called once.
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);

        // Now cooldown is active. Next call should NOT call primary.
        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
        // Primary should NOT have been called again (cooldown active).
        assert_eq!(
            primary_calls.load(Ordering::SeqCst),
            1,
            "primary should be skipped while in cooldown"
        );
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_fallback_probes_primary_when_halfopen() {
        let primary_calls = Arc::new(AtomicU32::new(0));
        let fallback_calls = Arc::new(AtomicU32::new(0));

        let provider = FallbackProvider::new(
            Box::new(CountingFailProvider {
                name: "primary",
                call_count: Arc::clone(&primary_calls),
            }),
            Box::new(CountingProvider {
                name: "fallback",
                call_count: Arc::clone(&fallback_calls),
            }),
        );

        // With CooldownTracker, after the first failure primary is in cooldown.
        // To trip the circuit breaker (needs 3 failures), directly manipulate
        // the atomic counter — this simulates 3 consecutive circuit failures.
        provider
            .circuit_breaker
            .failure_count
            .store(3, Ordering::Relaxed);
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        provider
            .circuit_breaker
            .last_failure_epoch
            .store(now_epoch, Ordering::Relaxed);

        assert_eq!(provider.circuit_breaker.state(), CircuitState::Open);

        // Also make sure the CooldownTracker is clear (no cooldown active)
        // so that the HalfOpen probe path is exercised, not the cooldown bypass.
        provider.cooldown.mark_success("primary");

        // Simulate circuit cooldown elapsed by back-dating the last failure timestamp.
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 31; // 31 seconds ago (> default 30s cooldown)
        provider
            .circuit_breaker
            .last_failure_epoch
            .store(past, Ordering::Relaxed);

        assert_eq!(provider.circuit_breaker.state(), CircuitState::HalfOpen);
        assert!(!provider.cooldown.is_in_cooldown("primary"));

        // Next call should probe the primary (HalfOpen allows one request, cooldown is clear).
        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Primary was called (probe) but failed, so fallback was used.
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from fallback");
        assert_eq!(
            primary_calls.load(Ordering::SeqCst),
            1,
            "primary should be probed once in HalfOpen state"
        );
    }
}
