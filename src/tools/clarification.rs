//! Ask clarification tool — pauses agent execution to ask the user a question.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Tool that pauses agent execution to ask the user for clarification.
pub struct AskClarificationTool;

#[async_trait]
impl Tool for AskClarificationTool {
    fn name(&self) -> &str {
        "ask_clarification"
    }

    fn description(&self) -> &str {
        "Ask the user for clarification before proceeding with an ambiguous or risky action"
    }

    fn compact_description(&self) -> &str {
        "Ask user for clarification"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "clarification_type": {
                    "type": "string",
                    "enum": ["missing_info", "ambiguous_requirement", "approach_choice", "risk_confirmation", "suggestion"],
                    "description": "The type of clarification needed"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Numbered options for the user to choose from"
                },
                "context": {
                    "type": "string",
                    "description": "Brief context for why clarification is needed"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // Batch mode fallback — no interactive user
        if ctx.is_batch {
            return Ok(ToolOutput::llm_only(
                "Unable to clarify in batch mode. Proceeding with best judgment based on available information."
            ));
        }

        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::ZeptoError::Tool("Missing required field: question".into())
            })?;

        let context = args.get("context").and_then(|v| v.as_str());
        let options: Vec<&str> = args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        // Build formatted user-facing message
        let mut user_msg = String::new();

        if let Some(ctx_text) = context {
            user_msg.push_str(ctx_text);
            user_msg.push_str("\n\n");
        }

        user_msg.push_str(question);

        if !options.is_empty() {
            user_msg.push('\n');
            for (i, opt) in options.iter().enumerate() {
                user_msg.push_str(&format!("\n{}. {}", i + 1, opt));
            }
        }

        Ok(ToolOutput::split(
            "Clarification requested. Waiting for user response before proceeding.",
            user_msg,
        )
        .with_pause())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new()
    }

    fn batch_ctx() -> ToolContext {
        ToolContext::new().with_batch(true)
    }

    #[test]
    fn test_name() {
        assert_eq!(AskClarificationTool.name(), "ask_clarification");
    }

    #[test]
    fn test_compact_description() {
        assert_eq!(
            AskClarificationTool.compact_description(),
            "Ask user for clarification"
        );
    }

    #[test]
    fn test_category_memory() {
        assert_eq!(AskClarificationTool.category(), ToolCategory::Memory);
    }

    #[test]
    fn test_parameters_schema() {
        let params = AskClarificationTool.parameters();
        let props = params.get("properties").unwrap();
        assert!(props.get("question").is_some());
        assert!(props.get("clarification_type").is_some());
        assert!(props.get("options").is_some());
        assert!(props.get("context").is_some());

        let required = params.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "question");
    }

    #[tokio::test]
    async fn test_execute_simple_question() {
        let args = json!({"question": "What format do you want?"});
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        assert_eq!(
            out.for_llm,
            "Clarification requested. Waiting for user response before proceeding."
        );
        assert_eq!(out.for_user.as_deref(), Some("What format do you want?"));
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_execute_with_context() {
        let args = json!({
            "question": "Which approach?",
            "context": "There are two ways to implement this."
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        let user = out.for_user.unwrap();
        assert!(user.starts_with("There are two ways to implement this."));
        assert!(user.contains("Which approach?"));
    }

    #[tokio::test]
    async fn test_execute_with_options() {
        let args = json!({
            "question": "Which database?",
            "options": ["PostgreSQL", "SQLite", "MongoDB"]
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        let user = out.for_user.unwrap();
        assert!(user.contains("1. PostgreSQL"));
        assert!(user.contains("2. SQLite"));
        assert!(user.contains("3. MongoDB"));
    }

    #[tokio::test]
    async fn test_execute_with_type() {
        let args = json!({
            "question": "Should I delete the file?",
            "clarification_type": "risk_confirmation"
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        assert!(out.pause_for_input);
        assert_eq!(out.for_user.as_deref(), Some("Should I delete the file?"));
    }

    #[tokio::test]
    async fn test_execute_full() {
        let args = json!({
            "question": "How should I proceed?",
            "clarification_type": "approach_choice",
            "context": "The module can be refactored two ways.",
            "options": ["Full rewrite", "Incremental patch"]
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        let user = out.for_user.unwrap();
        assert!(user.starts_with("The module can be refactored two ways."));
        assert!(user.contains("How should I proceed?"));
        assert!(user.contains("1. Full rewrite"));
        assert!(user.contains("2. Incremental patch"));
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_pause_flag_set() {
        let args = json!({"question": "test"});
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_missing_question() {
        let args = json!({"context": "some context"});
        let result = AskClarificationTool.execute(args, &ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_type_graceful() {
        let args = json!({
            "question": "What?",
            "clarification_type": "nonexistent_type"
        });
        // Should not error — clarification_type is informational, not validated
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_empty_options_array() {
        let args = json!({
            "question": "What?",
            "options": []
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        let user = out.for_user.unwrap();
        // Empty options should not produce numbered list
        assert!(!user.contains("1."));
        assert_eq!(user, "What?");
    }

    #[tokio::test]
    async fn test_batch_mode_fallback() {
        let args = json!({"question": "What format?"});
        let out = AskClarificationTool
            .execute(args, &batch_ctx())
            .await
            .unwrap();

        assert!(!out.pause_for_input);
        assert!(out.for_llm.contains("batch mode"));
        assert!(out.for_user.is_none());
    }
}
