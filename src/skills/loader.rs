//! Skills loader and parser.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use regex::Regex;

use super::types::{Skill, SkillInfo, SkillMetadata, ZeptoMetadata};

const BUILTIN_SKILLS_DIR: &str = "skills";

/// Discover and load markdown skills from workspace and builtin directories.
pub struct SkillsLoader {
    workspace_dir: PathBuf,
    builtin_dir: PathBuf,
}

impl SkillsLoader {
    /// Create loader with explicit directories.
    pub fn new(workspace_dir: PathBuf, builtin_dir: Option<PathBuf>) -> Self {
        let builtin = builtin_dir.unwrap_or_else(default_builtin_skills_dir);
        Self {
            workspace_dir,
            builtin_dir: builtin,
        }
    }

    /// Create loader with default directories.
    pub fn with_defaults() -> Self {
        let workspace = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
            .join("skills");
        Self::new(workspace, None)
    }

    /// Workspace skill directory.
    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Builtin skill directory.
    pub fn builtin_dir(&self) -> &Path {
        &self.builtin_dir
    }

    /// List known skills (`workspace` overrides `builtin` by name).
    pub fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();

        collect_skill_infos(&self.workspace_dir, "workspace", &mut out, &mut seen);
        collect_skill_infos(&self.builtin_dir, "builtin", &mut out, &mut seen);

        if filter_unavailable {
            out.retain(|info| {
                self.load_skill(&info.name)
                    .map(|skill| self.check_requirements(&skill))
                    .unwrap_or(false)
            });
        }

        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Load one skill by name.
    pub fn load_skill(&self, name: &str) -> Option<Skill> {
        let workspace = self.workspace_dir.join(name).join("SKILL.md");
        if workspace.is_file() {
            return self.parse_skill_file(&workspace, name, "workspace");
        }

        let builtin = self.builtin_dir.join(name).join("SKILL.md");
        if builtin.is_file() {
            return self.parse_skill_file(&builtin, name, "builtin");
        }

        None
    }

    /// Build summary XML block for prompt context.
    pub fn build_skills_summary(&self) -> String {
        let skills = self.list_skills(false);
        if skills.is_empty() {
            return String::new();
        }

        let mut lines = vec!["<skills>".to_string()];
        for info in skills {
            if let Some(skill) = self.load_skill(&info.name) {
                let available = self.check_requirements(&skill);
                let emoji = self.get_zeptometa(&skill).emoji.unwrap_or_default();
                let desc = escape_xml(&skill.description);
                lines.push(format!("  <skill available=\"{}\">", available));
                lines.push(format!(
                    "    <name>{}{}</name>",
                    emoji,
                    escape_xml(&skill.name)
                ));
                lines.push(format!("    <description>{}</description>", desc));
                lines.push(format!(
                    "    <location>{}</location>",
                    escape_xml(&skill.path)
                ));
                lines.push("  </skill>".to_string());
            }
        }
        lines.push("</skills>".to_string());
        lines.join("\n")
    }

    /// Load full content for a set of named skills.
    pub fn load_skills_for_context(&self, names: &[String]) -> String {
        let mut parts = Vec::new();
        for name in names {
            if let Some(skill) = self.load_skill(name) {
                let emoji = self
                    .get_zeptometa(&skill)
                    .emoji
                    .unwrap_or_else(|| "📚".to_string());
                parts.push(format!(
                    "### {} {} Skill\n\n{}",
                    emoji, skill.name, skill.content
                ));
            }
        }

        parts.join("\n\n---\n\n")
    }

    /// Return names of skills marked `always = true`.
    pub fn get_always_skills(&self) -> Vec<String> {
        self.list_skills(false)
            .into_iter()
            .filter_map(|info| self.load_skill(&info.name))
            .filter(|skill| self.get_zeptometa(skill).always)
            .map(|skill| skill.name)
            .collect()
    }

