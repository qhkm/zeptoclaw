//! Provider chain assembly for ZeptoKernel.
//!
//! Functions extracted (moved, not rewritten) from `cli/common.rs:139–384`.
//! Handles provider resolution, fallback chain, retry wrapper, quota wrapper,
//! and OAuth credential refresh.

use std::sync::Arc;

use tracing::warn;

use crate::auth::{self, AuthMethod};
use crate::config::Config;
use crate::providers::{
    provider_config_by_name, resolve_runtime_providers, ClaudeProvider, FallbackProvider,
    GeminiProvider, LLMProvider, OpenAIProvider, RetryProvider, RuntimeProviderSelection,
};

/// Build the complete provider chain from config.
///
/// Refreshes OAuth credentials, resolves runtime providers in registry order,
/// optionally wraps with fallback chain and retry decorator.
/// Returns `None` if no providers are configured.
pub async fn build_provider_chain(
    config: &Config,
) -> Option<(Arc<dyn LLMProvider>, Vec<&'static str>)> {
    refresh_oauth_credentials_if_needed(config).await;
    let (chain, names) = build_runtime_provider_chain(config)?;
    let chain = apply_retry_wrapper(chain, config);
    Some((Arc::from(chain), names))
}

/// Create a provider from a runtime selection entry.
///
/// Maps backend name ("anthropic", "openai") to the corresponding provider
/// struct, handling Gemini routing, OAuth credentials, and OpenAI-compatible
/// presets.
///
/// Moved from `cli/common.rs:139–199`.
pub fn provider_from_runtime_selection(
    selection: &RuntimeProviderSelection,
    configured_model: &str,
) -> Option<Box<dyn LLMProvider>> {
    match selection.backend {
        "anthropic" => {
            // Use credential-aware constructor when OAuth token is available
            if selection.credential.is_bearer() {
                Some(Box::new(ClaudeProvider::with_credential(
                    selection.credential.clone(),
                )))
            } else {
                Some(Box::new(ClaudeProvider::new(&selection.api_key)))
            }
        }
        "openai" => {
            // Route ALL Gemini selections through the native GeminiProvider, which
            // speaks the Gemini REST API directly and applies thinking-model filtering
            // (extract_text skips parts tagged `thought: true`).  This applies to
            // both OAuth bearer tokens (from Gemini CLI) and plain API keys.
            if selection.name == "gemini" {
                // Use the user-configured model, falling back to the built-in default.
                // from_config handles the full auth priority chain:
                //   config key → GEMINI_API_KEY → GOOGLE_API_KEY → Gemini CLI OAuth
                let model = if configured_model.is_empty() {
                    GeminiProvider::default_gemini_model()
                } else {
                    configured_model
                };
                let api_key = if selection.credential.is_bearer() {
                    None
                } else {
                    Some(selection.api_key.as_str())
                };
                let prefer_oauth = selection.credential.is_bearer();
                return GeminiProvider::from_config(api_key, model, prefer_oauth)
                    .map(|p| Box::new(p) as Box<dyn LLMProvider>);
            }
            let api_base = match selection.api_base.as_deref() {
                Some(base) => base,
                None if selection.name == "openai" => "https://api.openai.com/v1",
                None => {
                    tracing::warn!(
                        provider = selection.name,
                        "Missing api_base for OpenAI-compatible preset; skipping provider (set providers.{}.api_base in config)",
                        selection.name,
                    );
                    return None;
                }
            };
            let provider = OpenAIProvider::with_config(
                &selection.api_key,
                api_base,
                selection.auth_header.clone(),
                selection.api_version.clone(),
            );
            Some(Box::new(provider))
        }
        _ => None,
    }
}

struct RuntimeProviderCandidate {
    name: &'static str,
    provider: Box<dyn LLMProvider>,
    /// Per-provider model override from config.
    model: Option<String>,
}

