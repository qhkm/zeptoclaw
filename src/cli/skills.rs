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
        SkillsAction::Search { query, source } => {
            cmd_skills_search(&config, &query, &source).await?;
        }
        SkillsAction::Install { name, github } => {
            cmd_skills_install(&name, github.as_deref()).await?;
        }
    }

    Ok(())
}

async fn cmd_skills_search(config: &Config, query: &str, source: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let mut all_results = Vec::new();

    // GitHub search
    if source == "all" || source == "github" {
        let topics = &["zeptoclaw-skill", "openclaw-skill"];
        match zeptoclaw::skills::github_source::search_github(&client, query, topics).await {
            Ok(results) => all_results.extend(results),
            Err(e) => eprintln!("GitHub search failed: {}", e),
        }
    }

    // ClawHub search (reserved — config check kept for future integration)
    if source == "all" || source == "clawhub" {
        let _ = config; // config used for future ClawHub API calls
    }

    // Sort by score descending
    all_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if all_results.is_empty() {
        println!("No skills found matching '{}'", query);
        return Ok(());
    }

    println!("Found {} skill(s):\n", all_results.len());
    for r in &all_results {
        let source_label = match r.source {
            zeptoclaw::skills::github_source::SkillSource::GitHub => "github",
            zeptoclaw::skills::github_source::SkillSource::ClawHub => "clawhub",
        };
        println!(
            "  {} ({}) [{}] score={:.2} stars={}",
            r.name, r.slug, source_label, r.score, r.stars
        );
        if !r.description.is_empty() {
            println!("    {}", r.description);
        }
        println!();
    }

    Ok(())
}

/// Default community skills repository.
const COMMUNITY_REPO: &str = "qhkm/zeptoclaw-skills";

/// Validate a skill name for filesystem safety.
fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Skill name cannot be empty");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("Skill name cannot contain path separators: {:?}", name);
    }
    if name.starts_with('.') || name == ".." {
        anyhow::bail!("Skill name cannot start with dot: {:?}", name);
    }
    if name.contains("..") {
        anyhow::bail!("Skill name cannot contain '..': {:?}", name);
    }
    Ok(())
}

async fn cmd_skills_install(name: &str, github: Option<&str>) -> Result<()> {
    validate_skill_name(name)?;

    let skills_dir = zeptoclaw::config::Config::dir().join("skills");
    std::fs::create_dir_all(&skills_dir)?;
    let target_dir = skills_dir.join(name);

    if target_dir.exists() {
        anyhow::bail!(
            "Skill '{}' already exists at {}. Remove it first.",
            name,
            target_dir.display()
        );
    }

    if let Some(repo_arg) = github {
        let normalized = normalize_github_repo(repo_arg);
        let segments: Vec<&str> = normalized.split('/').collect();
        match segments.len() {
            2 => {
                // Single-skill repo: owner/repo → clone directly
                install_single_skill_repo(normalized, name, &target_dir).await?;
            }
            n if n >= 3 => {
                // Multi-skill repo: owner/repo/skill-path
                let repo_part = format!("{}/{}", segments[0], segments[1]);
                let skill_path = segments[2..].join("/");
                install_from_multi_skill_repo(&repo_part, &skill_path, &target_dir).await?;
            }
            _ => {
                anyhow::bail!(
                    "Expected owner/repo or owner/repo/skill format, got: {}",
                    normalized
                );
            }
        }
    } else {
        // Default: install from community repo
        install_from_multi_skill_repo(COMMUNITY_REPO, name, &target_dir).await?;
    }

    Ok(())
}

/// Normalize a GitHub argument, accepting both full URLs and shorthand.
fn normalize_github_repo(input: &str) -> &str {
    input
        .strip_prefix("https://github.com/")
        .or_else(|| input.strip_prefix("http://github.com/"))
        .unwrap_or(input)
        .trim_end_matches('/')
        .trim_end_matches(".git")
}

/// Install a single-skill repo (root SKILL.md) by cloning directly into target.
async fn install_single_skill_repo(
    repo: &str,
    name: &str,
    target_dir: &std::path::Path,
) -> Result<()> {
    println!("Installing '{}' from github.com/{} ...", name, repo);

    let output = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            &format!("https://github.com/{}.git", repo),
        ])
        .arg(target_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git clone failed for {}: {}", repo, stderr.trim());
    }

    let skill_md = target_dir.join("SKILL.md");
    if !skill_md.exists() {
        let _ = std::fs::remove_dir_all(target_dir);
        anyhow::bail!("Repository {} has no SKILL.md — not a valid skill", repo);
    }

    let _ = std::fs::remove_dir_all(target_dir.join(".git"));
    println!("Installed '{}' to {}", name, target_dir.display());
    Ok(())
}

