//! Provider rotation for ZeptoClaw
//!
//! This module provides a [`RotationProvider`] that manages N LLM providers
//! with health-aware selection. Unlike [`FallbackProvider`] which chains exactly
//! two providers, `RotationProvider` supports 3+ providers with configurable
//! rotation strategies (Priority or RoundRobin).
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::providers::rotation::{RotationProvider, RotationStrategy};
//! use zeptoclaw::providers::claude::ClaudeProvider;
//! use zeptoclaw::providers::openai::OpenAIProvider;
//!
//! let providers: Vec<Box<dyn LLMProvider>> = vec![
//!     Box::new(ClaudeProvider::new("claude-key")),
//!     Box::new(OpenAIProvider::new("openai-key")),
//!     Box::new(OpenAIProvider::with_base("groq-key", "https://api.groq.com/openai/v1")),
//! ];
//! let provider = RotationProvider::new(providers, RotationStrategy::Priority, 3, 30);
//! // Requests go to the first healthy provider. Unhealthy ones are skipped.
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::session::Message;

use super::{ChatOptions, LLMProvider, LLMResponse, StreamEvent, ToolDefinition};

// ============================================================================
// Rotation Strategy
// ============================================================================

/// Rotation strategy for selecting among healthy providers.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RotationStrategy {
    /// Use providers in configured order, skip unhealthy ones.
    #[default]
    Priority,
    /// Round-robin across healthy providers.
    RoundRobin,
}

// ============================================================================
// Provider Health
// ============================================================================

/// Health state for a single provider in the rotation.
struct ProviderHealth {
    /// Consecutive failure count.
    failure_count: AtomicU32,
    /// Timestamp (epoch secs) of last failure.
    last_failure_epoch: AtomicU64,
    /// Number of consecutive failures before marking unhealthy.
    failure_threshold: u32,
    /// Seconds before retrying an unhealthy provider.
    cooldown_secs: u64,
}

impl ProviderHealth {
    /// Create a new health tracker.
    fn new(failure_threshold: u32, cooldown_secs: u64) -> Self {
        Self {
            failure_count: AtomicU32::new(0),
            last_failure_epoch: AtomicU64::new(0),
            failure_threshold,
            cooldown_secs,
        }
    }

    /// Returns `true` if this provider is considered healthy (below failure threshold
    /// or cooldown has elapsed and it should be probed).
    fn is_healthy(&self) -> bool {
        let failures = self.failure_count.load(Ordering::Relaxed);
        if failures < self.failure_threshold {
            return true;
        }

        // Check if cooldown has elapsed (half-open equivalent).
        let last_failure = self.last_failure_epoch.load(Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(last_failure) >= self.cooldown_secs
    }

    /// Record a successful request -- resets the failure counter.
    fn record_success(&self) {
        let prev = self.failure_count.swap(0, Ordering::Relaxed);
        if prev >= self.failure_threshold {
            info!(
                previous_failures = prev,
                "Rotation: provider recovered, resetting health"
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

        if prev + 1 == self.failure_threshold {
            info!(
                threshold = self.failure_threshold,
                "Rotation: provider marked unhealthy"
            );
        }
    }
}

impl fmt::Debug for ProviderHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderHealth")
            .field("failure_count", &self.failure_count.load(Ordering::Relaxed))
            .field("failure_threshold", &self.failure_threshold)
            .field("cooldown_secs", &self.cooldown_secs)
            .field("is_healthy", &self.is_healthy())
            .finish()
    }
}

// ============================================================================
// RotationProvider
// ============================================================================

/// A provider that rotates across multiple LLM providers with health tracking.
///
/// Supports two strategies:
/// - **Priority**: iterate providers in order, skip unhealthy ones, use first healthy.
/// - **RoundRobin**: advance index atomically, skip unhealthy, wrap around.
///
/// When ALL providers are unhealthy, falls back to the one that has been
/// unhealthy the longest (most likely to have recovered).
pub struct RotationProvider {
    providers: Vec<(Box<dyn LLMProvider>, ProviderHealth)>,
    strategy: RotationStrategy,
    /// Atomic counter for round-robin (wraps around).
    round_robin_index: AtomicU32,
    /// Pre-computed composite name.
    composite_name: String,
}

impl fmt::Debug for RotationProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.providers.iter().map(|(p, _)| p.name()).collect();
        f.debug_struct("RotationProvider")
            .field("providers", &names)
            .field("strategy", &self.strategy)
            .finish()
    }
}

