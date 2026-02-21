//! `install_skill` tool — download and install a ClawHub skill by slug.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;
use crate::skills::registry::ClawHubRegistry;
use crate::tools::{Tool, ToolContext, ToolOutput};

/// Agent tool that installs a skill from ClawHub by slug.
pub struct InstallSkillTool {
    registry: Arc<ClawHubRegistry>,
    skills_dir: String,
}

impl InstallSkillTool {
    /// Create a new tool that installs skills into `skills_dir`.
    pub fn new(registry: Arc<ClawHubRegistry>, skills_dir: impl Into<String>) -> Self {
        Self {
            registry,
            skills_dir: skills_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for InstallSkillTool {
    fn name(&self) -> &str {
        "install_skill"
    }

    fn description(&self) -> &str {
        "Install a skill from ClawHub by slug. Use find_skills first to discover available slugs. \
         The skill will be available after restarting the agent."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "slug": {
                    "type": "string",
                    "description": "Skill slug from find_skills results (e.g. 'web-scraper')"
                }
            },
            "required": ["slug"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let slug = match args["slug"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(ToolOutput::error("slug is required")),
        };

        // Validate the slug early so the error message is clear to the caller
        // before any network or filesystem operations are attempted.
        if slug.is_empty()
            || !slug
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Ok(ToolOutput::error(format!(
                "Invalid skill slug '{}': only alphanumeric characters, hyphens, and underscores are allowed",
                slug
            )));
        }

        match self
            .registry
            .download_and_install(slug, &self.skills_dir)
            .await
        {
            Ok(path) => Ok(ToolOutput::user_visible(format!(
                "Skill '{}' installed to {}. Restart the agent to use it.",
                slug, path
            ))),
            Err(e) => Ok(ToolOutput::error(format!("Install failed: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::skills::registry::{ClawHubRegistry, SearchCache};

    fn make_tool() -> InstallSkillTool {
        let cache = Arc::new(SearchCache::new(10, Duration::from_secs(60)));
        let registry = Arc::new(ClawHubRegistry::new("https://clawhub.ai", None, cache));
        InstallSkillTool::new(registry, "/tmp/skills")
    }

    #[test]
    fn test_install_skill_tool_name() {
        assert_eq!(make_tool().name(), "install_skill");
    }

    #[test]
    fn test_install_skill_tool_description_nonempty() {
        assert!(!make_tool().description().is_empty());
    }

    #[test]
    fn test_install_skill_tool_parameters() {
        let params = make_tool().parameters();
        assert!(params["properties"]["slug"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("slug")));
    }

    #[tokio::test]
    async fn test_install_missing_slug_returns_error() {
        let tool = make_tool();
        let ctx = ToolContext::new();
        let result = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_install_empty_slug_returns_error() {
        let tool = make_tool();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"slug": ""}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_install_skill_tool_skills_dir_stored() {
        let tool = make_tool();
        assert_eq!(tool.skills_dir, "/tmp/skills");
    }

    // -------------------------------------------------------------------------
    // Fix 2: slug validation in the tool layer
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_install_path_traversal_slug_returns_error() {
        let tool = make_tool();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"slug": "../etc/passwd"}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(
            result.for_llm.contains("Invalid skill slug"),
            "expected validation error message, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_install_slug_with_slash_returns_error() {
        let tool = make_tool();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"slug": "foo/bar"}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.for_llm.contains("Invalid skill slug"));
    }

    #[tokio::test]
    async fn test_install_slug_with_space_returns_error() {
        let tool = make_tool();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"slug": "web scraper"}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.for_llm.contains("Invalid skill slug"));
    }
}
