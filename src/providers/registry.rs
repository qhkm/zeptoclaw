//! Provider registry and resolution helpers.
//!
//! This module centralizes provider metadata and the mapping from configuration
//! to runtime provider selection.

use crate::auth::{AuthMethod, ResolvedCredential};
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
    /// Default custom auth header name (e.g. "api-key" for Azure). None = "Authorization: Bearer"
    pub default_auth_header: Option<&'static str>,
    /// Default API version query param. None = no query param.
    pub default_api_version: Option<&'static str>,
    /// Whether this provider requires an API key to resolve.
    /// Set to `false` for local/keyless providers (e.g. Ollama, vLLM) that run without auth.
    pub api_key_required: bool,
}

/// Runtime-ready provider selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProviderSelection {
    /// Selected provider id.
    pub name: &'static str,
    /// API key used for provider auth (kept for backward compat).
    pub api_key: String,
    /// Optional provider base URL.
    pub api_base: Option<String>,
    /// The underlying backend type ("anthropic" or "openai").
    pub backend: &'static str,
    /// Resolved credential (OAuth token or API key).
    pub credential: ResolvedCredential,
    /// Per-provider model override from config.
    pub model: Option<String>,
    /// Effective auth header for this provider (user override OR spec default).
    pub auth_header: Option<String>,
    /// Effective API version param for this provider.
    pub api_version: Option<String>,
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
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "openai",
        model_keywords: &["openai", "gpt"],
        runtime_supported: true,
        default_base_url: None,
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "openrouter",
        model_keywords: &["openrouter"],
        runtime_supported: true,
        default_base_url: Some("https://openrouter.ai/api/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "groq",
        model_keywords: &["groq"],
        runtime_supported: true,
        default_base_url: Some("https://api.groq.com/openai/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "zhipu",
        model_keywords: &["zhipu", "glm", "zai"],
        runtime_supported: true,
        default_base_url: Some("https://open.bigmodel.cn/api/paas/v4"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "vllm",
        model_keywords: &["vllm"],
        runtime_supported: true,
        default_base_url: Some("http://localhost:8000/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: false,
    },
    ProviderSpec {
        name: "gemini",
        model_keywords: &["gemini"],
        runtime_supported: true,
        default_base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "ollama",
        model_keywords: &["ollama", "llama", "mistral", "phi", "qwen"],
        runtime_supported: true,
        default_base_url: Some("http://localhost:11434/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: false,
    },
    ProviderSpec {
        name: "nvidia",
        model_keywords: &["nvidia", "nim"],
        runtime_supported: true,
        default_base_url: Some("https://integrate.api.nvidia.com/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "deepseek",
        model_keywords: &["deepseek"],
        runtime_supported: true,
        default_base_url: Some("https://api.deepseek.com/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "kimi",
        model_keywords: &["kimi", "moonshot"],
        runtime_supported: true,
        default_base_url: Some("https://api.moonshot.cn/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "azure",
        model_keywords: &["azure"],
        runtime_supported: true,
        default_base_url: None, // user MUST set api_base to their deployment URL
        backend: "openai",
        default_auth_header: Some("api-key"),
        default_api_version: Some("2024-08-01-preview"),
        api_key_required: true,
    },
    ProviderSpec {
        name: "bedrock",
        model_keywords: &["bedrock", "anthropic.claude", "meta.llama", "amazon.titan"],
        runtime_supported: true,
        default_base_url: None, // User must configure api_base pointing to a SigV4 proxy or Bedrock API key endpoint
        backend: "openai",
        default_auth_header: None, // AWS SigV4 required; not yet implemented natively
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "xai",
        model_keywords: &["xai", "grok"],
        runtime_supported: true,
        default_base_url: Some("https://api.x.ai/v1"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "qianfan",
        model_keywords: &["qianfan", "ernie", "baidu"],
        runtime_supported: true,
        default_base_url: Some("https://qianfan.baidubce.com/v2"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
    ProviderSpec {
        name: "novita",
        model_keywords: &["novita"],
        runtime_supported: true,
        default_base_url: Some("https://api.novita.ai/openai"),
        backend: "openai",
        default_auth_header: None,
        default_api_version: None,
        api_key_required: true,
    },
];

pub fn provider_config_by_name<'a>(config: &'a Config, name: &str) -> Option<&'a ProviderConfig> {
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
        "deepseek" => config.providers.deepseek.as_ref(),
        "kimi" => config.providers.kimi.as_ref(),
        "azure" => config.providers.azure.as_ref(),
        "bedrock" => config.providers.bedrock.as_ref(),
        "xai" => config.providers.xai.as_ref(),
        "qianfan" => config.providers.qianfan.as_ref(),
        "novita" => config.providers.novita.as_ref(),
        _ => None,
    }
}

fn configured_api_key(provider: Option<&ProviderConfig>) -> Option<&str> {
    provider
        .and_then(|p| p.api_key.as_deref())
        .and_then(|k| if k.is_empty() { None } else { Some(k) })
}

/// Returns all configured provider ids in registry order.
///
/// Key-required providers are included only when an API key is present.
/// Keyless providers (e.g. Ollama, vLLM) are included whenever a config section is present,
/// even without an API key.
pub fn configured_provider_names(config: &Config) -> Vec<&'static str> {
    PROVIDER_REGISTRY
        .iter()
        .filter_map(|spec| {
            let provider = provider_config_by_name(config, spec.name)?;
            if !spec.api_key_required || configured_api_key(Some(provider)).is_some() {
                Some(spec.name)
            } else {
                None
            }
        })
        .collect()
}

/// Returns `(provider_name, model)` pairs for each configured provider that has an
/// explicit `model` field set. Useful for showing user-configured models in `/model list`.
///
/// Key-required providers must have an API key to appear. Keyless providers (e.g. Ollama, vLLM)
/// are eligible whenever a config section with a model is present.
pub fn configured_provider_models(config: &Config) -> Vec<(String, String)> {
    PROVIDER_REGISTRY
        .iter()
        .filter_map(|spec| {
            let prov = provider_config_by_name(config, spec.name)?;
            // For key-required providers, must have an API key. Keyless providers are always eligible.
            if spec.api_key_required && configured_api_key(Some(prov)).is_none() {
                return None;
            }
            let model = prov.model.as_ref()?.clone();
            if model.is_empty() {
                return None;
            }
            Some((spec.name.to_string(), model))
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
///
/// For each provider, resolves the credential based on the configured `auth_method`:
/// - `api_key` (default): uses the configured API key
/// - `oauth`: checks the token store for a valid OAuth token
/// - `auto`: tries OAuth first, falls back to API key
pub fn resolve_runtime_providers(config: &Config) -> Vec<RuntimeProviderSelection> {
    let mut resolved = Vec::new();

    // Try to load the token store for OAuth resolution.
    let token_store = crate::security::encryption::resolve_master_key(false)
        .ok()
        .map(crate::auth::store::TokenStore::new);

    for spec in PROVIDER_REGISTRY
        .iter()
        .filter(|spec| spec.runtime_supported)
    {
        let provider = provider_config_by_name(config, spec.name);
        let auth_method = provider
            .map(|p| p.resolved_auth_method())
            .unwrap_or_default();

        // Resolve credential based on auth method
        let (credential, api_key_str) =
            match resolve_credential(spec, &auth_method, provider, token_store.as_ref()) {
                Some(pair) => pair,
                None => continue, // No credential available for this provider
            };

        let user_base = provider.and_then(|p| p.api_base.clone()).and_then(|base| {
            if base.is_empty() {
                None
            } else {
                Some(base)
            }
        });
        let api_base = user_base.or_else(|| spec.default_base_url.map(String::from));

        let effective_auth_header = provider
            .and_then(|p| p.auth_header.as_deref())
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(|h| h.to_string())
            .or_else(|| spec.default_auth_header.map(String::from));

        let effective_api_version = provider
            .and_then(|p| p.api_version.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
            .or_else(|| spec.default_api_version.map(String::from));

        resolved.push(RuntimeProviderSelection {
            name: spec.name,
            api_key: api_key_str,
            api_base,
            backend: spec.backend,
            credential,
            model: provider.and_then(|p| p.model.clone()),
            auth_header: effective_auth_header,
            api_version: effective_api_version,
        });
    }

    resolved
}

/// Resolve a credential for a single provider.
///
/// Returns `Some((credential, api_key_string))` or `None` if no credential is available.
fn resolve_credential(
    spec: &ProviderSpec,
    auth_method: &AuthMethod,
    provider_config: Option<&ProviderConfig>,
    token_store: Option<&crate::auth::store::TokenStore>,
) -> Option<(ResolvedCredential, String)> {
    let provider_name = spec.name;
    let api_key = configured_api_key(provider_config);

    let result = match auth_method {
        AuthMethod::ApiKey => {
            // Only use API key
            api_key.map(|key| (ResolvedCredential::ApiKey(key.to_string()), key.to_string()))
        }
        AuthMethod::OAuth => {
            // Only use OAuth token
            try_load_oauth_token(provider_name, token_store)
                .map(|token| (token, api_key.unwrap_or("").to_string()))
        }
        AuthMethod::Auto => {
            // Try OAuth first, fall back to API key
            if let Some(token) = try_load_oauth_token(provider_name, token_store) {
                Some((token, api_key.unwrap_or("").to_string()))
            } else {
                api_key.map(|key| (ResolvedCredential::ApiKey(key.to_string()), key.to_string()))
            }
        }
    };

    // Keyless fallback: providers that don't require an API key (e.g. Ollama, vLLM)
    // resolve with an empty credential when no key/OAuth token is configured,
    // as long as the provider section is present in config.
    if result.is_none() && !spec.api_key_required && provider_config.is_some() {
        return Some((ResolvedCredential::ApiKey(String::new()), String::new()));
    }

    // Last-resort fallback: import from Claude CLI (Keychain / ~/.claude.json).
    // Disabled in test builds to avoid picking up real credentials from the host.
    #[cfg(not(test))]
    if result.is_none() && spec.name == "anthropic" {
        if let Some(token_set) = crate::auth::claude_import::read_claude_credentials() {
            static WARN_ONCE: std::sync::Once = std::sync::Once::new();
            WARN_ONCE.call_once(|| {
                // Dim the warning so it doesn't dominate CLI output
                eprintln!(
                    "\x1b[2mUsing Claude subscription token (unofficial). \
                     Set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY for official API access.\x1b[0m"
                );
            });

            let credential = ResolvedCredential::BearerToken {
                access_token: token_set.access_token,
                expires_at: token_set.expires_at,
            };
            return Some((credential, String::new()));
        }
    }

    result
}

/// Try to load a valid OAuth token from the store.
fn try_load_oauth_token(
    provider_name: &str,
    token_store: Option<&crate::auth::store::TokenStore>,
) -> Option<ResolvedCredential> {
    let store = token_store?;
    let token_set = store.load(provider_name).ok()??;

    // Expired tokens are ignored here; callers may refresh separately (the CLI does this on startup).
    if token_set.is_expired() {
        return None;
    }

    Some(ResolvedCredential::BearerToken {
        access_token: token_set.access_token,
        expires_at: token_set.expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
    fn test_zhipu_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.zhipu = Some(ProviderConfig {
            api_key: Some("zhipu-test-key".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "zhipu");
        assert_eq!(selected.backend, "openai");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://open.bigmodel.cn/api/paas/v4")
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

    fn test_token_store_with_token(
        token: crate::auth::OAuthTokenSet,
    ) -> (TempDir, crate::auth::store::TokenStore) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json.enc");
        let store = crate::auth::store::TokenStore::with_path(
            path,
            crate::security::encryption::SecretEncryption::from_raw_key(&[0x42u8; 32]),
        );
        store.save(&token).unwrap();
        (tmp, store)
    }

    #[test]
    fn test_try_load_oauth_token_skips_expired_token_even_with_refresh_token() {
        let token = crate::auth::OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token: "expired-access".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(chrono::Utc::now().timestamp() - 60),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp() - 3600,
            client_id: Some("zeptoclaw".to_string()),
        };

        let (_tmp, store) = test_token_store_with_token(token);
        let resolved = try_load_oauth_token("anthropic", Some(&store));
        assert!(resolved.is_none());
    }

    #[test]
    fn test_resolve_credential_auto_falls_back_to_api_key_when_oauth_token_expired() {
        let token = crate::auth::OAuthTokenSet {
            provider: "anthropic".to_string(),
            access_token: "expired-access".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(chrono::Utc::now().timestamp() - 60),
            token_type: "Bearer".to_string(),
            scope: None,
            obtained_at: chrono::Utc::now().timestamp() - 3600,
            client_id: Some("zeptoclaw".to_string()),
        };

        let (_tmp, store) = test_token_store_with_token(token);
        let provider = ProviderConfig {
            api_key: Some("sk-ant-fallback".to_string()),
            auth_method: Some("auto".to_string()),
            ..Default::default()
        };

        let spec = PROVIDER_REGISTRY
            .iter()
            .find(|s| s.name == "anthropic")
            .expect("anthropic spec must exist");

        let (credential, api_key) =
            resolve_credential(spec, &AuthMethod::Auto, Some(&provider), Some(&store))
                .expect("credential should resolve");

        assert_eq!(api_key, "sk-ant-fallback");
        assert!(matches!(credential, ResolvedCredential::ApiKey(_)));
    }

    #[test]
    fn test_runtime_selection_carries_provider_model() {
        let mut config = Config::default();
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-test".to_string()),
            model: Some("claude-opus-4-20250514".to_string()),
            ..Default::default()
        });

        let resolved = resolve_runtime_providers(&config);
        let anthropic = resolved.iter().find(|s| s.name == "anthropic").unwrap();
        assert_eq!(anthropic.model, Some("claude-opus-4-20250514".to_string()));
    }

    #[test]
    fn test_runtime_selection_model_none_when_not_configured() {
        let mut config = Config::default();
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        });

        let resolved = resolve_runtime_providers(&config);
        let anthropic = resolved.iter().find(|s| s.name == "anthropic").unwrap();
        assert_eq!(anthropic.model, None);
    }

    #[test]
    fn test_azure_provider_resolves_with_auth_header_and_api_version() {
        let mut config = Config::default();
        config.providers.azure = Some(ProviderConfig {
            api_key: Some("my-azure-key".to_string()),
            api_base: Some("https://myco.openai.azure.com/openai/deployments/gpt-4o".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("should resolve");
        assert_eq!(selected.name, "azure");
        assert_eq!(selected.backend, "openai");
        assert_eq!(selected.auth_header.as_deref(), Some("api-key"));
        assert_eq!(selected.api_version.as_deref(), Some("2024-08-01-preview"));
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://myco.openai.azure.com/openai/deployments/gpt-4o")
        );
    }

    #[test]
    fn test_azure_user_can_override_api_version() {
        let mut config = Config::default();
        config.providers.azure = Some(ProviderConfig {
            api_key: Some("my-azure-key".to_string()),
            api_version: Some("2025-01-01-preview".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("should resolve");
        assert_eq!(selected.name, "azure");
        // User override takes precedence over spec default
        assert_eq!(selected.api_version.as_deref(), Some("2025-01-01-preview"));
    }

    #[test]
    fn test_bedrock_provider_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.bedrock = Some(ProviderConfig {
            api_key: Some("aws-sig-placeholder".to_string()),
            api_base: Some("https://my-sigv4-proxy.example.com/v1".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("should resolve");
        assert_eq!(selected.name, "bedrock");
        assert_eq!(selected.backend, "openai");
        assert!(selected.auth_header.is_none());
        assert!(selected.api_version.is_none());
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://my-sigv4-proxy.example.com/v1")
        );
    }

    #[test]
    fn test_standard_provider_auth_header_is_none() {
        let mut config = Config::default();
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("should resolve");
        assert_eq!(selected.name, "openai");
        assert!(selected.auth_header.is_none());
        assert!(selected.api_version.is_none());
    }

    #[test]
    fn test_runtime_supported_constant_includes_azure_and_bedrock() {
        assert!(crate::providers::RUNTIME_SUPPORTED_PROVIDERS.contains(&"azure"));
        assert!(crate::providers::RUNTIME_SUPPORTED_PROVIDERS.contains(&"bedrock"));
    }

    #[test]
    fn test_configured_provider_models_returns_providers_with_model_set() {
        let mut config = Config::default();
        config.providers.nvidia = Some(ProviderConfig {
            api_key: Some("nvapi-test".to_string()),
            model: Some("nvidia/llama-3.3-70b".to_string()),
            ..Default::default()
        });
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("sk-ant".to_string()),
            // No model set — should not appear
            ..Default::default()
        });

        let models = configured_provider_models(&config);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].0, "nvidia");
        assert_eq!(models[0].1, "nvidia/llama-3.3-70b");
    }

    #[test]
    fn test_configured_provider_models_skips_empty_model() {
        let mut config = Config::default();
        config.providers.openai = Some(ProviderConfig {
            api_key: Some("sk-openai".to_string()),
            model: Some("".to_string()),
            ..Default::default()
        });

        let models = configured_provider_models(&config);
        assert!(models.is_empty());
    }

    #[test]
    fn test_configured_provider_models_skips_no_api_key() {
        let mut config = Config::default();
        config.providers.nvidia = Some(ProviderConfig {
            model: Some("nvidia/llama-3.3-70b".to_string()),
            // No API key
            ..Default::default()
        });

        let models = configured_provider_models(&config);
        assert!(models.is_empty());
    }

    #[test]
    fn test_empty_auth_header_falls_through_to_spec_default() {
        let mut config = Config::default();
        config.providers.azure = Some(ProviderConfig {
            api_key: Some("my-azure-key".to_string()),
            api_base: Some("https://myco.openai.azure.com/openai/deployments/gpt-4o".to_string()),
            auth_header: Some("".to_string()), // empty — should fall through to spec default "api-key"
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("should resolve");
        assert_eq!(selected.name, "azure");
        // Empty string should fall through to spec default
        assert_eq!(selected.auth_header.as_deref(), Some("api-key"));
    }

    #[test]
    fn test_xai_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.xai = Some(ProviderConfig {
            api_key: Some("xai-test-key".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "xai");
        assert_eq!(selected.backend, "openai");
        assert_eq!(selected.api_base.as_deref(), Some("https://api.x.ai/v1"));
    }

    #[test]
    fn test_qianfan_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.qianfan = Some(ProviderConfig {
            api_key: Some("qf-test-key".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "qianfan");
        assert_eq!(selected.backend, "openai");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://qianfan.baidubce.com/v2")
        );
    }

    #[test]
    fn test_novita_resolves_with_default_base_url() {
        let mut config = Config::default();
        config.providers.novita = Some(ProviderConfig {
            api_key: Some("novita-test-key".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("provider should resolve");
        assert_eq!(selected.name, "novita");
        assert_eq!(selected.backend, "openai");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://api.novita.ai/openai")
        );
    }

    #[test]
    fn test_xai_and_qianfan_in_fallback_chain() {
        let mut config = Config::default();
        config.providers.xai = Some(ProviderConfig {
            api_key: Some("xai-key".to_string()),
            ..Default::default()
        });
        config.providers.qianfan = Some(ProviderConfig {
            api_key: Some("qf-key".to_string()),
            ..Default::default()
        });

        let resolved = resolve_runtime_providers(&config);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "xai");
        assert_eq!(resolved[1].name, "qianfan");
    }

    #[test]
    fn test_ollama_resolves_without_api_key() {
        let mut config = Config::default();
        config.providers.ollama = Some(ProviderConfig {
            api_base: Some("http://localhost:11434/v1".to_string()),
            ..Default::default()
        });

        let selected =
            resolve_runtime_provider(&config).expect("ollama should resolve without api key");
        assert_eq!(selected.name, "ollama");
        assert_eq!(selected.api_key, "");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn test_vllm_resolves_without_api_key() {
        let mut config = Config::default();
        config.providers.vllm = Some(ProviderConfig {
            api_base: Some("http://gpu-server:8000/v1".to_string()),
            ..Default::default()
        });

        let selected =
            resolve_runtime_provider(&config).expect("vllm should resolve without api key");
        assert_eq!(selected.name, "vllm");
        assert_eq!(selected.api_key, "");
    }

    #[test]
    fn test_ollama_bare_config_resolves_with_defaults() {
        let mut config = Config::default();
        config.providers.ollama = Some(ProviderConfig::default());

        let selected =
            resolve_runtime_provider(&config).expect("bare ollama config should resolve");
        assert_eq!(selected.name, "ollama");
        assert_eq!(selected.api_key, "");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn test_ollama_with_api_key_still_resolves_with_key() {
        let mut config = Config::default();
        config.providers.ollama = Some(ProviderConfig {
            api_key: Some("secret-cloud-key".to_string()),
            api_base: Some("https://cloud-ollama.example.com/v1".to_string()),
            ..Default::default()
        });

        let selected = resolve_runtime_provider(&config).expect("ollama with key should resolve");
        assert_eq!(selected.name, "ollama");
        assert_eq!(selected.api_key, "secret-cloud-key");
        assert_eq!(
            selected.api_base.as_deref(),
            Some("https://cloud-ollama.example.com/v1")
        );
    }

    #[test]
    fn test_configured_provider_names_includes_keyless_ollama() {
        let mut config = Config::default();
        config.providers.ollama = Some(ProviderConfig::default());

        let names = configured_provider_names(&config);
        assert!(
            names.contains(&"ollama"),
            "keyless ollama should appear in configured providers"
        );
    }

    #[test]
    fn test_configured_provider_models_includes_keyless_provider_with_model() {
        let mut config = Config::default();
        config.providers.ollama = Some(ProviderConfig {
            model: Some("llama3.3".to_string()),
            ..Default::default()
        });

        let models = configured_provider_models(&config);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].0, "ollama");
        assert_eq!(models[0].1, "llama3.3");
    }
}
