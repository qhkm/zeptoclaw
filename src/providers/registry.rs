//! Provider registry and resolution helpers.
//!
//! This module centralizes provider metadata and the mapping from configuration
//! to runtime provider selection.

use crate::config::{Config, ProviderConfig};

/// Metadata describing an LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpec {
    /// Config key / provider id (e.g. "openai").
    pub name: &'static str,
    /// Model keywords commonly associated with this provider.
    pub model_keywords: &'static [&'static str],
    /// Whether this provider is currently wired for runtime execution.
    pub runtime_supported: bool,
    /// Default API base URL (None = native OpenAI endpoint).
    pub default_base_url: Option<&'static str>,
    /// The underlying backend ("anthropic" or "openai") for routing.
    pub backend: &'static str,
}

/// Runtime-ready provider selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProviderSelection {
    /// Selected provider id.
    pub name: &'static str,
    /// API key used for provider auth.
    pub api_key: String,
    /// Optional provider base URL.
    pub api_base: Option<String>,
    /// The underlying backend type ("anthropic" or "openai").
    pub backend: &'static str,
}

/// Provider registry in priority order.
///
/// Runtime selection follows this order for runtime-supported providers.
pub const PROVIDER_REGISTRY: &[ProviderSpec] = &[
    ProviderSpec {
        name: "anthropic",
        model_keywords: &["anthropic", "claude"],
        runtime_supported: true,
        default_base_url: None,
        backend: "anthropic",
    },
    ProviderSpec {
        name: "openai",
        model_keywords: &["openai", "gpt"],
        runtime_supported: true,
        default_base_url: None,
        backend: "openai",
    },
    ProviderSpec {
        name: "openrouter",
        model_keywords: &["openrouter"],
        runtime_supported: true,
        default_base_url: Some("https://openrouter.ai/api/v1"),
        backend: "openai",
    },
    ProviderSpec {
        name: "groq",
        model_keywords: &["groq"],
        runtime_supported: true,
        default_base_url: Some("https://api.groq.com/openai/v1"),
        backend: "openai",
    },
    ProviderSpec {
        name: "zhipu",
        model_keywords: &["zhipu", "glm", "zai"],
        runtime_supported: true,
        default_base_url: Some("https://open.bigmodel.cn/api/paas/v4"),
        backend: "openai",
    },
    ProviderSpec {
        name: "vllm",
        model_keywords: &["vllm"],
        runtime_supported: true,
        default_base_url: Some("http://localhost:8000/v1"),
        backend: "openai",
    },
    ProviderSpec {
        name: "gemini",
        model_keywords: &["gemini"],
        runtime_supported: true,
        default_base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        backend: "openai",
    },
    ProviderSpec {
        name: "ollama",
        model_keywords: &["ollama", "llama", "mistral", "phi", "qwen"],
        runtime_supported: true,
        default_base_url: Some("http://localhost:11434/v1"),
        backend: "openai",
    },
    ProviderSpec {
        name: "nvidia",
        model_keywords: &["nvidia", "nim"],
        runtime_supported: true,
        default_base_url: Some("https://integrate.api.nvidia.com/v1"),
        backend: "openai",
    },
];

fn provider_config_by_name<'a>(config: &'a Config, name: &str) -> Option<&'a ProviderConfig> {
    match name {
        "anthropic" => config.providers.anthropic.as_ref(),
        "openai" => config.providers.openai.as_ref(),
        "openrouter" => config.providers.openrouter.as_ref(),
        "groq" => config.providers.groq.as_ref(),
        "zhipu" => config.providers.zhipu.as_ref(),
        "vllm" => config.providers.vllm.as_ref(),
        "gemini" => config.providers.gemini.as_ref(),
        "ollama" => config.providers.ollama.as_ref(),
        "nvidia" => config.providers.nvidia.as_ref(),
        _ => None,
    }
}

fn configured_api_key(provider: Option<&ProviderConfig>) -> Option<&str> {
    provider
        .and_then(|p| p.api_key.as_deref())
        .and_then(|k| if k.is_empty() { None } else { Some(k) })
}

/// Returns all configured provider ids in registry order.
pub fn configured_provider_names(config: &Config) -> Vec<&'static str> {
    PROVIDER_REGISTRY
        .iter()
        .filter_map(|spec| {
            configured_api_key(provider_config_by_name(config, spec.name)).map(|_| spec.name)
        })
        .collect()
}

/// Returns configured provider ids that are not yet runtime-supported.
pub fn configured_unsupported_provider_names(config: &Config) -> Vec<&'static str> {
    PROVIDER_REGISTRY
        .iter()
        .filter_map(|spec| {
            if spec.runtime_supported {
                None
            } else {
                configured_api_key(provider_config_by_name(config, spec.name)).map(|_| spec.name)
            }
        })
        .collect()
}

