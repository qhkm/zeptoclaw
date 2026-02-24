//! Persona switching command parser and preset registry.
//!
//! Provides `/persona` command parsing for runtime persona switching in channels.
//!
//! # Architecture Note
//!
//! Currently implemented as Telegram-first (Approach A: metadata-based).
//! When adding /persona to more channels, consider migrating to Approach B
//! (CommandInterceptor in agent loop), mirroring the model_switch design pattern.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use crate::memory::longterm::LongTermMemory;

/// A persona preset for display in `/persona list`.
#[derive(Debug, Clone)]
pub struct PersonaPreset {
    pub name: &'static str,
    pub label: &'static str,
    pub soul_content: &'static str,
}

/// Known persona presets registry — built-in personalities for `/persona list`.
pub const PERSONA_PRESETS: &[PersonaPreset] = &[
    PersonaPreset {
        name: "default",
        label: "Default Assistant",
        soul_content: "",
    },
    PersonaPreset {
        name: "concise",
        label: "Concise & Direct",
        soul_content: "You are extremely concise. Answer in as few words as possible. No filler, no pleasantries. Get straight to the point.",
    },
    PersonaPreset {
        name: "friendly",
        label: "Friendly & Warm",
        soul_content: "You are warm, friendly, and encouraging. Use a conversational tone. Show genuine interest in helping. Be supportive and positive.",
    },
    PersonaPreset {
        name: "professional",
        label: "Professional & Formal",
        soul_content: "You are professional and formal. Use precise language. Structure responses clearly. Maintain a business-appropriate tone at all times.",
    },
    PersonaPreset {
        name: "creative",
        label: "Creative & Playful",
        soul_content: "You are creative, playful, and imaginative. Use vivid language, metaphors, and humor when appropriate. Think outside the box.",
    },
    PersonaPreset {
        name: "technical",
        label: "Technical Expert",
        soul_content: "You are a technical expert. Provide detailed, accurate technical explanations. Include code examples when relevant. Prioritize precision over simplicity.",
    },
];

/// Parsed `/persona` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonaCommand {
    /// `/persona` — show current persona
    Show,
    /// `/persona <name_or_custom>` — set persona by preset name or custom text
    Set(String),
    /// `/persona reset` — clear override
    Reset,
    /// `/persona list` — show available presets
    List,
}

/// Thread-safe store for per-chat persona overrides.
///
/// The value is the persona name (preset) or custom soul content text.
pub type PersonaOverrideStore = Arc<RwLock<HashMap<String, String>>>;

const PERSONA_PREF_CATEGORY: &str = "persona_pref";
const PERSONA_PREF_PREFIX: &str = "persona_pref:";