/// Install a specific skill subdirectory from a multi-skill repo.
async fn install_from_multi_skill_repo(
    repo: &str,
    skill_path: &str,
    target_dir: &std::path::Path,
) -> Result<()> {
    println!("Installing '{}' from github.com/{} ...", skill_path, repo);

    let tmp_dir =
        std::env::temp_dir().join(format!("zeptoclaw-skill-install-{}", std::process::id()));

    let output = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            &format!("https://github.com/{}.git", repo),
        ])
        .arg(&tmp_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_dir_all(&tmp_dir);
        anyhow::bail!("git clone failed for {}: {}", repo, stderr.trim());
    }

    let skill_src = tmp_dir.join(skill_path);
    let skill_md = skill_src.join("SKILL.md");

    if !skill_md.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        anyhow::bail!(
            "Skill '{}' not found in {} (no {}/SKILL.md)",
            skill_path,
            repo,
            skill_path,
        );
    }

    copy_dir_recursive(&skill_src, target_dir)?;
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let name = target_dir
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    println!("Installed '{}' to {}", name, target_dir.display());
    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
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

    #[test]
    fn test_normalize_github_repo() {
        // Full HTTPS URL
        assert_eq!(
            normalize_github_repo("https://github.com/steipete/gogcli"),
            "steipete/gogcli"
        );
        // Trailing slash on URL
        assert_eq!(
            normalize_github_repo("https://github.com/owner/repo/"),
            "owner/repo"
        );
        // HTTP variant
        assert_eq!(
            normalize_github_repo("http://github.com/owner/repo"),
            "owner/repo"
        );
        // Already owner/repo shorthand — unchanged
        assert_eq!(normalize_github_repo("owner/repo"), "owner/repo");
        // org with trailing slash in shorthand
        assert_eq!(normalize_github_repo("owner/repo/"), "owner/repo");
        // .git suffix stripped
        assert_eq!(
            normalize_github_repo("https://github.com/owner/repo.git"),
            "owner/repo"
        );
        // .git suffix on shorthand
        assert_eq!(normalize_github_repo("owner/repo.git"), "owner/repo");
        // Sub-path preserved for multi-skill repos
        assert_eq!(
            normalize_github_repo("https://github.com/qhkm/zeptoclaw-skills/obsidian-vault"),
            "qhkm/zeptoclaw-skills/obsidian-vault"
        );
        // Shorthand with skill path
        assert_eq!(
            normalize_github_repo("qhkm/zeptoclaw-skills/weather"),
            "qhkm/zeptoclaw-skills/weather"
        );
    }

    #[test]
    fn test_validate_skill_name() {
        // Valid names
        assert!(validate_skill_name("obsidian-vault").is_ok());
        assert!(validate_skill_name("my_skill").is_ok());
        assert!(validate_skill_name("weather").is_ok());
        assert!(validate_skill_name("send-email").is_ok());

        // Invalid: empty
        assert!(validate_skill_name("").is_err());
        // Invalid: path traversal
        assert!(validate_skill_name("..").is_err());
        assert!(validate_skill_name("../escape").is_err());
        assert!(validate_skill_name("foo/bar").is_err());
        // Invalid: hidden files
        assert!(validate_skill_name(".hidden").is_err());
        // Invalid: backslash
        assert!(validate_skill_name("foo\\bar").is_err());
    }

    #[test]
    fn test_github_segment_parsing() {
        // 2 segments → single-skill repo
        let normalized = normalize_github_repo("user/my-skill");
        let segments: Vec<&str> = normalized.split('/').collect();
        assert_eq!(segments.len(), 2);

        // 3 segments → multi-skill repo with skill path
        let normalized =
            normalize_github_repo("https://github.com/qhkm/zeptoclaw-skills/obsidian-vault");
        let segments: Vec<&str> = normalized.split('/').collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(
            format!("{}/{}", segments[0], segments[1]),
            "qhkm/zeptoclaw-skills"
        );
        assert_eq!(segments[2..].join("/"), "obsidian-vault");
    }

    #[test]
    fn test_copy_dir_recursive() {
        let tmp = std::env::temp_dir().join("zeptoclaw-test-copy-dir");
        let _ = std::fs::remove_dir_all(&tmp);

        let src = tmp.join("src");
        let dst = tmp.join("dst");

        // Create source structure
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("SKILL.md"), "# Test").unwrap();
        std::fs::write(src.join("sub/nested.txt"), "nested").unwrap();

        // Copy
        copy_dir_recursive(&src, &dst).unwrap();

        // Verify
        assert!(dst.join("SKILL.md").exists());
        assert!(dst.join("sub/nested.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dst.join("SKILL.md")).unwrap(),
            "# Test"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("sub/nested.txt")).unwrap(),
            "nested"
        );

        // Clean up
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_community_repo_constant() {
        assert_eq!(COMMUNITY_REPO, "qhkm/zeptoclaw-skills");
        let segments: Vec<&str> = COMMUNITY_REPO.split('/').collect();
        assert_eq!(segments.len(), 2);
    }
}
