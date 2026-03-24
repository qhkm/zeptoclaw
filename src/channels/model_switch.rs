//! Model switching command parser and known models registry.
//!
//! Provides `/model` command parsing for runtime LLM switching in channels.
//!
//! # Architecture Note
//!
//! Currently implemented as Telegram-first (Approach A: metadata-based).
//! TODO(#63): When adding /model to more channels, migrate to Approach B
//! (CommandInterceptor in agent loop). See docs/plans/2026-02-18-llm-switching-design.md

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use crate::memory::longterm::LongTermMemory;

/// A known LLM model for display in `/model list`.
#[derive(Debug, Clone)]
pub struct KnownModel {
    pub provider: &'static str,
    pub model: &'static str,
    pub label: &'static str,
}

/// Known models registry — popular models per provider for `/model list`.
pub const KNOWN_MODELS: &[KnownModel] = &[
    // Anthropic
    KnownModel {
        provider: "anthropic",
        model: "claude-opus-4-6",
        label: "Claude Opus 4.6",
    },
    KnownModel {
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        label: "Claude Sonnet 4.6",
    },
    KnownModel {
        provider: "anthropic",
        model: "claude-haiku-4-5-20251001",
        label: "Claude Haiku 4.5",
    },
    // OpenAI
    KnownModel {
        provider: "openai",
        model: "gpt-5.4",
        label: "GPT-5.4",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-5.4-mini",
        label: "GPT-5.4 Mini",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-5.4-nano",
        label: "GPT-5.4 Nano",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-5.3-codex",
        label: "GPT-5.3 Codex",
    },
    // OpenRouter
    KnownModel {
        provider: "openrouter",
        model: "anthropic/claude-sonnet-4-6",
        label: "Claude Sonnet 4.6 (OR)",
    },
    KnownModel {
        provider: "openrouter",
        model: "google/gemini-3.1-pro",
        label: "Gemini 3.1 Pro (OR)",
    },
    // Groq
    KnownModel {
        provider: "groq",
        model: "llama-4-scout-17b-16e-instruct",
        label: "Llama 4 Scout",
    },
    KnownModel {
        provider: "groq",
        model: "llama-4-maverick-17b-128e-instruct",
        label: "Llama 4 Maverick",
    },
    // Gemini
    KnownModel {
        provider: "gemini",
        model: "gemini-3.1-pro",
        label: "Gemini 3.1 Pro",
    },
    KnownModel {
        provider: "gemini",
        model: "gemini-3.1-flash-lite",
        label: "Gemini 3.1 Flash Lite",
    },
    // Ollama (local)
    KnownModel {
        provider: "ollama",
        model: "llama3.3",
        label: "Llama 3.3 (local)",
    },
    KnownModel {
        provider: "ollama",
        model: "mistral",
        label: "Mistral (local)",
    },
    // DeepSeek
    KnownModel {
        provider: "deepseek",
        model: "deepseek-chat",
        label: "DeepSeek V3",
    },
    KnownModel {
        provider: "deepseek",
        model: "deepseek-reasoner",
        label: "DeepSeek R1",
    },
    // Kimi (Moonshot AI)
    KnownModel {
        provider: "kimi",
        model: "moonshot-v1-128k",
        label: "Kimi 128K",
    },
    KnownModel {
        provider: "kimi",
        model: "moonshot-v1-32k",
        label: "Kimi 32K",
    },
];

/// Per-chat model override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOverride {
    /// Provider name (None = keep current provider).
    pub provider: Option<String>,
    /// Model identifier.
    pub model: String,
}

/// Parsed `/model` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCommand {
    /// `/model` — show current model
    Show,
    /// `/model <provider:model>` — set model
    Set(ModelOverride),
    /// `/model reset` — clear override
    Reset,
    /// `/model list` — show available models
    List,
    /// `/model fetch` — fetch live models from provider APIs
    Fetch,
}

/// Thread-safe store for per-chat model overrides.
pub type ModelOverrideStore = Arc<RwLock<HashMap<String, ModelOverride>>>;

const MODEL_PREF_CATEGORY: &str = "model_pref";
const MODEL_PREF_PREFIX: &str = "model_pref:";