impl RotationProvider {
    /// Create a new rotation provider.
    ///
    /// # Arguments
    /// * `providers` - The LLM providers to rotate across (order matters for Priority).
    /// * `strategy` - Rotation strategy (Priority or RoundRobin).
    /// * `failure_threshold` - Consecutive failures before marking a provider unhealthy.
    /// * `cooldown_secs` - Seconds to wait before retrying an unhealthy provider.
    ///
    /// # Panics
    /// Panics if `providers` is empty.
    pub fn new(
        providers: Vec<Box<dyn LLMProvider>>,
        strategy: RotationStrategy,
        failure_threshold: u32,
        cooldown_secs: u64,
    ) -> Self {
        assert!(
            !providers.is_empty(),
            "RotationProvider requires at least one provider"
        );

        let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
        let composite_name = format!("rotation({})", names.join(", "));

        let providers = providers
            .into_iter()
            .map(|p| {
                let health = ProviderHealth::new(failure_threshold, cooldown_secs);
                (p, health)
            })
            .collect();

        Self {
            providers,
            strategy,
            round_robin_index: AtomicU32::new(0),
            composite_name,
        }
    }

    /// Select the index of the provider to use based on the current strategy.
    ///
    /// Returns the index into `self.providers`.
    fn select_provider_index(&self) -> usize {
        let len = self.providers.len();

        match self.strategy {
            RotationStrategy::Priority => {
                // Try providers in order, skip unhealthy ones.
                for i in 0..len {
                    if self.providers[i].1.is_healthy() {
                        return i;
                    }
                }
                // All unhealthy: use the one with the oldest last_failure (most likely recovered).
                self.oldest_unhealthy_index()
            }
            RotationStrategy::RoundRobin => {
                // Advance index atomically. Try up to `len` positions.
                let start = self.round_robin_index.fetch_add(1, Ordering::Relaxed) as usize;
                for offset in 0..len {
                    let i = (start + offset) % len;
                    if self.providers[i].1.is_healthy() {
                        return i;
                    }
                }
                // All unhealthy: use the one with the oldest last_failure.
                self.oldest_unhealthy_index()
            }
        }
    }

    /// Find the provider with the oldest last_failure_epoch (most likely to have recovered).
    fn oldest_unhealthy_index(&self) -> usize {
        self.providers
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, h))| h.last_failure_epoch.load(Ordering::Relaxed))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Determine whether an error should trigger rotation to the next provider.
    fn should_rotate(err: &crate::error::ZeptoError) -> bool {
        match err {
            crate::error::ZeptoError::ProviderTyped(pe) => pe.should_fallback(),
            _ => true, // Legacy errors always rotate
        }
    }
}

#[async_trait]
impl LLMProvider for RotationProvider {
    fn name(&self) -> &str {
        &self.composite_name
    }

