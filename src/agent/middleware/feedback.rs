//! Middleware for tool execution feedback events.
//!
//! Emits `ToolFeedback::Thinking` before the inner pipeline runs and
//! `ToolFeedback::ResponseReady` after it completes.  These events
//! drive the CLI shimmer animation and other UI indicators.

use async_trait::async_trait;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::agent::ToolFeedback;
use crate::error::Result;

/// Emits UI feedback events around the pipeline execution.
///
/// Before `next.run()`: sends `Thinking`
/// After `next.run()`:  sends `ResponseReady`
///
/// These events are only sent when a feedback channel is configured
/// (i.e. `ctx.subsystems.tool_feedback_tx` contains a sender).
#[derive(Debug)]
pub struct FeedbackMiddleware;

impl FeedbackMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FeedbackMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for FeedbackMiddleware {
    fn name(&self) -> &'static str {
        "feedback"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // Send "Thinking" feedback before the pipeline runs.
        send_feedback(
            &ctx.subsystems.tool_feedback_tx,
            ToolFeedback {
                tool_name: String::new(),
                phase: crate::agent::ToolFeedbackPhase::Thinking,
                args_json: None,
            },
        )
        .await;

        let result = next.run(ctx).await;

        // On success, send "ResponseReady" so the UI can stop the shimmer
        // and prepare to display the response.
        if result.is_ok() {
            send_feedback(
                &ctx.subsystems.tool_feedback_tx,
                ToolFeedback {
                    tool_name: String::new(),
                    phase: crate::agent::ToolFeedbackPhase::ResponseReady,
                    args_json: None,
                },
            )
            .await;
        }

        result
    }
}

/// Send a feedback event if a channel is configured.  Silently ignores
/// send failures (e.g. receiver dropped).
async fn send_feedback(
    tx: &tokio::sync::RwLock<Option<tokio::sync::mpsc::UnboundedSender<ToolFeedback>>>,
    event: ToolFeedback,
) {
    if let Some(sender) = tx.read().await.as_ref() {
        let _ = sender.send(event);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::agent::ToolFeedbackPhase;
    use std::sync::Arc;
    use tokio::sync::{mpsc, RwLock};

    fn subsystems_with_feedback() -> (
        Arc<super::super::Subsystems>,
        mpsc::UnboundedReceiver<ToolFeedback>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut inner = test_subsystems_inner();
        inner.tool_feedback_tx = Arc::new(RwLock::new(Some(tx)));
        (Arc::new(inner), rx)
    }

    #[tokio::test]
    async fn sends_thinking_and_response_ready() {
        let mw = FeedbackMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let (subsystems, mut rx) = subsystems_with_feedback();
        let mut ctx = test_context(subsystems);

        let _ = pipeline.execute(&mut ctx).await.unwrap();

        // Should have received Thinking first
        let first = rx.recv().await.expect("should receive Thinking");
        assert!(
            matches!(first.phase, ToolFeedbackPhase::Thinking),
            "Expected Thinking, got {:?}",
            first.phase
        );

        // Then ResponseReady
        let second = rx.recv().await.expect("should receive ResponseReady");
        assert!(
            matches!(second.phase, ToolFeedbackPhase::ResponseReady),
            "Expected ResponseReady, got {:?}",
            second.phase
        );
    }

    #[tokio::test]
    async fn no_feedback_channel_does_not_panic() {
        let mw = FeedbackMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems(); // no feedback tx
        let mut ctx = test_context(subsystems);

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("ok"));
    }

    #[tokio::test]
    async fn error_does_not_send_response_ready() {
        let mw = FeedbackMiddleware::new();
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(ErrorTerminal);

        let (subsystems, mut rx) = subsystems_with_feedback();
        let mut ctx = test_context(subsystems);

        let result = pipeline.execute(&mut ctx).await;
        assert!(result.is_err());

        // Should have Thinking but NOT ResponseReady
        let first = rx.recv().await.expect("should receive Thinking");
        assert!(matches!(first.phase, ToolFeedbackPhase::Thinking));

        // Channel should be empty (no ResponseReady on error)
        assert!(rx.try_recv().is_err());
    }

    /// Terminal that always errors.
    #[derive(Debug)]
    struct ErrorTerminal;

    #[async_trait]
    impl crate::agent::pipeline::Terminal for ErrorTerminal {
        async fn execute(
            &self,
            _ctx: &mut PipelineContext,
        ) -> crate::error::Result<PipelineOutput> {
            Err(crate::error::ZeptoError::Provider("test error".into()))
        }
    }
}