/// Resolve the provider currently used by runtime execution.
///
/// Priority follows `PROVIDER_REGISTRY` order for `runtime_supported` providers.
pub fn resolve_runtime_provider(config: &Config) -> Option<RuntimeProviderSelection> {
    resolve_runtime_providers(config).into_iter().next()
}

/// Resolve all runtime-supported configured providers in registry order.
pub fn resolve_runtime_providers(config: &Config) -> Vec<RuntimeProviderSelection> {
    let mut resolved = Vec::new();

    for spec in PROVIDER_REGISTRY
        .iter()
        .filter(|spec| spec.runtime_supported)
    {
        let provider = provider_config_by_name(config, spec.name);
        let Some(api_key) = configured_api_key(provider) else {
            continue;
        };

        let user_base = provider.and_then(|p| p.api_base.clone()).and_then(|base| {
            if base.is_empty() {
                None
            } else {
                Some(base)
            }
        });
        let api_base = user_base.or_else(|| spec.default_base_url.map(String::from));

        resolved.push(RuntimeProviderSelection {
            name: spec.name,
            api_key: api_key.to_string(),
            api_base,
            backend: spec.backend,
        });
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_configured_provider_names_registry_order() {
        let mut config = Config::default();
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });

        let names = configured_provider_names(&config);
        assert_eq!(names, vec!["anthropic", "openai"]);
    }

    #[test]
    fn test_configured_unsupported_provider_names_empty_when_all_supported() {
        let mut config = Config::default();
        config.providers.openrouter = Some(ProviderConfig {
            api_key: Some("sk-or".to_string()),
            ..Default::default()
        });
        config.providers.groq = Some(ProviderConfig {
            api_key: Some("sk-groq".to_string()),
            ..Default::default()
        });

        // All providers are now runtime-supported via OpenAI-compatible backend.
        let names = configured_unsupported_provider_names(&config);
        assert!(names.is_empty());
    }

    #[test]
    fn test_resolve_runtime_provider_priority() {
        let mut config = Config::default();
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            api_base: Some("https://example.com/v1".to_string()),
            ..Default::default()
        });
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "anthropic");
        assert_eq!(selected.api_key, "sk-ant");
        assert_eq!(selected.api_base, None);
    }

    #[test]
    fn test_resolve_runtime_provider_openai_base_url() {
        let mut config = Config::default();
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            api_base: Some("https://example.com/v1".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "openai");
        assert_eq!(selected.api_key, "sk-openai");
        assert_eq!(selected.api_base.as_deref(), Some("https://example.com/v1"));
    }

    #[test]
    fn test_resolve_runtime_providers_returns_all_supported() {
        let mut config = Config::default();
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });

        let resolved = resolve_runtime_providers(&config);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "anthropic");
        assert_eq!(resolved[1].name, "openai");
    }

    #[test]
    fn test_runtime_supported_constant_stays_in_sync() {
        let runtime_supported: Vec<&str> = PROVIDER_REGISTRY
            .iter()
            .filter(|spec| spec.runtime_supported)
            .map(|spec| spec.name)
            .collect();

        assert_eq!(
            runtime_supported,
            crate::providers::RUNTIME_SUPPORTED_PROVIDERS
        );
    }

    #[test]
    fn test_groq_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.groq = Some(ProviderConfig {
            api_key: Some("gsk-test".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "groq");
        assert_eq!(selected.backend, "openai");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://api.groq.com/openai/v1")
        );
    }

    #[test]
    fn test_ollama_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.ollama = Some(ProviderConfig {
            api_key: Some("ollama".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "ollama");
        assert_eq!(selected.backend, "openai");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn test_gemini_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.gemini = Some(ProviderConfig {
            api_key: Some("AIza-test".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "gemini");
        assert_eq!(selected.backend, "openai");
        assert!(selected
            .api_base
            .as_deref()
            .unwrap()
            .contains("generativelanguage"));
    }

    #[test]
    fn test_user_base_url_overrides_default() {
        let mut config = Config::default();
        config.providers.groq = Some(ProviderConfig {
            api_key: Some("gsk-test".to_string()),
            api_base: Some("https://custom.groq.example/v1".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "groq");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://custom.groq.example/v1")
        );
    }

    #[test]
    fn test_anthropic_has_no_default_base_url() {
        let mut config = Config::default();
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "anthropic");
        assert_eq!(selected.backend, "anthropic");
        assert_eq!(selected.api_base, None);
    }

    #[test]
    fn test_nvidia_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.nvidia = Some(ProviderConfig {
            api_key: Some("nvapi-test".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "nvidia");
        assert_eq!(selected.backend, "openai");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
    }

    #[test]
    fn test_openai_has_no_default_base_url() {
        let mut config = Config::default();
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "openai");
        assert_eq!(selected.backend, "openai");
        assert_eq!(selected.api_base, None);
    }
}
