//! Hands-lite registry and manifest parsing.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, ZeptoError};

#[derive(Debug, Clone)]
pub struct Hand {
    pub manifest: HandManifest,
    pub skill_md: String,
    pub source: HandSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandSource {
    BuiltIn,
    User,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HandManifest {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub settings: HashMap<String, String>,
    #[serde(default)]
    pub guardrails: HandGuardrails,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HandGuardrails {
    #[serde(default)]
    pub require_approval_for: Vec<String>,
}

pub fn parse_hand_toml(content: &str) -> Result<HandManifest> {
    toml::from_str(content).map_err(|e| ZeptoError::Config(format!("Invalid HAND.toml: {}", e)))
}

pub fn built_in_hands() -> Vec<Hand> {
    vec![built_in_researcher(), built_in_coder(), built_in_monitor()]
}

pub fn resolve_hand(name: &str, hands_dir: &Path) -> Result<Option<Hand>> {
    if let Some(hand) = built_in_hands()
        .into_iter()
        .find(|h| h.manifest.name.eq_ignore_ascii_case(name))
    {
        return Ok(Some(hand));
    }

    let user_hands = load_hands_from_dir(hands_dir)?;
    Ok(user_hands
        .into_iter()
        .find(|h| h.manifest.name.eq_ignore_ascii_case(name)))
}

pub fn load_hands_from_dir(dir: &Path) -> Result<Vec<Hand>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if !dir.is_dir() {
        return Err(ZeptoError::Config(format!(
            "Hands path is not a directory: {}",
            dir.display()
        )));
    }

    let mut hands = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(hand) = load_hand_dir(&path)? {
            hands.push(hand);
        }
    }
    Ok(hands)
}

fn load_hand_dir(path: &Path) -> Result<Option<Hand>> {
    let manifest_path = path.join("HAND.toml");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let skill_path = path.join("SKILL.md");
    let manifest_raw = std::fs::read_to_string(&manifest_path)?;
    let manifest = parse_hand_toml(&manifest_raw)?;
    let skill_md = if skill_path.is_file() {
        std::fs::read_to_string(skill_path)?
    } else {
        String::new()
    };
    Ok(Some(Hand {
        manifest,
        skill_md,
        source: HandSource::User,
    }))
}

fn built_in_researcher() -> Hand {
    Hand {
        manifest: HandManifest {
            name: "researcher".to_string(),
            description: "Autonomous web researcher with citation-first output".to_string(),
            system_prompt: "You are Researcher Hand. Build evidence-backed answers: plan search steps, gather sources, cross-check claims, and produce concise cited summaries. Prefer primary sources. Call out uncertainty. End with sources used."
                .to_string(),
            required_tools: vec![
                "web_search".to_string(),
                "web_fetch".to_string(),
                "memory_search".to_string(),
                "memory_get".to_string(),
            ],
            settings: HashMap::new(),
            guardrails: HandGuardrails {
                require_approval_for: vec!["shell*".to_string(), "write_*".to_string()],
            },
        },
        skill_md: "# Researcher Skill\nPrioritize primary sources and include citations.".to_string(),
        source: HandSource::BuiltIn,
    }
}

fn built_in_coder() -> Hand {
    Hand {
        manifest: HandManifest {
            name: "coder".to_string(),
            description: "Code-focused hand limited to repo-safe tools".to_string(),
            system_prompt: "You are Coder Hand. Write safe, minimal diffs. Validate assumptions before editing. Prefer deterministic commands, include tests, and avoid unrelated changes."
                .to_string(),
            required_tools: vec![
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "list_dir".to_string(),
                "git".to_string(),
            ],
            settings: HashMap::new(),
            guardrails: HandGuardrails {
                require_approval_for: vec!["shell*".to_string()],
            },
        },
        skill_md: "# Coder Skill\nKeep patches small, test-backed, and reversible.".to_string(),
        source: HandSource::BuiltIn,
    }
}

fn built_in_monitor() -> Hand {
    Hand {
        manifest: HandManifest {
            name: "monitor".to_string(),
            description: "URL/API watcher with proactive notifications".to_string(),
            system_prompt: "You are Monitor Hand. Check targets periodically, detect meaningful change, and notify with terse diffs and severity. Avoid noisy updates."
                .to_string(),
            required_tools: vec![
                "web_fetch".to_string(),
                "http_request".to_string(),
                "message".to_string(),
                "cron".to_string(),
            ],
            settings: HashMap::new(),
            guardrails: HandGuardrails {
                require_approval_for: vec!["shell*".to_string(), "write_*".to_string()],
            },
        },
        skill_md: "# Monitor Skill\nOnly notify on meaningful changes.".to_string(),
        source: HandSource::BuiltIn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hand_toml() {
        let raw = r#"
name = "qa"
description = "QA hand"
system_prompt = "test"
required_tools = ["read_file", "git"]

[guardrails]
require_approval_for = ["shell*"]
"#;
        let hand = parse_hand_toml(raw).unwrap();
        assert_eq!(hand.name, "qa");
        assert_eq!(hand.required_tools.len(), 2);
        assert_eq!(hand.guardrails.require_approval_for, vec!["shell*"]);
    }
}
