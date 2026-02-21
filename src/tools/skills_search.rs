//! `find_skills` tool — search the ClawHub marketplace for skills.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;
use crate::skills::registry::ClawHubRegistry;
use crate::tools::{Tool, ToolContext, ToolOutput};

/// Agent tool that searches the ClawHub skills marketplace.
pub struct FindSkillsTool {
    registry: Arc<ClawHubRegistry>,
}

impl FindSkillsTool {
    /// Create a new tool backed by `registry`.
    pub fn new(registry: Arc<ClawHubRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for FindSkillsTool {
    fn name(&self) -> &str {
        "find_skills"
    }

    fn description(&self) -> &str {
        "Search the ClawHub skills marketplace for skills matching a query. \
         Returns a ranked list of skills with slugs, descriptions, and metadata."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (e.g. 'web scraping', 'data analysis')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return (default 10)",
                    "default": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let query = match args["query"].as_str() {
            Some(q) if !q.is_empty() => q,
            _ => return Ok(ToolOutput::error("query is required")),
        };
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        match self.registry.search(query, limit).await {
            Ok(results) if results.is_empty() => {
                Ok(ToolOutput::llm_only("No skills found matching that query."))
            }
            Ok(results) => {
                let mut out = format!("Found {} skill(s):\n\n", results.len());
                for r in &results {
                    let warning = if r.is_suspicious {
                        " WARNING: SUSPICIOUS"
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "- **{}** (`{}`){}\n  Version: {}\n  {}\n\n",
                        r.display_name, r.slug, warning, r.version, r.summary
                    ));
                }
                Ok(ToolOutput::user_visible(out))
            }
            Err(e) => Ok(ToolOutput::error(format!("Skill search failed: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::skills::registry::{ClawHubRegistry, SearchCache};

    fn make_registry() -> Arc<ClawHubRegistry> {
        let cache = Arc::new(SearchCache::new(10, Duration::from_secs(60)));
        Arc::new(ClawHubRegistry::new("https://clawhub.ai", None, cache))
    }

    #[test]
    fn test_find_skills_tool_name() {
        let tool = FindSkillsTool::new(make_registry());
        assert_eq!(tool.name(), "find_skills");
    }

    #[test]
    fn test_find_skills_tool_description_nonempty() {
        let tool = FindSkillsTool::new(make_registry());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_find_skills_tool_parameters() {
        let tool = FindSkillsTool::new(make_registry());
        let params = tool.parameters();
        assert!(params["properties"]["query"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("query")));
    }

    #[tokio::test]
    async fn test_find_skills_empty_query_returns_error() {
        let tool = FindSkillsTool::new(make_registry());
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"query": ""}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_find_skills_missing_query_returns_error() {
        let tool = FindSkillsTool::new(make_registry());
        let ctx = ToolContext::new();
        let result = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_find_skills_tool_parameters_limit_has_default() {
        let tool = FindSkillsTool::new(make_registry());
        let params = tool.parameters();
        assert_eq!(params["properties"]["limit"]["default"], 10);
    }
}