    /// Check if required binaries, env vars, and platform constraints are met.
    pub fn check_requirements(&self, skill: &Skill) -> bool {
        let meta = self.get_zeptometa(skill);

        // Platform filter: if `os` is non-empty, current OS must be listed.
        if !meta.os.is_empty() && !meta.os.iter().any(|o| o == current_os()) {
            return false;
        }

        // All listed bins must be present.
        for bin in &meta.requires.bins {
            if !binary_in_path(bin) {
                return false;
            }
        }

        // At least one of any_bins must be present (if non-empty).
        if !meta.requires.any_bins.is_empty()
            && !meta.requires.any_bins.iter().any(|b| binary_in_path(b))
        {
            return false;
        }

        for env_name in &meta.requires.env {
            if std::env::var(env_name).is_err() {
                return false;
            }
        }

        // All declared skill dependencies must exist in skill directories.
        for dep in &skill.metadata.depends {
            if self.load_skill(dep).is_none() {
                return false;
            }
        }

        true
    }

    fn parse_skill_file(&self, path: &Path, fallback_name: &str, source: &str) -> Option<Skill> {
        let raw = std::fs::read_to_string(path).ok()?;
        let (metadata, body) = self.parse_frontmatter(&raw);

        let name = if metadata.name.trim().is_empty() {
            fallback_name.to_string()
        } else {
            metadata.name.clone()
        };
        let description = if metadata.description.trim().is_empty() {
            format!("Skill '{}'", name)
        } else {
            metadata.description.clone()
        };

        // Replace {baseDir} with the skill's parent directory (OpenClaw compat).
        let base_dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let body = body.replace("{baseDir}", &base_dir);

        Some(Skill {
            name,
            description,
            path: path.to_string_lossy().to_string(),
            source: source.to_string(),
            metadata,
            content: body,
        })
    }

    fn parse_frontmatter(&self, content: &str) -> (SkillMetadata, String) {
        let re = Regex::new(r"(?s)^---\n(.*?)\n---\n?").ok();
        if let Some(re) = re {
            if let Some(captures) = re.captures(content) {
                if let (Some(frontmatter), Some(full)) = (captures.get(1), captures.get(0)) {
                    let metadata = parse_frontmatter_metadata(frontmatter.as_str());
                    let body = content[full.end()..].trim().to_string();
                    return (metadata, body);
                }
            }
        }

        (SkillMetadata::default(), content.to_string())
    }

    fn get_zeptometa(&self, skill: &Skill) -> ZeptoMetadata {
        skill
            .metadata
            .metadata
            .as_ref()
            .and_then(|value| {
                // Priority: zeptoclaw > clawdbot > openclaw > clawdis > raw object
                if let Some(scoped) = value.get("zeptoclaw") {
                    serde_json::from_value(scoped.clone()).ok()
                } else if let Some(scoped) = value.get("clawdbot") {
                    serde_json::from_value(scoped.clone()).ok()
                } else if let Some(scoped) = value.get("openclaw") {
                    serde_json::from_value(scoped.clone()).ok()
                } else if let Some(scoped) = value.get("clawdis") {
                    serde_json::from_value(scoped.clone()).ok()
                } else {
                    serde_json::from_value(value.clone()).ok()
                }
            })
            .unwrap_or_default()
    }
}

fn default_builtin_skills_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|p| p.join(BUILTIN_SKILLS_DIR)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(BUILTIN_SKILLS_DIR))
}

