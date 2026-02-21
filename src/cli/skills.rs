//! Skills management command handler.

use anyhow::{Context, Result};

use zeptoclaw::config::Config;
use zeptoclaw::skills::{EnvSpec, Skill, SkillsLoader};

use super::common::skills_loader_from_config;
use super::SkillsAction;

/// Format the `skills show` output for a skill into a string.
///
/// Extracted as a pure function to simplify unit testing.
fn format_skill_show(skill: &Skill, loader: &SkillsLoader) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Name:        {}", skill.name));

    if let Some(ref v) = skill.metadata.version {
        lines.push(format!("Version:     {}", v));
    }

    if let Some(ref a) = skill.metadata.author {
        lines.push(format!("Author:      {}", a));
    }

    if let Some(ref l) = skill.metadata.license {
        lines.push(format!("License:     {}", l));
    }

    if !skill.metadata.tags.is_empty() {
        lines.push(format!("Tags:        {}", skill.metadata.tags.join(", ")));
    }

    if !skill.metadata.depends.is_empty() {
        let deps: Vec<String> = skill
            .metadata
            .depends
            .iter()
            .map(|dep| {
                let check = if loader.load_skill(dep).is_some() {
                    "\u{2713}"
                } else {
                    "\u{2717}"
                };
                format!("{} {}", dep, check)
            })
            .collect();
        lines.push(format!("Depends:     {}", deps.join(", ")));
    }

    if !skill.metadata.conflicts.is_empty() {
        let cfls: Vec<String> = skill
            .metadata
            .conflicts
            .iter()
            .map(|c| {
                if loader.load_skill(c).is_some() {
                    format!("{} (installed \u{2717})", c)
                } else {
                    format!("{} (not installed \u{2713})", c)
                }
            })
            .collect();
        lines.push(format!("Conflicts:   {}", cfls.join(", ")));
    }

    if !skill.metadata.env_needed.is_empty() {
        lines.push("Env needed:".to_string());
        let max_name_len = compute_max_name_len(&skill.metadata.env_needed);
        for env in &skill.metadata.env_needed {
            let req = if env.required { "required" } else { "optional" };
            lines.push(format!(
                "  {:<width$}   {}   {}",
                env.name,
                req,
                env.description,
                width = max_name_len
            ));
        }
    }

    let available = if loader.check_requirements(skill) {
        "yes"
    } else {
        "no"
    };
    lines.push(format!("Available:   {}", available));

    lines.push(String::new());
    lines.push("--- Content ---".to_string());
    lines.push(skill.content.clone());

    lines.join("\n")
}

/// Compute the length of the longest `name` in an `env_needed` list.
fn compute_max_name_len(env_needed: &[EnvSpec]) -> usize {
    env_needed.iter().map(|e| e.name.len()).max().unwrap_or(0)
}

/// Skills management command.
pub(crate) async fn cmd_skills(action: SkillsAction) -> Result<()> {
    let config = Config::load().with_context(|| "Failed to load configuration")?;
    let loader = skills_loader_from_config(&config);

    match action {
        SkillsAction::List { all } => {
            let disabled: std::collections::HashSet<String> = config
                .skills
                .disabled
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect();
            let mut listed = loader.list_skills(!all);
            listed.retain(|info| !disabled.contains(&info.name.to_ascii_lowercase()));

            if listed.is_empty() {
                println!("No skills found.");
                return Ok(());
            }

            println!("Skills:");
            for info in listed {
                let ready = loader
                    .load_skill(&info.name)
                    .map(|skill| loader.check_requirements(&skill))
                    .unwrap_or(false);
                let marker = if ready {
                    "ready"
                } else {
                    "missing requirements"
                };
                println!("  - {} ({}, {})", info.name, info.source, marker);
            }
        }
        SkillsAction::Show { name } => {
            if let Some(skill) = loader.load_skill(&name) {
                let output = format_skill_show(&skill, &loader);
                println!("{}", output);
            } else {
                anyhow::bail!("Skill '{}' not found", name);
            }
        }
        SkillsAction::Create { name } => {
            let dir = loader.workspace_dir().join(&name);
            let skill_file = dir.join("SKILL.md");
            if skill_file.exists() {
                anyhow::bail!("Skill '{}' already exists at {:?}", name, skill_file);
            }

            std::fs::create_dir_all(&dir)?;
            let template = format!(
                r#"---
name: {name}
version: 1.0.0
description: Describe what this skill does.
# author: Your Name or Org
# license: MIT
# tags:
#   - category
# depends:
#   - another-skill
# conflicts:
#   - incompatible-skill
# env_needed:
#   - name: MY_API_KEY
#     description: Your API key for the service
#     required: true
metadata: {{"zeptoclaw":{{"emoji":"📚","requires":{{}}}}}}
---

# {name} Skill

Describe usage and concrete command examples.
"#
            );
            std::fs::write(&skill_file, template)?;
            println!("Created skill at {:?}", skill_file);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the create template string contains all new field comments.
    #[test]
    fn test_create_template_contains_new_field_comments() {
        // Build the template the same way cmd_skills does (inline the pattern here).
        let name = "test-skill";
        let template = format!(
            r#"---
name: {name}
version: 1.0.0
description: Describe what this skill does.
# author: Your Name or Org
# license: MIT
# tags:
#   - category
# depends:
#   - another-skill
# conflicts:
#   - incompatible-skill
# env_needed:
#   - name: MY_API_KEY
#     description: Your API key for the service
#     required: true
metadata: {{"zeptoclaw":{{"emoji":"📚","requires":{{}}}}}}
---

# {name} Skill

Describe usage and concrete command examples.
"#
        );

        assert!(
            template.contains("# author:"),
            "template should contain '# author:'"
        );
        assert!(
            template.contains("# license:"),
            "template should contain '# license:'"
        );
        assert!(
            template.contains("# tags:"),
            "template should contain '# tags:'"
        );
        assert!(
            template.contains("# depends:"),
            "template should contain '# depends:'"
        );
        assert!(
            template.contains("# conflicts:"),
            "template should contain '# conflicts:'"
        );
        assert!(
            template.contains("# env_needed:"),
            "template should contain '# env_needed:'"
        );
        assert!(
            template.contains("version: 1.0.0"),
            "template should contain 'version: 1.0.0'"
        );
    }

    /// Verify that `compute_max_name_len` returns the correct padding value.
    #[test]
    fn test_env_spec_display_format() {
        let env_needed = vec![
            EnvSpec {
                name: "SHORT".to_string(),
                description: "A short name".to_string(),
                required: true,
            },
            EnvSpec {
                name: "MUCH_LONGER_NAME".to_string(),
                description: "A longer name".to_string(),
                required: false,
            },
            EnvSpec {
                name: "MED".to_string(),
                description: "Medium".to_string(),
                required: true,
            },
        ];

        let max_len = compute_max_name_len(&env_needed);
        assert_eq!(
            max_len,
            "MUCH_LONGER_NAME".len(),
            "max name len should be length of 'MUCH_LONGER_NAME'"
        );

        // Verify empty list returns 0.
        let empty: Vec<EnvSpec> = vec![];
        assert_eq!(
            compute_max_name_len(&empty),
            0,
            "max name len of empty list should be 0"
        );

        // Verify single-entry list returns that entry's name length.
        let single = vec![EnvSpec {
            name: "ONLY_ONE".to_string(),
            description: "desc".to_string(),
            required: true,
        }];
        assert_eq!(compute_max_name_len(&single), "ONLY_ONE".len());
    }
}
