//! Template command handler.

use anyhow::Result;

use zeptoclaw::config::templates::TemplateRegistry;

use super::common::load_template_registry;
use super::TemplateAction;

/// Manage agent templates.
pub(crate) async fn cmd_template(action: TemplateAction) -> Result<()> {
    let registry = load_template_registry()?;

    match action {
        TemplateAction::List => {
            let builtin = TemplateRegistry::new()
                .names()
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect::<std::collections::HashSet<_>>();

            let mut templates = registry.list().into_iter().cloned().collect::<Vec<_>>();
            templates.sort_by(|a, b| a.name.cmp(&b.name));

            if templates.is_empty() {
                println!("No templates available.");
                return Ok(());
            }

            println!("Templates:");
            for tpl in templates {
                let origin = if builtin.contains(&tpl.name) {
                    "built-in"
                } else {
                    "user"
                };
                println!("  - {} ({}) — {}", tpl.name, origin, tpl.description);
            }
        }
        TemplateAction::Show { name } => {
            let Some(tpl) = registry.get(&name) else {
                anyhow::bail!("Template '{}' not found", name);
            };

            println!("Name: {}", tpl.name);
            println!("Description: {}", tpl.description);
            if let Some(model) = &tpl.model {
                println!("Model override: {}", model);
            }
            if let Some(max_tokens) = tpl.max_tokens {
                println!("Max tokens override: {}", max_tokens);
            }
            if let Some(temperature) = tpl.temperature {
                println!("Temperature override: {}", temperature);
            }
            if let Some(max_tool_iterations) = tpl.max_tool_iterations {
                println!("Max tool iterations override: {}", max_tool_iterations);
            }
            if let Some(allowed) = &tpl.allowed_tools {
                println!("Allowed tools: {}", allowed.join(", "));
            }
            if let Some(blocked) = &tpl.blocked_tools {
                println!("Blocked tools: {}", blocked.join(", "));
            }
            if !tpl.tags.is_empty() {
                println!("Tags: {}", tpl.tags.join(", "));
            }
            println!();
            println!("System prompt:");
            println!("{}", tpl.system_prompt);
        }
    }

    Ok(())
}