/// Create a new empty persona override store.
pub fn new_persona_store() -> PersonaOverrideStore {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Parse a message as a `/persona` command. Returns None if not a `/persona` command.
///
/// Only matches exactly `/persona` or `/persona ` followed by args. Does NOT match
/// `/personas`, `/persona_x`, or other commands that happen to start with `/persona`.
pub fn parse_persona_command(text: &str) -> Option<PersonaCommand> {
    let trimmed = text.trim();

    // Must be exactly "/persona" or "/persona " followed by args
    let rest = if trimmed == "/persona" {
        ""
    } else if let Some(after) = trimmed.strip_prefix("/persona ") {
        after.trim()
    } else {
        return None;
    };

    if rest.is_empty() {
        return Some(PersonaCommand::Show);
    }

    match rest {
        "reset" => Some(PersonaCommand::Reset),
        "list" => Some(PersonaCommand::List),
        arg => Some(PersonaCommand::Set(arg.to_string())),
    }
}

/// Format the `/persona list` output showing all presets with current marker.
pub fn format_persona_list(current: Option<&str>) -> String {
    let mut output = String::from("Available personas:\n\n");

    for preset in PERSONA_PRESETS {
        let is_current = current.is_some_and(|c| c == preset.name);
        let marker = if is_current { " (current)" } else { "" };
        output.push_str(&format!("  {} — {}{}\n", preset.name, preset.label, marker));
    }

    output.push_str(
        "\nUse /persona <name> to set a preset, or /persona <custom text> for a custom persona.",
    );
    output.trim_end().to_string()
}

/// Format the `/persona` (show current) output.
pub fn format_current_persona(current: Option<&str>) -> String {
    match current {
        Some(name) => {
            // Check if it matches a known preset name
            if let Some(preset) = PERSONA_PRESETS.iter().find(|p| p.name == name) {
                format!(
                    "Current persona: {} — {} (override)",
                    preset.name, preset.label
                )
            } else {
                format!("Current persona: custom override\nContent: {}", name)
            }
        }
        None => "Current persona: default (no override)".to_string(),
    }
}

/// Persist a single chat's persona override to long-term memory.
pub async fn persist_single(chat_id: &str, value: &str, ltm: &Arc<Mutex<LongTermMemory>>) {
    let key = format!("{}{}", PERSONA_PREF_PREFIX, chat_id);
    let mut ltm = ltm.lock().await;
    let _ = ltm
        .set(&key, value, PERSONA_PREF_CATEGORY, vec![], 0.2)
        .await;
}

/// Remove a chat's persona override from long-term memory.
pub async fn remove_single(chat_id: &str, ltm: &Arc<Mutex<LongTermMemory>>) {
    let key = format!("{}{}", PERSONA_PREF_PREFIX, chat_id);
    let mut ltm = ltm.lock().await;
    let _ = ltm.delete(&key).await;
}

/// Hydrate persona overrides from long-term memory into the in-memory store.
pub async fn hydrate_overrides(store: &PersonaOverrideStore, ltm: &Arc<Mutex<LongTermMemory>>) {
    let entries: Vec<(String, String)> = {
        let ltm = ltm.lock().await;
        ltm.list_by_category(PERSONA_PREF_CATEGORY)
            .iter()
            .map(|entry| (entry.key.clone(), entry.value.clone()))
            .collect()
    };

    let mut map = store.write().await;
    for (key, value) in entries {
        if let Some(chat_id) = key.strip_prefix(PERSONA_PREF_PREFIX) {
            if !value.is_empty() {
                map.insert(chat_id.to_string(), value);
            }
        }
    }
}

/// Resolve a persona name or custom text to its soul content.
///
/// If the input matches a known preset name, returns that preset's `soul_content`.
/// Otherwise treats the input as raw custom soul content and returns it as-is.
pub fn resolve_soul_content(name_or_text: &str) -> String {
    if let Some(preset) = PERSONA_PRESETS.iter().find(|p| p.name == name_or_text) {
        preset.soul_content.to_string()
    } else {
        name_or_text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::builtin_searcher::BuiltinSearcher;

    #[test]
    fn test_parse_persona_command_show() {
        let cmd = parse_persona_command("/persona");
        assert_eq!(cmd, Some(PersonaCommand::Show));
    }

    #[test]
    fn test_parse_persona_command_list() {
        let cmd = parse_persona_command("/persona list");
        assert_eq!(cmd, Some(PersonaCommand::List));
    }

    #[test]
    fn test_parse_persona_command_reset() {
        let cmd = parse_persona_command("/persona reset");
        assert_eq!(cmd, Some(PersonaCommand::Reset));
    }

    #[test]
    fn test_parse_persona_command_set_preset() {
        let cmd = parse_persona_command("/persona concise");
        assert_eq!(cmd, Some(PersonaCommand::Set("concise".to_string())));
    }

    #[test]
    fn test_parse_persona_command_set_custom() {
        let cmd = parse_persona_command("/persona Be a pirate");
        assert_eq!(cmd, Some(PersonaCommand::Set("Be a pirate".to_string())));
    }

    #[test]
    fn test_parse_persona_command_not_persona() {
        let cmd = parse_persona_command("hello");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_persona_rejects_similar() {
        // Must not match commands that merely start with "/persona"
        assert_eq!(parse_persona_command("/personas"), None);
        assert_eq!(parse_persona_command("/persona_x"), None);
    }

    #[test]
    fn test_format_persona_list_shows_presets() {
        let output = format_persona_list(None);
        assert!(output.contains("default"));
        assert!(output.contains("concise"));
        assert!(output.contains("friendly"));
        assert!(output.contains("professional"));
        assert!(output.contains("creative"));
        assert!(output.contains("technical"));
    }

    #[test]
    fn test_resolve_soul_content_preset() {
        let content = resolve_soul_content("concise");
        assert_eq!(
            content,
            "You are extremely concise. Answer in as few words as possible. No filler, no pleasantries. Get straight to the point."
        );
    }

    #[test]
    fn test_resolve_soul_content_custom() {
        let content = resolve_soul_content("Be a pirate");
        assert_eq!(content, "Be a pirate");
    }

    #[tokio::test]
    async fn test_persist_and_hydrate_persona() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("longterm.json");
        let ltm = LongTermMemory::with_path_and_searcher(path, Arc::new(BuiltinSearcher)).unwrap();
        let ltm = Arc::new(Mutex::new(ltm));

        let store = new_persona_store();
        {
            let mut map = store.write().await;
            map.insert("chat456".to_string(), "concise".to_string());
        }

        // Persist the override from the store
        {
            let map = store.read().await;
            for (chat_id, value) in map.iter() {
                persist_single(chat_id, value, &ltm).await;
            }
        }

        // Hydrate into a fresh store
        let store2 = new_persona_store();
        hydrate_overrides(&store2, &ltm).await;

        let map = store2.read().await;
        let value = map.get("chat456").unwrap();
        assert_eq!(value, "concise");
    }
}
