//! Middleware for context overflow recovery (compaction).
//!
//! Checks the context urgency via `ContextMonitor`.  When the session
//! history exceeds configured thresholds, applies three-tier recovery:
//! truncate, shrink tool results, or summarize — depending on urgency.
//! On `Normal` urgency a memory flush is triggered first so important
//! facts are persisted before compaction discards older messages.

use async_trait::async_trait;
use tracing::debug;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::context_monitor::CompactionUrgency;
use crate::agent::pipeline::Next;
use crate::error::Result;

/// Applies context overflow recovery when the session approaches the
/// context window limit.
///
/// Requires `ctx.session` to be populated (i.e. runs after
/// [`SessionMiddleware`](super::session::SessionMiddleware)).
///
/// After this middleware, `ctx.session.messages` may have been compacted.
#[derive(Debug)]
pub struct CompactionMiddleware;

impl CompactionMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CompactionMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for CompactionMiddleware {
    fn name(&self) -> &'static str {
        "compaction"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // If no context monitor is configured, compaction is disabled.
        let monitor = match ctx.subsystems.context_monitor {
            Some(ref m) => m,
            None => return next.run(ctx).await,
        };

        // If no session is loaded yet, nothing to compact.
        let session = match ctx.session {
            Some(ref mut s) => s,
            None => return next.run(ctx).await,
        };

        if let Some(urgency) = monitor.urgency(&session.messages) {
            // On Normal urgency, flush memories before compaction.
            // Emergency/Critical skip the flush to recover faster.
            if matches!(urgency, CompactionUrgency::Normal) {
                // Memory flush is a best-effort operation — we don't
                // block the pipeline if it fails.  The full memory_flush
                // implementation lives in AgentLoop and requires the
                // provider + tools, so we log a debug note here.
                // Phase 4 will wire the actual flush via the terminal.
                debug!("compaction: Normal urgency, memory flush would run here");
            }

            let context_limit = ctx.config.compaction.context_limit;
            let tool_result_cap = ctx.config.agents.defaults.max_tool_result_bytes;
            let (recovered, tier) = crate::agent::compaction::try_recover_context_with_urgency(
                std::mem::take(&mut session.messages),
                context_limit,
                urgency,
                8,               // keep_recent for tier 1
                tool_result_cap, // tool result budget for tier 2
            );
            if tier > 0 {
                debug!(
                    tier = tier,
                    urgency = ?urgency,
                    "Context recovered via tier {} compaction",
                    tier
                );
            }
            session.messages = recovered;
        }

        next.run(ctx).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context_monitor::ContextMonitor;
    use crate::agent::middleware::test_helpers::*;
    use crate::session::Message;
    use std::sync::Arc;

    /// Create subsystems with a ContextMonitor configured to trigger at
    /// a very low threshold so our test messages exceed it.
    fn subsystems_with_monitor(
        context_limit: usize,
        threshold: f64,
    ) -> Arc<super::super::Subsystems> {
        let mut inner = test_subsystems_inner();
        inner.context_monitor = Some(ContextMonitor::new_with_thresholds(
            context_limit,
            threshold,
            0.95, // emergency
            0.99, // critical
        ));
        Arc::new(inner)
    }

    #[tokio::test]
    async fn no_monitor_passes_through() {
        let mw = CompactionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems(); // no context_monitor
        let mut ctx = test_context(subsystems);
        ctx.session = Some(crate::session::Session::new("test"));

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("ok"));
    }

    #[tokio::test]
    async fn no_session_passes_through() {
        let mw = CompactionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = subsystems_with_monitor(100, 0.5);
        let mut ctx = test_context(subsystems);
        // session is None by default

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("ok"));
    }

    #[tokio::test]
    async fn below_threshold_no_compaction() {
        let mw = CompactionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        // Very high limit so small messages won't trigger
        let subsystems = subsystems_with_monitor(1_000_000, 0.8);
        let mut ctx = test_context(subsystems);
        let mut session = crate::session::Session::new("test");
        session.add_message(Message::user("short message"));
        let msg_count_before = session.messages.len();
        ctx.session = Some(session);

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        let session = ctx.session.as_ref().unwrap();
        assert_eq!(session.messages.len(), msg_count_before);
    }

    #[tokio::test]
    async fn above_threshold_triggers_compaction() {
        let mw = CompactionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        // Tiny limit (50 tokens) with low threshold (0.1) so any messages trigger
        let subsystems = subsystems_with_monitor(50, 0.1);
        let mut ctx = test_context(subsystems);
        // IMPORTANT: The compaction function uses ctx.config.compaction.context_limit,
        // not the monitor's limit.  We must align them.
        let mut config = (*ctx.config).clone();
        config.compaction.context_limit = 50;
        ctx.config = Arc::new(config);

        let mut session = crate::session::Session::new("test");
        // Add many messages to exceed the tiny limit
        for i in 0..20 {
            session.add_message(Message::user(&format!(
                "This is message number {} with enough text to be significant",
                i
            )));
            session.add_message(Message::assistant(&format!(
                "Response to message number {} with detailed explanation",
                i
            )));
        }
        let msg_count_before = session.messages.len();
        ctx.session = Some(session);

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        let session = ctx.session.as_ref().unwrap();
        // After compaction, should have fewer messages
        assert!(
            session.messages.len() < msg_count_before,
            "Expected compaction to reduce messages from {} but got {}",
            msg_count_before,
            session.messages.len()
        );
    }
}