/// Build the runtime provider chain (base → optional fallback → optional quota).
///
/// Resolves configured runtime providers in registry order and optionally
/// chains them with `FallbackProvider` when `providers.fallback.enabled`.
///
/// Moved from `cli/common.rs:251–315`.
pub fn build_runtime_provider_chain(
    config: &Config,
) -> Option<(Box<dyn LLMProvider>, Vec<&'static str>)> {
    let mut candidates: Vec<RuntimeProviderCandidate> = Vec::new();
    let configured_model = &config.agents.defaults.model;

    // Create a single shared QuotaStore for all providers assembled in this call.
    let quota_store = Arc::new(crate::providers::QuotaStore::load_or_default());

    for selection in resolve_runtime_providers(config) {
        if let Some(provider) = provider_from_runtime_selection(&selection, configured_model) {
            let quota =
                provider_config_by_name(config, selection.name).and_then(|pc| pc.quota.clone());
            let provider =
                apply_quota_wrapper(provider, selection.name, quota, Arc::clone(&quota_store));
            candidates.push(RuntimeProviderCandidate {
                name: selection.name,
                provider,
                model: selection.model.clone(),
            });
        } else {
            warn!(
                provider = selection.name,
                backend = selection.backend,
                "Skipping runtime provider with unsupported backend"
            );
        }
    }

    let mut candidates_iter = candidates.into_iter();
    let first = candidates_iter.next()?;

    // Only chain multiple providers when fallback is explicitly enabled.
    // Without this gate, users who configure multiple API keys for different
    // purposes (e.g. Anthropic for production, OpenAI for testing) would get
    // unexpected automatic failover.
    if !config.providers.fallback.enabled {
        return Some((first.provider, vec![first.name]));
    }

    let mut fallback_candidates: Vec<RuntimeProviderCandidate> = candidates_iter.collect();
    if !fallback_candidates.is_empty() {
        let mut ordered = Vec::with_capacity(1 + fallback_candidates.len());
        ordered.push(first);
        ordered.append(&mut fallback_candidates);
        apply_fallback_preference(&mut ordered, config.providers.fallback.provider.as_deref());

        let mut ordered_iter = ordered.into_iter();
        let primary = ordered_iter.next()?;
        let mut provider_names = vec![primary.name];
        let mut provider_chain = primary.provider;

        for candidate in ordered_iter {
            provider_names.push(candidate.name);
            provider_chain = Box::new(
                FallbackProvider::new(provider_chain, candidate.provider)
                    .with_fallback_model(candidate.model.clone()),
            ) as Box<dyn LLMProvider>;
        }

        return Some((provider_chain, provider_names));
    }

    Some((first.provider, vec![first.name]))
}

/// Wrap `provider` with retry decorator when `providers.retry.enabled`.
///
/// Moved from `cli/common.rs:317–329`.
pub fn apply_retry_wrapper(
    provider: Box<dyn LLMProvider>,
    config: &Config,
) -> Box<dyn LLMProvider> {
    if !config.providers.retry.enabled {
        return provider;
    }

    Box::new(
        RetryProvider::new(provider)
            .with_max_retries(config.providers.retry.max_retries)
            .with_base_delay_ms(config.providers.retry.base_delay_ms)
            .with_max_delay_ms(config.providers.retry.max_delay_ms)
            .with_retry_budget_ms(config.providers.retry.retry_budget_ms),
    )
}

/// Wrap `provider` in a [`crate::providers::QuotaProvider`] when a quota
/// configuration is present, otherwise return `provider` unchanged.
///
/// Moved from `cli/common.rs:333–345`.
fn apply_quota_wrapper(
    provider: Box<dyn LLMProvider>,
    name: &str,
    quota: Option<crate::providers::QuotaConfig>,
    store: Arc<crate::providers::QuotaStore>,
) -> Box<dyn LLMProvider> {
    match quota {
        Some(config) => Box::new(crate::providers::QuotaProvider::new(
            provider, name, config, store,
        )),
        None => provider,
    }
}

fn provider_auth_method(config: &Config, name: &str) -> AuthMethod {
    provider_config_by_name(config, name)
        .map(|p| p.resolved_auth_method())
        .unwrap_or_default()
}