    fn default_model(&self) -> &str {
        self.providers[0].0.default_model()
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        let len = self.providers.len();
        let start_index = self.select_provider_index();
        let mut last_err = None;

        // Try starting from selected provider, then rotate through others.
        for offset in 0..len {
            let i = (start_index + offset) % len;
            let (provider, health) = &self.providers[i];

            // Skip unhealthy providers (except the start index which was already selected).
            if offset > 0 && !health.is_healthy() {
                continue;
            }

            match provider
                .chat(messages.clone(), tools.clone(), model, options.clone())
                .await
            {
                Ok(response) => {
                    health.record_success();
                    return Ok(response);
                }
                Err(err) => {
                    if Self::should_rotate(&err) {
                        health.record_failure();
                        warn!(
                            provider = provider.name(),
                            error = %err,
                            "Rotation: provider failed, trying next"
                        );
                        last_err = Some(err);
                    } else {
                        // Non-recoverable error (auth, billing, invalid request):
                        // do not rotate, return error immediately.
                        warn!(
                            provider = provider.name(),
                            error = %err,
                            "Rotation: non-recoverable error, not rotating"
                        );
                        return Err(err);
                    }
                }
            }
        }

        // All providers failed.
        Err(last_err.unwrap_or_else(|| {
            crate::error::ZeptoError::Provider("All rotation providers failed".into())
        }))
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let len = self.providers.len();
        let start_index = self.select_provider_index();
        let mut last_err = None;

        for offset in 0..len {
            let i = (start_index + offset) % len;
            let (provider, health) = &self.providers[i];

            if offset > 0 && !health.is_healthy() {
                continue;
            }

            match provider
                .chat_stream(messages.clone(), tools.clone(), model, options.clone())
                .await
            {
                Ok(receiver) => {
                    health.record_success();
                    return Ok(receiver);
                }
                Err(err) => {
                    if Self::should_rotate(&err) {
                        health.record_failure();
                        warn!(
                            provider = provider.name(),
                            error = %err,
                            "Rotation: provider streaming failed, trying next"
                        );
                        last_err = Some(err);
                    } else {
                        warn!(
                            provider = provider.name(),
                            error = %err,
                            "Rotation: non-recoverable streaming error, not rotating"
                        );
                        return Err(err);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            crate::error::ZeptoError::Provider("All rotation providers failed (streaming)".into())
        }))
    }

    /// Delegate embed() to the currently selected provider.
    ///
    /// Uses the same provider-selection logic as `chat()` so the embedding
    /// request lands on the healthiest available provider in the rotation.
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        let index = self.select_provider_index();
        self.providers[index].0.embed(texts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ProviderError, ZeptoError};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // ---------------------------------------------------------------
    // Test helpers (mirroring fallback.rs patterns)
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

    /// A provider that fails with a specific ProviderTyped error.
    struct TypedFailProvider {
        name: &'static str,
        error: fn() -> ZeptoError,
    }

    impl fmt::Debug for TypedFailProvider {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TypedFailProvider")
                .field("name", &self.name)
                .finish()
        }
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

    // ---------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------

    #[test]
    fn test_rotation_name() {
        let provider = RotationProvider::new(
            vec![
                Box::new(SuccessProvider { name: "claude" }),
                Box::new(SuccessProvider { name: "openai" }),
                Box::new(SuccessProvider { name: "groq" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        assert_eq!(provider.name(), "rotation(claude, openai, groq)");
    }

    #[test]
    fn test_rotation_default_model() {
        let provider = RotationProvider::new(
            vec![
                Box::new(SuccessProvider { name: "claude" }),
                Box::new(SuccessProvider { name: "openai" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        // Should delegate to first provider's default_model.
        assert_eq!(provider.default_model(), "success-model-v1");
    }

    #[tokio::test]
    async fn test_rotation_priority_uses_first_healthy() {
        let calls_a = Arc::new(AtomicU32::new(0));
        let calls_b = Arc::new(AtomicU32::new(0));
        let calls_c = Arc::new(AtomicU32::new(0));

        let provider = RotationProvider::new(
            vec![
                Box::new(CountingProvider {
                    name: "alpha",
                    call_count: Arc::clone(&calls_a),
                }),
                Box::new(CountingProvider {
                    name: "beta",
                    call_count: Arc::clone(&calls_b),
                }),
                Box::new(CountingProvider {
                    name: "gamma",
                    call_count: Arc::clone(&calls_c),
                }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("should succeed");

        assert_eq!(response.content, "success from alpha");
        assert_eq!(calls_a.load(Ordering::SeqCst), 1);
        assert_eq!(calls_b.load(Ordering::SeqCst), 0);
        assert_eq!(calls_c.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_rotation_round_robin() {
        let calls_a = Arc::new(AtomicU32::new(0));
        let calls_b = Arc::new(AtomicU32::new(0));
        let calls_c = Arc::new(AtomicU32::new(0));

        let provider = RotationProvider::new(
            vec![
                Box::new(CountingProvider {
                    name: "alpha",
                    call_count: Arc::clone(&calls_a),
                }),
                Box::new(CountingProvider {
                    name: "beta",
                    call_count: Arc::clone(&calls_b),
                }),
                Box::new(CountingProvider {
                    name: "gamma",
                    call_count: Arc::clone(&calls_c),
                }),
            ],
            RotationStrategy::RoundRobin,
            3,
            30,
        );

        // Make 3 calls — each should go to a different provider.
        for _ in 0..3 {
            let _ = provider
                .chat(vec![], vec![], None, ChatOptions::default())
                .await
                .expect("should succeed");
        }

        assert_eq!(calls_a.load(Ordering::SeqCst), 1);
        assert_eq!(calls_b.load(Ordering::SeqCst), 1);
        assert_eq!(calls_c.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_rotation_records_failure() {
        let provider = RotationProvider::new(
            vec![
                Box::new(FailProvider { name: "alpha" }),
                Box::new(SuccessProvider { name: "beta" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        // First call: alpha fails, beta succeeds.
        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("beta should succeed");

        assert_eq!(response.content, "success from beta");
        // Alpha should have 1 failure recorded.
        assert_eq!(
            provider.providers[0]
                .1
                .failure_count
                .load(Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn test_rotation_records_success_resets() {
        let provider = RotationProvider::new(
            vec![
                Box::new(SuccessProvider { name: "alpha" }),
                Box::new(SuccessProvider { name: "beta" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        // Simulate 2 prior failures on alpha.
        provider.providers[0]
            .1
            .failure_count
            .store(2, Ordering::Relaxed);

        // Alpha succeeds, should reset failure count.
        let _ = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("should succeed");

        assert_eq!(
            provider.providers[0]
                .1
                .failure_count
                .load(Ordering::Relaxed),
            0
        );
    }

    #[tokio::test]
    async fn test_rotation_skips_unhealthy() {
        let calls_a = Arc::new(AtomicU32::new(0));
        let calls_b = Arc::new(AtomicU32::new(0));

        let provider = RotationProvider::new(
            vec![
                Box::new(CountingFailProvider {
                    name: "alpha",
                    call_count: Arc::clone(&calls_a),
                }),
                Box::new(CountingProvider {
                    name: "beta",
                    call_count: Arc::clone(&calls_b),
                }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        // Trip alpha past the failure threshold (3 failures).
        for _ in 0..3 {
            let _ = provider
                .chat(vec![], vec![], None, ChatOptions::default())
                .await;
        }

        assert_eq!(calls_a.load(Ordering::SeqCst), 3);
        assert_eq!(calls_b.load(Ordering::SeqCst), 3); // beta was called as fallback

        // Now alpha is unhealthy. Next call should skip alpha entirely.
        let prev_a = calls_a.load(Ordering::SeqCst);
        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("beta should succeed");

        assert_eq!(response.content, "success from beta");
        // Alpha should NOT have been called again.
        assert_eq!(
            calls_a.load(Ordering::SeqCst),
            prev_a,
            "unhealthy alpha should be skipped"
        );
    }

    #[tokio::test]
    async fn test_rotation_all_unhealthy_uses_oldest() {
        let provider = RotationProvider::new(
            vec![
                Box::new(FailProvider { name: "alpha" }),
                Box::new(FailProvider { name: "beta" }),
                Box::new(FailProvider { name: "gamma" }),
            ],
            RotationStrategy::Priority,
            1, // threshold of 1 so each provider fails once to become unhealthy
            30,
        );

        // Trip all providers: each fails once to cross threshold=1.
        for _ in 0..3 {
            let _ = provider
                .chat(vec![], vec![], None, ChatOptions::default())
                .await;
        }

        // Now all are unhealthy. Set different last_failure_epoch values.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // alpha failed 100s ago (oldest — should be selected).
        provider.providers[0]
            .1
            .last_failure_epoch
            .store(now - 100, Ordering::Relaxed);
        // beta failed 50s ago.
        provider.providers[1]
            .1
            .last_failure_epoch
            .store(now - 50, Ordering::Relaxed);
        // gamma failed 10s ago (most recent).
        provider.providers[2]
            .1
            .last_failure_epoch
            .store(now - 10, Ordering::Relaxed);

        // Select should choose alpha (oldest failure).
        let selected = provider.select_provider_index();
        assert_eq!(selected, 0, "should select provider with oldest failure");
    }

    #[tokio::test]
    async fn test_rotation_auth_error_no_rotation() {
        let provider = RotationProvider::new(
            vec![
                Box::new(TypedFailProvider {
                    name: "alpha",
                    error: || ZeptoError::ProviderTyped(ProviderError::Auth("invalid key".into())),
                }),
                Box::new(SuccessProvider { name: "beta" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Auth error should NOT trigger rotation — request should fail immediately.
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Authentication error"));
    }

    #[tokio::test]
    async fn test_rotation_rate_limit_triggers_rotation() {
        let provider = RotationProvider::new(
            vec![
                Box::new(TypedFailProvider {
                    name: "alpha",
                    error: || {
                        ZeptoError::ProviderTyped(ProviderError::RateLimit("quota exceeded".into()))
                    },
                }),
                Box::new(SuccessProvider { name: "beta" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Rate limit SHOULD trigger rotation to beta.
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from beta");
    }

    #[tokio::test]
    async fn test_rotation_single_provider() {
        let provider = RotationProvider::new(
            vec![Box::new(SuccessProvider { name: "solo" })],
            RotationStrategy::Priority,
            3,
            30,
        );

        assert_eq!(provider.name(), "rotation(solo)");

        let response = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await
            .expect("should succeed");

        assert_eq!(response.content, "success from solo");
    }

    #[test]
    fn test_rotation_config_defaults() {
        use crate::config::RotationConfig;

        let config = RotationConfig::default();
        assert!(!config.enabled);
        assert!(config.order.is_empty());
        assert_eq!(config.strategy, RotationStrategy::Priority);
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.cooldown_secs, 30);
    }

    #[test]
    fn test_rotation_strategy_serialize() {
        let strategy = RotationStrategy::RoundRobin;
        let json = serde_json::to_string(&strategy).unwrap();
        assert_eq!(json, "\"round_robin\"");

        let parsed: RotationStrategy = serde_json::from_str("\"priority\"").unwrap();
        assert_eq!(parsed, RotationStrategy::Priority);
    }

    #[tokio::test]
    async fn test_rotation_billing_error_no_rotation() {
        let provider = RotationProvider::new(
            vec![
                Box::new(TypedFailProvider {
                    name: "alpha",
                    error: || ZeptoError::ProviderTyped(ProviderError::Billing("no funds".into())),
                }),
                Box::new(SuccessProvider { name: "beta" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        // Billing error should NOT trigger rotation.
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Billing error"));
    }

    #[tokio::test]
    async fn test_rotation_server_error_triggers_rotation() {
        let provider = RotationProvider::new(
            vec![
                Box::new(TypedFailProvider {
                    name: "alpha",
                    error: || {
                        ZeptoError::ProviderTyped(ProviderError::ServerError(
                            "internal error".into(),
                        ))
                    },
                }),
                Box::new(SuccessProvider { name: "beta" }),
                Box::new(SuccessProvider { name: "gamma" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "success from beta");
    }

    #[tokio::test]
    async fn test_rotation_all_fail_returns_last_error() {
        let provider = RotationProvider::new(
            vec![
                Box::new(FailProvider { name: "alpha" }),
                Box::new(FailProvider { name: "beta" }),
            ],
            RotationStrategy::Priority,
            3,
            30,
        );

        let result = provider
            .chat(vec![], vec![], None, ChatOptions::default())
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn test_provider_health_starts_healthy() {
        let health = ProviderHealth::new(3, 30);
        assert!(health.is_healthy());
        assert_eq!(health.failure_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_provider_health_becomes_unhealthy() {
        let health = ProviderHealth::new(3, 30);
        health.record_failure();
        assert!(health.is_healthy());
        health.record_failure();
        assert!(health.is_healthy());
        health.record_failure();
        // After 3 failures with cooldown still active, should be unhealthy.
        assert!(!health.is_healthy());
    }

    #[test]
    fn test_provider_health_recovers_after_cooldown() {
        let health = ProviderHealth::new(3, 1); // 1s cooldown
        health.record_failure();
        health.record_failure();
        health.record_failure();
        assert!(!health.is_healthy());

        // Simulate cooldown elapsed by backdating last_failure_epoch.
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 2; // 2 seconds ago
        health.last_failure_epoch.store(past, Ordering::Relaxed);

        assert!(health.is_healthy());
    }

    #[test]
    fn test_provider_health_success_resets() {
        let health = ProviderHealth::new(3, 30);
        health.record_failure();
        health.record_failure();
        health.record_success();
        assert_eq!(health.failure_count.load(Ordering::Relaxed), 0);
        assert!(health.is_healthy());
    }
}
