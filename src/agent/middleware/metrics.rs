//! Middleware for pipeline execution timing and error metrics.
//!
//! Wraps the inner pipeline: records wall-clock time of the full
//! pipeline execution and logs errors.  Uses the shared
//! `MetricsCollector` from subsystems.

use async_trait::async_trait;
use tracing::error;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::error::Result;

/// Records execution timing and error counts around the inner pipeline.
///
/// This is a "wrap" middleware: it runs code before *and* after
/// `next.run(ctx)`.  It should be placed near the outermost position
/// so that its timing covers most of the pipeline.
#[derive(Debug)]
pub struct MetricsMiddleware;

impl MetricsMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MetricsMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for MetricsMiddleware {
    fn name(&self) -> &'static str {
        "metrics"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        let start = std::time::Instant::now();

        let result = next.run(ctx).await;

        let elapsed = start.elapsed();
        let latency_ms = elapsed.as_millis() as u64;

        match &result {
            Ok(output) => {
                // Record token usage if available in the output.
                if let Some(usage) = output.usage() {
                    ctx.subsystems
                        .metrics_collector
                        .record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
                }
                tracing::debug!(
                    latency_ms = latency_ms,
                    cached = output.is_cached(),
                    "Pipeline execution completed"
                );
            }
            Err(e) => {
                error!(
                    latency_ms = latency_ms,
                    error = %e,
                    "Pipeline execution failed"
                );
                // Record error in usage metrics if available.
                let usage_metrics = ctx.subsystems.usage_metrics.read().await;
                usage_metrics.record_error();
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::error::ZeptoError;

    #[tokio::test]
    async fn records_timing_on_success() {
        let mw = MetricsMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("ok"));
    }

    #[tokio::test]
    async fn records_timing_on_error() {
        let mw = MetricsMiddleware::new();
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(ErrorTerminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let result = pipeline.execute(&mut ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn passes_through_response() {
        let mw = MetricsMiddleware::new();
        let terminal = MockTerminal::with_response("passthrough");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("passthrough"));
    }

    /// Terminal that always errors, for testing error-path metrics.
    #[derive(Debug)]
    struct ErrorTerminal;

    #[async_trait]
    impl crate::agent::pipeline::Terminal for ErrorTerminal {
        async fn execute(
            &self,
            _ctx: &mut PipelineContext,
        ) -> crate::error::Result<PipelineOutput> {
            Err(ZeptoError::Provider("test error".into()))
        }
    }
}