fn collect_skill_infos(
    dir: &Path,
    source: &str,
    output: &mut Vec<SkillInfo>,
    seen: &mut HashSet<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        if seen.contains(&name) {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }

        seen.insert(name.clone());
        output.push(SkillInfo {
            name,
            path: skill_file.to_string_lossy().to_string(),
            source: source.to_string(),
        });
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parse_frontmatter_metadata(frontmatter: &str) -> SkillMetadata {
    match serde_yaml::from_str::<SkillMetadata>(frontmatter) {
        Ok(meta) => meta,
        Err(e) => {
            tracing::warn!("Failed to parse skill frontmatter: {}", e);
            SkillMetadata::default()
        }
    }
}

/// Map `cfg!(target_os)` to OpenClaw's platform strings.
fn current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

fn binary_in_path(bin: &str) -> bool {
    if bin.trim().is_empty() {
        return false;
    }
    let path = match std::env::var_os("PATH") {
        Some(path) => path,
        None => return false,
    };

    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{}.exe", bin));
            if candidate.is_file() {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter() {
        let loader = SkillsLoader::with_defaults();
        let content = r#"---
name: weather
description: Weather helper
metadata: {"zeptoclaw":{"emoji":"🌤️","requires":{"bins":["curl"]}}}
---
# Weather

Use wttr.in.
"#;

        let (meta, body) = loader.parse_frontmatter(content);
        assert_eq!(meta.name, "weather");
        assert_eq!(meta.description, "Weather helper");
        assert!(body.contains("# Weather"));
    }

    #[test]
    fn test_parse_frontmatter_without_frontmatter() {
        let loader = SkillsLoader::with_defaults();
        let content = "# Just markdown";
        let (meta, body) = loader.parse_frontmatter(content);
        assert!(meta.name.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_workspace_overrides_builtin() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("workspace");
        let builtin = temp.path().join("builtin");
        std::fs::create_dir_all(ws.join("demo")).unwrap();
        std::fs::create_dir_all(builtin.join("demo")).unwrap();
        std::fs::write(
            ws.join("demo/SKILL.md"),
            "---\nname: demo\ndescription: workspace\n---\nworkspace",
        )
        .unwrap();
        std::fs::write(
            builtin.join("demo/SKILL.md"),
            "---\nname: demo\ndescription: builtin\n---\nbuiltin",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(builtin));
        let skill = loader.load_skill("demo").unwrap();
        assert_eq!(skill.source, "workspace");
        assert_eq!(skill.description, "workspace");
    }

    // --- OpenClaw compatibility tests ---

    #[test]
    fn test_openclaw_metadata_loads() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("github")).unwrap();
        std::fs::write(
            ws.join("github/SKILL.md"),
            "---\nname: github\ndescription: GitHub integration\nmetadata: {\"openclaw\":{\"emoji\":\"🐙\",\"requires\":{\"bins\":[\"gh\"]},\"always\":true}}\n---\n# GitHub\nUse gh CLI.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("github").unwrap();
        let meta = loader.get_zeptometa(&skill);
        assert_eq!(meta.emoji, Some("🐙".to_string()));
        assert!(meta.always);
        assert_eq!(meta.requires.bins, vec!["gh"]);
    }

    #[test]
    fn test_zeptoclaw_metadata_takes_priority_over_openclaw() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("dual")).unwrap();
        std::fs::write(
            ws.join("dual/SKILL.md"),
            "---\nname: dual\ndescription: Both namespaces\nmetadata: {\"zeptoclaw\":{\"emoji\":\"🦀\"},\"openclaw\":{\"emoji\":\"🦞\"}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("dual").unwrap();
        let meta = loader.get_zeptometa(&skill);
        // zeptoclaw takes priority
        assert_eq!(meta.emoji, Some("🦀".to_string()));
    }

    #[test]
    fn test_openclaw_unknown_fields_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("compat")).unwrap();
        // Include OpenClaw-only fields that ZeptoClaw doesn't have
        std::fs::write(
            ws.join("compat/SKILL.md"),
            "---\nname: compat\ndescription: With extra fields\nmetadata: {\"openclaw\":{\"emoji\":\"✅\",\"primaryEnv\":\"MY_API_KEY\",\"skillKey\":\"my-skill\",\"requires\":{\"bins\":[],\"config\":[\"some.config.path\"]}}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("compat").unwrap();
        let meta = loader.get_zeptometa(&skill);
        // Should load successfully despite unknown fields
        assert_eq!(meta.emoji, Some("✅".to_string()));
    }

    #[test]
    fn test_any_bins_satisfied() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("editor")).unwrap();
        // any_bins: at least one of these should be on PATH
        std::fs::write(
            ws.join("editor/SKILL.md"),
            "---\nname: editor\ndescription: Editor\nmetadata: {\"openclaw\":{\"requires\":{\"anyBins\":[\"vim\",\"nano\",\"nonexistent_xyz\"]}}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("editor").unwrap();
        let meta = loader.get_zeptometa(&skill);
        assert!(!meta.requires.any_bins.is_empty());
        // At least vim or nano should exist on most systems
    }

    #[test]
    fn test_any_bins_none_found() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("missing")).unwrap();
        std::fs::write(
            ws.join("missing/SKILL.md"),
            "---\nname: missing\ndescription: Missing bins\nmetadata: {\"zeptoclaw\":{\"requires\":{\"any_bins\":[\"zzz_nonexistent_1\",\"zzz_nonexistent_2\"]}}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("missing").unwrap();
        assert!(!loader.check_requirements(&skill));
    }

    #[test]
    fn test_os_filter_current_platform() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("platform")).unwrap();
        // Skill restricted to current platform — should pass
        std::fs::write(
            ws.join("platform/SKILL.md"),
            &format!(
                "---\nname: platform\ndescription: Platform-specific\nmetadata: {{\"openclaw\":{{\"os\":[\"{}\"]}}}}\n---\nBody.",
                current_os()
            ),
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("platform").unwrap();
        assert!(loader.check_requirements(&skill));
    }

    #[test]
    fn test_os_filter_wrong_platform() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("wrong_os")).unwrap();
        // Skill restricted to a platform we're NOT on
        let wrong_os = if cfg!(target_os = "macos") {
            "win32"
        } else {
            "darwin"
        };
        std::fs::write(
            ws.join("wrong_os/SKILL.md"),
            &format!(
                "---\nname: wrong_os\ndescription: Wrong platform\nmetadata: {{\"openclaw\":{{\"os\":[\"{}\"]}}}}\n---\nBody.",
                wrong_os
            ),
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("wrong_os").unwrap();
        assert!(!loader.check_requirements(&skill));
    }

    #[test]
    fn test_os_filter_empty_means_all_platforms() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("universal")).unwrap();
        std::fs::write(
            ws.join("universal/SKILL.md"),
            "---\nname: universal\ndescription: All platforms\nmetadata: {\"openclaw\":{\"os\":[]}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("universal").unwrap();
        assert!(loader.check_requirements(&skill));
    }

    #[test]
    fn test_openclaw_always_skills() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("auto")).unwrap();
        std::fs::write(
            ws.join("auto/SKILL.md"),
            "---\nname: auto\ndescription: Auto-inject\nmetadata: {\"openclaw\":{\"always\":true}}\n---\nAlways loaded.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let always = loader.get_always_skills();
        assert!(always.contains(&"auto".to_string()));
    }

    #[test]
    fn test_clawdbot_namespace_loads() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("venice")).unwrap();
        std::fs::write(
            ws.join("venice/SKILL.md"),
            "---\nname: venice\ndescription: Venice AI\nversion: 1.2.0\nmetadata: {\"clawdbot\":{\"emoji\":\"🎨\",\"requires\":{\"bins\":[\"python3\"]}}}\n---\n# Venice\nGenerate images.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("venice").unwrap();
        let meta = loader.get_zeptometa(&skill);
        assert_eq!(meta.emoji, Some("🎨".to_string()));
        assert_eq!(meta.requires.bins, vec!["python3"]);
        assert_eq!(skill.metadata.version, Some("1.2.0".to_string()));
    }

    #[test]
    fn test_clawdis_namespace_loads() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("legacy")).unwrap();
        std::fs::write(
            ws.join("legacy/SKILL.md"),
            "---\nname: legacy\ndescription: Legacy skill\nmetadata: {\"clawdis\":{\"emoji\":\"📦\",\"always\":true}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("legacy").unwrap();
        let meta = loader.get_zeptometa(&skill);
        assert_eq!(meta.emoji, Some("📦".to_string()));
        assert!(meta.always);
    }

    #[test]
    fn test_zeptoclaw_priority_over_clawdbot() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("prio")).unwrap();
        std::fs::write(
            ws.join("prio/SKILL.md"),
            "---\nname: prio\ndescription: Priority test\nmetadata: {\"zeptoclaw\":{\"emoji\":\"🦀\"},\"clawdbot\":{\"emoji\":\"🤖\"}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("prio").unwrap();
        let meta = loader.get_zeptometa(&skill);
        assert_eq!(meta.emoji, Some("🦀".to_string()));
    }

    #[test]
    fn test_clawdbot_priority_over_openclaw() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("nsorder")).unwrap();
        std::fs::write(
            ws.join("nsorder/SKILL.md"),
            "---\nname: nsorder\ndescription: Namespace order\nmetadata: {\"clawdbot\":{\"emoji\":\"🤖\"},\"openclaw\":{\"emoji\":\"🔓\"}}\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("nsorder").unwrap();
        let meta = loader.get_zeptometa(&skill);
        assert_eq!(meta.emoji, Some("🤖".to_string()));
    }

    #[test]
    fn test_basedir_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        let skill_dir = ws.join("scripted");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: scripted\ndescription: With scripts\n---\nRun `{baseDir}/scripts/run.sh` to execute.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("scripted").unwrap();
        assert!(skill
            .content
            .contains(&skill_dir.to_string_lossy().to_string()));
        assert!(!skill.content.contains("{baseDir}"));
    }

    #[test]
    fn test_version_field_parsed() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("versioned")).unwrap();
        std::fs::write(
            ws.join("versioned/SKILL.md"),
            "---\nname: versioned\ndescription: Versioned skill\nversion: 2.1.0\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("versioned").unwrap();
        assert_eq!(skill.metadata.version, Some("2.1.0".to_string()));
    }

    #[test]
    fn test_version_field_optional() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("noversion")).unwrap();
        std::fs::write(
            ws.join("noversion/SKILL.md"),
            "---\nname: noversion\ndescription: No version\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("noversion").unwrap();
        assert_eq!(skill.metadata.version, None);
    }

    #[test]
    fn test_current_os_returns_valid_value() {
        let os = current_os();
        assert!(
            os == "darwin" || os == "linux" || os == "win32",
            "unexpected os: {}",
            os
        );
    }

    #[test]
    fn test_parse_new_manifest_fields() {
        let loader = SkillsLoader::with_defaults();
        let content = "---\nname: sea-orders\ndescription: SEA order management\nversion: 1.0.0\nauthor: Kitakod Ventures\nlicense: MIT\ntags:\n  - messaging\n  - sea\ndepends:\n  - longterm-memory\nconflicts:\n  - orders-lite\nenv_needed:\n  - name: WHATSAPP_PHONE_NUMBER_ID\n    description: Your phone number ID\n    required: true\n  - name: WEBHOOK_TOKEN\n    description: Webhook verify token\n    required: false\nmetadata: {\"zeptoclaw\": {\"emoji\": \"\u{1F6D2}\"}}\n---\nBody.\n";
        let (meta, _) = loader.parse_frontmatter(content);
        assert_eq!(meta.author.as_deref(), Some("Kitakod Ventures"));
        assert_eq!(meta.license.as_deref(), Some("MIT"));
        assert_eq!(meta.tags, vec!["messaging", "sea"]);
        assert_eq!(meta.depends, vec!["longterm-memory"]);
        assert_eq!(meta.conflicts, vec!["orders-lite"]);
        assert_eq!(meta.env_needed.len(), 2);
        assert_eq!(meta.env_needed[0].name, "WHATSAPP_PHONE_NUMBER_ID");
        assert_eq!(meta.env_needed[0].description, "Your phone number ID");
        assert!(meta.env_needed[0].required);
        assert!(!meta.env_needed[1].required);
    }

    #[test]
    fn test_old_skills_load_without_new_fields() {
        let loader = SkillsLoader::with_defaults();
        let content = "---\nname: old\ndescription: Old skill\n---\nBody.";
        let (meta, _) = loader.parse_frontmatter(content);
        assert!(meta.tags.is_empty());
        assert!(meta.depends.is_empty());
        assert!(meta.author.is_none());
    }

    #[test]
    fn test_depends_missing_makes_skill_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("child")).unwrap();
        std::fs::write(
            ws.join("child/SKILL.md"),
            "---\nname: child\ndescription: Needs parent\ndepends:\n  - nonexistent-parent\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("child").unwrap();
        assert!(!loader.check_requirements(&skill));
    }

    #[test]
    fn test_depends_present_does_not_block() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("skills");
        std::fs::create_dir_all(ws.join("parent")).unwrap();
        std::fs::create_dir_all(ws.join("child")).unwrap();
        std::fs::write(
            ws.join("parent/SKILL.md"),
            "---\nname: parent\ndescription: Parent\n---\nBody.",
        )
        .unwrap();
        std::fs::write(
            ws.join("child/SKILL.md"),
            "---\nname: child\ndescription: Needs parent\ndepends:\n  - parent\n---\nBody.",
        )
        .unwrap();

        let loader = SkillsLoader::new(ws, Some(temp.path().join("empty")));
        let skill = loader.load_skill("child").unwrap();
        assert!(loader.check_requirements(&skill));
    }
}
