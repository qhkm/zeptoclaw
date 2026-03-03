//! Security-gated tool execution for ZeptoKernel.
//!
//! `execute_tool()` wraps core execution (safety check + lookup + run + metrics).
//! The agent loop's per-session gates (hooks, approval, dry-run, streaming feedback)
//! stay in `agent/loop.rs` as a wrapper around this.

use serde_json::Value;
use std::time::Instant;

use crate::error::Result;
use crate::safety::SafetyLayer;
use crate::tools::{ToolContext, ToolOutput, ToolRegistry};
use crate::utils::metrics::MetricsCollector;

/// Execute a tool with security gates applied.
///
/// Pipeline:
/// 1. Safety check on input (when safety enabled)
/// 2. Tool lookup + execute
/// 3. Safety check on output (when safety enabled)
/// 4. Metrics recording
///
/// This is the core execution path. Per-session gates (hooks, approval,
/// dry-run) are handled by the agent loop wrapper.
pub async fn execute_tool(
    registry: &ToolRegistry,
    name: &str,
    input: Value,
    ctx: &ToolContext,
    safety: Option<&SafetyLayer>,
    metrics: &MetricsCollector,
) -> Result<ToolOutput> {
    let start = Instant::now();

    // Step 1: Safety check on input
    if let Some(safety_layer) = safety {
        let input_str = serde_json::to_string(&input).unwrap_or_default();
        let result = safety_layer.check_tool_output(&input_str);
        if result.blocked {
            metrics.record_tool_call(name, start.elapsed(), false);
            return Ok(ToolOutput::error(format!(
                "Tool '{}' input blocked by safety: {}",
                name,
                result.warnings.join("; ")
            )));
        }
    }

    // Step 2: Execute
    let output = registry.execute_with_context(name, input, ctx).await?;

    // Step 3: Safety check on output
    if let Some(safety_layer) = safety {
        let result = safety_layer.check_tool_output(&output.for_llm);
        if result.blocked {
            metrics.record_tool_call(name, start.elapsed(), false);
            return Ok(ToolOutput::error(format!(
                "Tool '{}' output blocked by safety: {}",
                name,
                result.warnings.join("; ")
            )));
        }
    }

    // Step 4: Record metrics
    metrics.record_tool_call(name, start.elapsed(), !output.is_error);

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::{SafetyConfig, SafetyLayer};
    use crate::tools::{EchoTool, ToolRegistry};
    use crate::utils::metrics::MetricsCollector;
    use serde_json::json;

    fn setup_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry
    }

    #[tokio::test]
    async fn test_execute_tool_basic() {
        let registry = setup_registry();
        let metrics = MetricsCollector::new();
        let ctx = ToolContext::default();

        let result = execute_tool(
            &registry,
            "echo",
            json!({"message": "hello"}),
            &ctx,
            None,
            &metrics,
        )
        .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.for_llm, "hello");
    }

    #[tokio::test]
    async fn test_execute_tool_not_found() {
        let registry = setup_registry();
        let metrics = MetricsCollector::new();
        let ctx = ToolContext::default();

        let result = execute_tool(&registry, "nonexistent", json!({}), &ctx, None, &metrics).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.is_error);
        assert!(output.for_llm.contains("Tool not found"));
    }

    #[tokio::test]
    async fn test_execute_tool_records_metrics() {
        let registry = setup_registry();
        let metrics = MetricsCollector::new();
        let ctx = ToolContext::default();

        let _ = execute_tool(
            &registry,
            "echo",
            json!({"message": "hi"}),
            &ctx,
            None,
            &metrics,
        )
        .await;

        assert_eq!(metrics.total_tool_calls(), 1);
    }

    #[tokio::test]
    async fn test_execute_tool_with_safety_passes_clean_input() {
        let registry = setup_registry();
        let metrics = MetricsCollector::new();
        let ctx = ToolContext::default();
        let safety = SafetyLayer::new(SafetyConfig::default());

        let result = execute_tool(
            &registry,
            "echo",
            json!({"message": "hello world"}),
            &ctx,
            Some(&safety),
            &metrics,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "hello world");
    }

    #[tokio::test]
    async fn test_execute_tool_without_safety_skips_checks() {
        let registry = setup_registry();
        let metrics = MetricsCollector::new();
        let ctx = ToolContext::default();

        // None safety → no checks applied
        let result = execute_tool(
            &registry,
            "echo",
            json!({"message": "anything goes"}),
            &ctx,
            None,
            &metrics,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().for_llm, "anything goes");
    }

    #[tokio::test]
    async fn test_execute_tool_metrics_even_on_not_found() {
        let registry = setup_registry();
        let metrics = MetricsCollector::new();
        let ctx = ToolContext::default();

        let _ = execute_tool(&registry, "missing", json!({}), &ctx, None, &metrics).await;

        // Metrics should still be recorded for missing tools
        // (the tool lookup happens inside registry, which returns Ok with error output)
        assert_eq!(metrics.total_tool_calls(), 1);
    }
}