/// Create a new empty override store.
pub fn new_override_store() -> ModelOverrideStore {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Parse a message as a `/model` command. Returns None if not a `/model` command.
///
/// Only matches exactly `/model` or `/model <arg>`. Does NOT match `/models`,
/// `/model_test`, or other commands that happen to start with `/model`.
pub fn parse_model_command(text: &str) -> Option<ModelCommand> {
    let trimmed = text.trim();

    // Must be exactly "/model" or "/model " followed by args
    let rest = if trimmed == "/model" {
        ""
    } else if let Some(after) = trimmed.strip_prefix("/model ") {
        after.trim()
    } else {
        return None;
    };

    if rest.is_empty() {
        return Some(ModelCommand::Show);
    }

    match rest {
        "reset" => Some(ModelCommand::Reset),
        "list" => Some(ModelCommand::List),
        "fetch" => Some(ModelCommand::Fetch),
        arg => {
            // Only treat "x:y" as provider:model when x is a known provider name.
            // This avoids mis-parsing Ollama tags like "llama3.3:latest".
            if let Some((left, right)) = arg.split_once(':') {
                let is_provider = crate::providers::PROVIDER_REGISTRY
                    .iter()
                    .any(|s| s.name == left);
                if is_provider {
                    Some(ModelCommand::Set(ModelOverride {
                        provider: Some(left.to_string()),
                        model: right.to_string(),
                    }))
                } else {
                    Some(ModelCommand::Set(ModelOverride {
                        provider: None,
                        model: arg.to_string(),
                    }))
                }
            } else {
                Some(ModelCommand::Set(ModelOverride {
                    provider: None,
                    model: arg.to_string(),
                }))
            }
        }
    }
}

/// Format the `/model list` output showing known models with configured status.
///
/// `configured_models` contains `(provider, model)` pairs from user config. Any
/// configured model that isn't already in `KNOWN_MODELS` for its provider is appended
/// with a `(configured)` tag so users can see and select it.
pub fn format_model_list(
    configured_providers: &[String],
    current: Option<&ModelOverride>,
    configured_models: &[(String, String)],
) -> String {
    let mut output = String::from("Available models:\n\n");

    let mut by_provider: Vec<(&str, Vec<&KnownModel>)> = Vec::new();
    for km in KNOWN_MODELS {
        if let Some((_, models)) = by_provider.iter_mut().find(|(p, _)| *p == km.provider) {
            models.push(km);
        } else {
            by_provider.push((km.provider, vec![km]));
        }
    }

    for (provider, models) in &by_provider {
        let is_configured = configured_providers.iter().any(|p| p == provider);
        let status_label = if is_configured { "[ok]" } else { "[warn]" };

        output.push_str(&format!("{} {}:\n", status_label, provider));
        for km in models {
            let is_current = current
                .is_some_and(|c| c.model == km.model && c.provider.as_deref() == Some(km.provider));
            let marker = if is_current { " (current)" } else { "" };
            output.push_str(&format!("  {} {}{}\n", km.model, km.label, marker));
        }
        // Append configured models not already in KNOWN_MODELS for this provider.
        for (cfg_provider, cfg_model) in configured_models {
            if cfg_provider == *provider && !models.iter().any(|km| km.model == cfg_model.as_str())
            {
                let is_current = current.is_some_and(|c| {
                    c.model == *cfg_model && c.provider.as_deref() == Some(*provider)
                });
                let marker = if is_current {
                    " (configured, current)"
                } else {
                    " (configured)"
                };
                output.push_str(&format!("  {}{}\n", cfg_model, marker));
            }
        }
        if !is_configured {
            output.push_str("  (no API key configured)\n");
        }
        output.push('\n');
    }

    // Providers that have configured models but no entry in KNOWN_MODELS at all.
    // Collect unique extra providers first, then emit all their models.
    let mut extra_providers: Vec<&str> = configured_models
        .iter()
        .filter(|(p, _)| !by_provider.iter().any(|(known, _)| *known == p.as_str()))
        .map(|(p, _)| p.as_str())
        .collect();
    extra_providers.dedup();

    for provider in extra_providers {
        output.push_str(&format!("[ok] {}:\n", provider));
        for (cfg_provider, cfg_model) in configured_models {
            if cfg_provider != provider {
                continue;
            }
            let is_current = current
                .is_some_and(|c| c.model == *cfg_model && c.provider.as_deref() == Some(provider));
            let marker = if is_current {
                " (configured, current)"
            } else {
                " (configured)"
            };
            output.push_str(&format!("  {}{}\n", cfg_model, marker));
        }
        output.push('\n');
    }

    output.trim_end().to_string()
}

/// Format the `/model` (show current) output.
pub fn format_current_model(current: Option<&ModelOverride>, default_model: &str) -> String {
    match current {
        Some(ov) => {
            let provider_str = ov.provider.as_deref().unwrap_or("auto");
            format!(
                "Current: {}:{} (override)\nDefault: {}",
                provider_str, ov.model, default_model
            )
        }
        None => format!("Current: {} (default)", default_model),
    }
}

/// Persist a single chat's model override to long-term memory.
pub async fn persist_single(chat_id: &str, ov: &ModelOverride, ltm: &Arc<Mutex<LongTermMemory>>) {
    let key = format!("{}{}", MODEL_PREF_PREFIX, chat_id);
    let value = match &ov.provider {
        Some(p) => format!("{}:{}", p, ov.model),
        None => ov.model.clone(),
    };
    let mut ltm = ltm.lock().await;
    let _ = ltm
        .set(&key, &value, MODEL_PREF_CATEGORY, vec![], 0.2)
        .await;
}

/// Remove a chat's model override from long-term memory.
pub async fn remove_single(chat_id: &str, ltm: &Arc<Mutex<LongTermMemory>>) {
    let key = format!("{}{}", MODEL_PREF_PREFIX, chat_id);
    let mut ltm = ltm.lock().await;
    let _ = ltm.delete(&key).await;
}

/// Persist all overrides to long-term memory.
pub async fn persist_overrides(store: &ModelOverrideStore, ltm: &Arc<Mutex<LongTermMemory>>) {
    let map = store.read().await;
    for (chat_id, ov) in map.iter() {
        persist_single(chat_id, ov, ltm).await;
    }
}

/// Hydrate overrides from long-term memory into the in-memory store.
pub async fn hydrate_overrides(store: &ModelOverrideStore, ltm: &Arc<Mutex<LongTermMemory>>) {
    let entries: Vec<(String, String)> = {
        let ltm = ltm.lock().await;
        ltm.list_by_category(MODEL_PREF_CATEGORY)
            .iter()
            .map(|entry| (entry.key.clone(), entry.value.clone()))
            .collect()
    };

    let mut map = store.write().await;
    for (key, value) in entries {
        if let Some(chat_id) = key.strip_prefix(MODEL_PREF_PREFIX) {
            if let Some(ov) = parse_override_value(&value) {
                map.insert(chat_id.to_string(), ov);
            }
        }
    }
}

fn parse_override_value(value: &str) -> Option<ModelOverride> {
    if value.is_empty() {
        return None;
    }
    if let Some((left, right)) = value.split_once(':') {
        let is_provider = crate::providers::PROVIDER_REGISTRY
            .iter()
            .any(|s| s.name == left);
        if is_provider {
            Some(ModelOverride {
                provider: Some(left.to_string()),
                model: right.to_string(),
            })
        } else {
            Some(ModelOverride {
                provider: None,
                model: value.to_string(),
            })
        }
    } else {
        Some(ModelOverride {
            provider: None,
            model: value.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::builtin_searcher::BuiltinSearcher;

    #[test]
    fn test_parse_model_command_set_model_only() {
        let cmd = parse_model_command("/model gpt-5.4");
        assert_eq!(
            cmd,
            Some(ModelCommand::Set(ModelOverride {
                provider: None,
                model: "gpt-5.4".to_string(),
            }))
        );
    }

    #[test]
    fn test_parse_model_command_set_provider_and_model() {
        let cmd = parse_model_command("/model openai:gpt-5.4");
        assert_eq!(
            cmd,
            Some(ModelCommand::Set(ModelOverride {
                provider: Some("openai".to_string()),
                model: "gpt-5.4".to_string(),
            }))
        );
    }

    #[test]
    fn test_parse_model_command_reset() {
        let cmd = parse_model_command("/model reset");
        assert_eq!(cmd, Some(ModelCommand::Reset));
    }

    #[test]
    fn test_parse_model_command_list() {
        let cmd = parse_model_command("/model list");
        assert_eq!(cmd, Some(ModelCommand::List));
    }

    #[test]
    fn test_parse_model_command_fetch() {
        let cmd = parse_model_command("/model fetch");
        assert_eq!(cmd, Some(ModelCommand::Fetch));
    }

    #[test]
    fn test_parse_model_command_ollama_tag_not_split() {
        // "llama3.3:latest" should NOT be split into provider=llama3.3, model=latest
        let cmd = parse_model_command("/model llama3.3:latest");
        assert_eq!(
            cmd,
            Some(ModelCommand::Set(ModelOverride {
                provider: None,
                model: "llama3.3:latest".to_string(),
            }))
        );
    }

    #[test]
    fn test_parse_model_command_show_current() {
        let cmd = parse_model_command("/model");
        assert_eq!(cmd, Some(ModelCommand::Show));
    }

    #[test]
    fn test_parse_model_command_not_model() {
        let cmd = parse_model_command("hello world");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_model_command_rejects_similar_commands() {
        // Must not match commands that merely start with "/model"
        assert_eq!(parse_model_command("/models"), None);
        assert_eq!(parse_model_command("/model_test"), None);
        assert_eq!(parse_model_command("/modelling"), None);
        assert_eq!(parse_model_command("/modelx gpt-5"), None);
    }

    #[test]
    fn test_known_models_has_entries() {
        assert!(!KNOWN_MODELS.is_empty());
    }

    #[test]
    fn test_known_models_all_providers_valid() {
        let valid = [
            "anthropic",
            "openai",
            "openrouter",
            "groq",
            "ollama",
            "gemini",
            "nvidia",
            "zhipu",
            "vllm",
            "deepseek",
            "kimi",
        ];
        for km in KNOWN_MODELS {
            assert!(
                valid.contains(&km.provider),
                "Unknown provider: {}",
                km.provider
            );
        }
    }

    #[test]
    fn test_format_model_list_with_configured() {
        let configured = vec!["anthropic".to_string()];
        let output = format_model_list(&configured, None, &[]);
        assert!(output.contains("anthropic"));
        assert!(output.contains("Claude"));
    }

    #[test]
    fn test_format_model_list_shows_current() {
        let configured = vec!["anthropic".to_string()];
        let current = ModelOverride {
            provider: Some("anthropic".to_string()),
            model: "claude-opus-4-6".to_string(),
        };
        let output = format_model_list(&configured, Some(&current), &[]);
        assert!(output.contains("current"));
    }

    #[test]
    fn test_format_model_list_shows_configured_model_not_in_known() {
        let configured = vec!["nvidia".to_string()];
        let configured_models = vec![("nvidia".to_string(), "nvidia/llama-3.3-70b".to_string())];
        let output = format_model_list(&configured, None, &configured_models);
        assert!(output.contains("nvidia/llama-3.3-70b"));
        assert!(output.contains("(configured)"));
    }

    #[test]
    fn test_format_model_list_does_not_duplicate_known_model() {
        let configured = vec!["anthropic".to_string()];
        // This model is already in KNOWN_MODELS — should NOT appear twice
        let configured_models = vec![("anthropic".to_string(), "claude-opus-4-6".to_string())];
        let output = format_model_list(&configured, None, &configured_models);
        let count = output.matches("claude-opus-4-6").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_format_model_list_configured_model_shows_current() {
        let configured = vec!["nvidia".to_string()];
        let configured_models = vec![("nvidia".to_string(), "nvidia/llama-3.3-70b".to_string())];
        let current = ModelOverride {
            provider: Some("nvidia".to_string()),
            model: "nvidia/llama-3.3-70b".to_string(),
        };
        let output = format_model_list(&configured, Some(&current), &configured_models);
        assert!(output.contains("(configured, current)"));
    }

    #[test]
    fn test_format_model_list_provider_with_no_known_models() {
        // Provider not in KNOWN_MODELS at all (e.g. a custom provider)
        let configured = vec!["zhipu".to_string()];
        let configured_models = vec![("zhipu".to_string(), "glm-4-flash".to_string())];
        let output = format_model_list(&configured, None, &configured_models);
        // zhipu isn't in KNOWN_MODELS by_provider grouping, should appear as a new section
        assert!(output.contains("zhipu"));
        assert!(output.contains("glm-4-flash"));
    }

    #[test]
    fn test_known_models_no_empty_fields() {
        for km in KNOWN_MODELS {
            assert!(
                !km.provider.is_empty(),
                "provider is empty for {:?}",
                km.model
            );
            assert!(!km.model.is_empty(), "model is empty for {:?}", km.provider);
            assert!(!km.label.is_empty(), "label is empty for {:?}", km.model);
        }
    }

    #[test]
    fn test_known_models_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for km in KNOWN_MODELS {
            let key = format!("{}:{}", km.provider, km.model);
            assert!(seen.insert(key.clone()), "duplicate model entry: {}", key);
        }
    }

    #[test]
    fn test_known_models_includes_default_model() {
        let default_model = crate::config::AgentDefaults::default().model;
        assert!(
            KNOWN_MODELS.iter().any(|km| km.model == default_model),
            "Default model '{}' must appear in KNOWN_MODELS",
            default_model
        );
    }

    #[tokio::test]
    async fn test_persist_and_hydrate_model_overrides() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("longterm.json");
        let ltm = LongTermMemory::with_path_and_searcher(path, Arc::new(BuiltinSearcher)).unwrap();
        let ltm = Arc::new(Mutex::new(ltm));

        let store = new_override_store();
        {
            let mut map = store.write().await;
            map.insert(
                "chat123".to_string(),
                ModelOverride {
                    provider: Some("openai".to_string()),
                    model: "gpt-5.4".to_string(),
                },
            );
        }

        persist_overrides(&store, &ltm).await;

        let store2 = new_override_store();
        hydrate_overrides(&store2, &ltm).await;

        let map = store2.read().await;
        let ov = map.get("chat123").unwrap();
        assert_eq!(ov.model, "gpt-5.4");
        assert_eq!(ov.provider.as_deref(), Some("openai"));
    }
}
