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
        model: "claude-sonnet-4-5-20250929",
        label: "Claude Sonnet 4.5",
    },
    KnownModel {
        provider: "anthropic",
        model: "claude-haiku-4-5-20251001",
        label: "Claude Haiku 4.5",
    },
    KnownModel {
        provider: "anthropic",
        model: "claude-opus-4-6",
        label: "Claude Opus 4.6",
    },
    // OpenAI
    KnownModel {
        provider: "openai",
        model: "gpt-5.1",
        label: "GPT-5.1",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-4.1",
        label: "GPT-4.1",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-4.1-mini",
        label: "GPT-4.1 Mini",
    },
    // OpenRouter
    KnownModel {
        provider: "openrouter",
        model: "anthropic/claude-sonnet-4-5",
        label: "Claude Sonnet 4.5 (OR)",
    },
    KnownModel {
        provider: "openrouter",
        model: "google/gemini-2.5-pro",
        label: "Gemini 2.5 Pro (OR)",
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
        model: "gemini-2.5-pro",
        label: "Gemini 2.5 Pro",
    },
    KnownModel {
        provider: "gemini",
        model: "gemini-2.5-flash",
        label: "Gemini 2.5 Flash",
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
pub fn parse_model_command(text: &str) -> Option<ModelCommand> {
    let trimmed = text.trim();
    if !trimmed.starts_with("/model") {
        return None;
    }

    let rest = trimmed.strip_prefix("/model").unwrap_or("").trim();

    if rest.is_empty() {
        return Some(ModelCommand::Show);
    }

    match rest {
        "reset" => Some(ModelCommand::Reset),
        "list" => Some(ModelCommand::List),
        arg => {
            if let Some((provider, model)) = arg.split_once(':') {
                Some(ModelCommand::Set(ModelOverride {
                    provider: Some(provider.to_string()),
                    model: model.to_string(),
                }))
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
pub fn format_model_list(
    configured_providers: &[String],
    current: Option<&ModelOverride>,
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
        if !is_configured {
            output.push_str("  (no API key configured)\n");
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
    if let Some((provider, model)) = value.split_once(':') {
        Some(ModelOverride {
            provider: Some(provider.to_string()),
            model: model.to_string(),
        })
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
        let cmd = parse_model_command("/model gpt-5.1");
        assert_eq!(
            cmd,
            Some(ModelCommand::Set(ModelOverride {
                provider: None,
                model: "gpt-5.1".to_string(),
            }))
        );
    }

    #[test]
    fn test_parse_model_command_set_provider_and_model() {
        let cmd = parse_model_command("/model openai:gpt-5.1");
        assert_eq!(
            cmd,
            Some(ModelCommand::Set(ModelOverride {
                provider: Some("openai".to_string()),
                model: "gpt-5.1".to_string(),
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
        let output = format_model_list(&configured, None);
        assert!(output.contains("anthropic"));
        assert!(output.contains("Claude"));
    }

    #[test]
    fn test_format_model_list_shows_current() {
        let configured = vec!["anthropic".to_string()];
        let current = ModelOverride {
            provider: Some("anthropic".to_string()),
            model: "claude-sonnet-4-5-20250929".to_string(),
        };
        let output = format_model_list(&configured, Some(&current));
        assert!(output.contains("current"));
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
                    model: "gpt-5.1".to_string(),
                },
            );
        }

        persist_overrides(&store, &ltm).await;

        let store2 = new_override_store();
        hydrate_overrides(&store2, &ltm).await;

        let map = store2.read().await;
        let ov = map.get("chat123").unwrap();
        assert_eq!(ov.model, "gpt-5.1");
        assert_eq!(ov.provider.as_deref(), Some("openai"));
    }
}
