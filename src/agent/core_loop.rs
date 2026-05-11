//! Terminal executor for the agent middleware pipeline.
//!
//! This module hosts the [`Terminal`] implementations that sit at the end
//! of the [`Pipeline`](crate::agent::pipeline::Pipeline) chain. The
//! production target is a `CoreLoop` terminal that owns the LLM call +
//! tool-iteration logic currently inlined in [`AgentLoop::process_message`].
//!
//! # Phase status (issue #399)
//!
//! - **Phase 1 (#564, merged):** middleware framework + 11 middleware
//!   implementations + `Pipeline` composition primitives.
//! - **Phase 2 (this module):** wiring scaffolding — adds the
//!   construction surface on [`AgentLoop`] (build `Subsystems`,
//!   `PipelineContext`, and `Pipeline`) and a placeholder
//!   [`LegacyTerminal`] so the framework can be exercised end-to-end
//!   from unit tests. The legacy `process_message` body has **not** been
//!   migrated yet.
//! - **Phase 2b (follow-up):** extract the post-prelude body of
//!   `process_message` into a real `CoreLoop` terminal that closes the
//!   parity gap against the inline body (audit hash chain, tool loop,
//!   compaction recovery, streaming, safety/taint/metrics). Once parity
//!   is reached, `process_message` flips from the inline body to
//!   `pipeline.execute()`.
//! - **Phase 3 (follow-up):** build the pipeline once at `AgentLoop`
//!   construction time instead of per-message (pattern from
//!   commit `484dc13` in the original PR #404 stack).
//!
//! The placeholder terminal in this module short-circuits with a clear
//! error so any accidental production wiring during scaffolding is loud,
//! not silently broken.

use async_trait::async_trait;

use crate::agent::middleware::{PipelineContext, PipelineOutput};
use crate::agent::pipeline::Terminal;
use crate::error::{Result, ZeptoError};

/// Placeholder terminal used during Phase 2 scaffolding.
///
/// Returns a structured error rather than silently no-op'ing so that any
/// path that accidentally reaches the terminal during this phase is
/// immediately visible in tests and logs. The Phase 2b follow-up
/// replaces this with a real terminal that runs the LLM + tool loop.
#[derive(Debug, Default)]
pub struct LegacyTerminal;

impl LegacyTerminal {
    /// Create a new scaffolding terminal.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Terminal for LegacyTerminal {
    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<PipelineOutput> {
        Err(ZeptoError::Provider(
            "agent::core_loop::LegacyTerminal is a Phase 2 scaffolding stub; \
             the real LLM + tool-loop executor lands in Phase 2b of #399. \
             Production should continue calling AgentLoop::process_message()."
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::agent::pipeline::Pipeline;

    #[tokio::test]
    async fn legacy_terminal_returns_scaffolding_error() {
        let pipeline = Pipeline::builder().build(LegacyTerminal::new());
        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);

        let result = pipeline.execute(&mut ctx).await;
        let err = result.expect_err("LegacyTerminal must short-circuit during Phase 2");
        let msg = err.to_string();
        assert!(
            msg.contains("scaffolding stub"),
            "error should explain the Phase 2 scaffolding gap, got: {msg}"
        );
    }
}
