//! Hand command handler.

use anyhow::{Context, Result};

use zeptoclaw::config::Config;
use zeptoclaw::hands::{built_in_hands, load_hands_from_dir, resolve_hand, HandSource};

use super::HandAction;

pub(crate) async fn cmd_hand(action: HandAction) -> Result<()> {
    match action {
        HandAction::List => {
            let mut hands = built_in_hands();
            let user_hands = load_hands_from_dir(&Config::dir().join("hands"))
                .with_context(|| "Failed to load user hands")?;
            hands.extend(user_hands);
            hands.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));

            if hands.is_empty() {
                println!("No hands available.");
                return Ok(());
            }

            println!("Hands:");
            for hand in hands {
                let source = match hand.source {
                    HandSource::BuiltIn => "built-in",
                    HandSource::User => "user",
                };
                println!(
                    "  - {} ({}) — {}",
                    hand.manifest.name, source, hand.manifest.description
                );
            }
        }
        HandAction::Activate { name } => {
            let hands_dir = Config::dir().join("hands");
            let hand = resolve_hand(&name, &hands_dir)?
                .with_context(|| format!("Hand '{}' not found", name))?;
            let mut cfg = Config::load().with_context(|| "Failed to load config")?;
            cfg.agents.defaults.active_hand = Some(hand.manifest.name.clone());
            cfg.save().with_context(|| "Failed to save config")?;
            println!("Activated hand: {}", hand.manifest.name);
        }
        HandAction::Deactivate => {
            let mut cfg = Config::load().with_context(|| "Failed to load config")?;
            if cfg.agents.defaults.active_hand.is_none() {
                println!("No active hand to deactivate.");
                return Ok(());
            }
            let name = cfg.agents.defaults.active_hand.take().unwrap();
            cfg.save().with_context(|| "Failed to save config")?;
            println!("Deactivated hand: {}", name);
        }
        HandAction::Status => {
            let cfg = Config::load().with_context(|| "Failed to load config")?;
            let Some(active) = cfg.agents.defaults.active_hand.as_deref() else {
                println!("No active hand.");
                return Ok(());
            };
            let hands_dir = Config::dir().join("hands");
            let hand = resolve_hand(active, &hands_dir)?
                .with_context(|| format!("Active hand '{}' not found", active))?;
            println!("Active hand: {}", hand.manifest.name);
            println!("Description: {}", hand.manifest.description);
            if !hand.manifest.required_tools.is_empty() {
                println!("Tools: {}", hand.manifest.required_tools.join(", "));
            }
            if !hand.manifest.guardrails.require_approval_for.is_empty() {
                println!(
                    "Guardrails: require approval for {}",
                    hand.manifest.guardrails.require_approval_for.join(", ")
                );
            }
        }
    }

    Ok(())
}