fn apply_fallback_preference(
    candidates: &mut Vec<RuntimeProviderCandidate>,
    preferred: Option<&str>,
) {
    let Some(preferred) = preferred.map(str::trim).filter(|name| !name.is_empty()) else {
        return;
    };

    if candidates.len() < 2 {
        return;
    }

    if candidates[0].name.eq_ignore_ascii_case(preferred) {
        warn!(
            preferred_fallback = preferred,
            primary = candidates[0].name,
            "Preferred fallback provider is already primary; keeping registry order"
        );
        return;
    }

    let preferred_index = candidates
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, candidate)| {
            candidate
                .name
                .eq_ignore_ascii_case(preferred)
                .then_some(index)
        });

    if let Some(index) = preferred_index {
        let preferred_candidate = candidates.remove(index);
        candidates.insert(1, preferred_candidate);
    } else {
        warn!(
            preferred_fallback = preferred,
            "Preferred fallback provider is not configured or runtime-supported; keeping registry order"
        );
    }
}

/// Refresh OAuth credentials for providers configured with OAuth/Auto auth.
///
/// Moved from `cli/common.rs:353–384`.
pub async fn refresh_oauth_credentials_if_needed(config: &Config) {
    let encryption = match crate::security::encryption::resolve_master_key(false) {
        Ok(enc) => enc,
        Err(_) => return,
    };

    let store = auth::store::TokenStore::new(encryption);

    for &provider in auth::oauth_supported_providers() {
        let method = provider_auth_method(config, provider);
        if !matches!(method, AuthMethod::OAuth | AuthMethod::Auto) {
            continue;
        }

        let token = match store.load(provider) {
            Ok(Some(token)) => token,
            Ok(None) => continue,
            Err(err) => {
                warn!(provider = provider, error = %err, "Failed to load OAuth token from store");
                continue;
            }
        };

        if !token.expires_within(auth::refresh::REFRESH_BUFFER_SECS) {
            continue;
        }

        if let Err(err) = auth::refresh::ensure_fresh_token(&store, provider).await {
            warn!(provider = provider, error = %err, "Failed to refresh OAuth token");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    use crate::config::Config;
    use crate::error::ProviderError;
    use crate::providers::{ChatOptions, LLMResponse, ToolDefinition};
    use crate::session::Message;
    use async_trait::async_trait;

    // -----------------------------------------------------------
    // Config default tests (pre-existing)
    // -----------------------------------------------------------

    #[test]
    fn test_provider_chain_none_without_keys() {
        let config = Config::default();
        let selections = crate::providers::resolve_runtime_providers(&config);
        assert!(
            selections.is_empty(),
            "Default config with no API keys should yield no runtime providers"
        );
    }

    #[test]
    fn test_retry_wrapper_disabled_by_default() {
        let config = Config::default();
        assert!(
            !config.providers.retry.enabled,
            "Retry should be disabled by default"
        );
    }

    #[test]
    fn test_fallback_disabled_by_default() {
        let config = Config::default();
        assert!(
            !config.providers.fallback.enabled,
            "Fallback should be disabled by default"
        );
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = Config::default();
        assert_eq!(config.providers.retry.max_retries, 3);
        assert_eq!(config.providers.retry.base_delay_ms, 1000);
        assert_eq!(config.providers.retry.max_delay_ms, 30000);
    }

    // -----------------------------------------------------------
    // Provider chain assembly tests (moved from cli/common.rs)
    // -----------------------------------------------------------

    #[derive(Debug)]
    struct FlakyProvider {
        calls: Arc<AtomicU32>,
        fail_until: u32,
    }

    #[async_trait]
    impl LLMProvider for FlakyProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> crate::error::Result<LLMResponse> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call <= self.fail_until {
                Err(ProviderError::RateLimit("simulated rate limit".to_string()).into())
            } else {
                Ok(LLMResponse::text("ok"))
            }
        }

        fn default_model(&self) -> &str {
            "mock-model"
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[test]
    fn test_build_runtime_provider_chain_empty_when_no_provider() {
        let config = Config::default();
        assert!(build_runtime_provider_chain(&config).is_none());
    }

    #[test]
    fn test_build_runtime_provider_chain_single_provider() {
        let mut config = Config::default();
        config.providers.openai = Some(crate::config::ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });

        let (provider, names) =
            build_runtime_provider_chain(&config).expect("provider chain should resolve");
        assert_eq!(names, vec!["openai"]);
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn test_build_runtime_provider_chain_preserves_registry_order() {
        let mut config = Config::default();
        config.providers.fallback.enabled = true;
        config.providers.anthropic = Some(crate::config::ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });
        config.providers.openai = Some(crate::config::ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });
        config.providers.groq = Some(crate::config::ProviderConfig {
            api_key: Some("gsk-test".to_string()),
            ..Default::default()
        });

        let (provider, names) =
            build_runtime_provider_chain(&config).expect("provider chain should resolve");
        assert_eq!(names, vec!["anthropic", "openai", "groq"]);

        let chain_name = provider.name();
        assert_eq!(chain_name.matches("->").count(), 2);
        assert!(chain_name.contains("openai"));
    }

    #[test]
    fn test_build_runtime_provider_chain_honors_preferred_fallback_provider() {
        let mut config = Config::default();
        config.providers.fallback.enabled = true;
        config.providers.fallback.provider = Some("groq".to_string());
        config.providers.anthropic = Some(crate::config::ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });
        config.providers.openai = Some(crate::config::ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });
        config.providers.groq = Some(crate::config::ProviderConfig {
            api_key: Some("gsk-test".to_string()),
            ..Default::default()
        });

        let (_provider, names) =
            build_runtime_provider_chain(&config).expect("provider chain should resolve");
        assert_eq!(names, vec!["anthropic", "groq", "openai"]);
    }

    #[test]
    fn test_build_runtime_provider_chain_no_chain_when_fallback_disabled() {
        let mut config = Config::default();
        config.providers.fallback.enabled = false;
        config.providers.anthropic = Some(crate::config::ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });
        config.providers.openai = Some(crate::config::ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });

        let (provider, names) =
            build_runtime_provider_chain(&config).expect("provider chain should resolve");
        // Only the highest-priority provider is used
        assert_eq!(names, vec!["anthropic"]);
        assert_eq!(provider.name(), "claude");
    }

    #[tokio::test]
    async fn test_apply_retry_wrapper_retries_when_enabled() {
        let mut config = Config::default();
        config.providers.retry.enabled = true;
        config.providers.retry.max_retries = 3;
        config.providers.retry.base_delay_ms = 0;
        config.providers.retry.max_delay_ms = 0;

        let calls = Arc::new(AtomicU32::new(0));
        let wrapped = apply_retry_wrapper(
            Box::new(FlakyProvider {
                calls: Arc::clone(&calls),
                fail_until: 2,
            }),
            &config,
        );

        let result = wrapped
            .chat(
                vec![Message::user("hello")],
                vec![],
                None,
                ChatOptions::new(),
            )
            .await
            .expect("retry wrapper should eventually succeed");

        assert_eq!(result.content, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_apply_retry_wrapper_is_noop_when_disabled() {
        let mut config = Config::default();
        config.providers.retry.enabled = false;

        let calls = Arc::new(AtomicU32::new(0));
        let wrapped = apply_retry_wrapper(
            Box::new(FlakyProvider {
                calls: Arc::clone(&calls),
                fail_until: 1,
            }),
            &config,
        );

        let err = wrapped
            .chat(
                vec![Message::user("hello")],
                vec![],
                None,
                ChatOptions::new(),
            )
            .await
            .expect_err("retry disabled should not retry");

        assert!(err.to_string().contains("rate limit"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_apply_quota_wrapper_passthrough_when_none() {
        let calls = Arc::new(AtomicU32::new(0));
        let store = Arc::new(crate::providers::QuotaStore::load_or_default());
        let wrapped = apply_quota_wrapper(
            Box::new(FlakyProvider {
                calls: Arc::clone(&calls),
                fail_until: 0, // always succeeds
            }),
            "test",
            None, // no quota config
            store,
        );

        let result = wrapped
            .chat(
                vec![Message::user("hello")],
                vec![],
                None,
                ChatOptions::new(),
            )
            .await
            .expect("provider with None quota should succeed");

        assert_eq!(result.content, "ok");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "exactly one call should be made"
        );
    }
}
