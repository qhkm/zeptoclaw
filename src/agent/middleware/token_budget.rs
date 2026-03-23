//! Middleware for per-run token budget and tool-call-limit resets.
//!
//! At the start of each pipeline execution this middleware resets the
//! per-run counters (token budget and tool call limit) so that limits
//! apply independently per `process_message` call rather than accumulating
//! across the lifetime of the `AgentLoop`.  If the budget is already
//! exceeded after reset (i.e. limit is 0) the pipeline short-circuits.

use async_trait::async_trait;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::error::{Result, ZeptoError};

/// Resets per-run counters and guards the token budget.
///
/// Inserted early in the pipeline (after injection scan) so that every
/// run starts from a clean counter state. If the configured budget is
/// zero (unlimited) the check is a no-op.
#[derive(Debug)]
pub struct TokenBudgetMiddleware;

impl TokenBudgetMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TokenBudgetMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for TokenBudgetMiddleware {
    fn name(&self) -> &'static str {
        "token_budget"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // Reset per-run counters so limits apply per-invocation.
        ctx.subsystems.token_budget.reset();
        ctx.subsystems.tool_call_limit.reset();

        // Short-circuit if budget is already exceeded (e.g. limit == 0 with
        // an immediate record, though practically this only matters when a
        // custom budget has been configured and is already depleted).
        if ctx.subsystems.token_budget.is_exceeded() {
            return Err(ZeptoError::Provider(format!(
                "Token budget exceeded: {}",
                ctx.subsystems.token_budget.summary()
            )));
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
    use crate::agent::budget::TokenBudget;
    use crate::agent::middleware::test_helpers::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn unlimited_budget_passes_through() {
        let mw = TokenBudgetMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        // Default test_subsystems uses budget limit 0 => unlimited
        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("ok"));
    }

    #[tokio::test]
    async fn budget_with_headroom_passes_through() {
        let mw = TokenBudgetMiddleware::new();
        let terminal = MockTerminal::with_response("within-budget");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut subsystems = test_subsystems_inner();
        // Set a generous budget (1000 tokens, none used after reset)
        subsystems.token_budget = Arc::new(TokenBudget::new(1000));
        let subsystems = Arc::new(subsystems);
        let mut ctx = test_context(subsystems);

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("within-budget"));
    }

    #[tokio::test]
    async fn resets_are_called() {
        // Record some usage, then verify the middleware resets it.
        let budget = Arc::new(TokenBudget::new(1000));
        budget.record(500, 0);
        assert_eq!(budget.total_used(), 500);

        let mw = TokenBudgetMiddleware::new();
        let terminal = MockTerminal::with_response("reset-ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut subsystems = test_subsystems_inner();
        subsystems.token_budget = Arc::clone(&budget);
        let subsystems = Arc::new(subsystems);
        let mut ctx = test_context(subsystems);

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("reset-ok"));
        // After the middleware ran, budget should be reset to 0
        assert_eq!(budget.total_used(), 0);
    }

    #[tokio::test]
    async fn zero_limit_budget_is_unlimited() {
        // TokenBudget with limit 0 is "unlimited" — should never short-circuit.
        let mw = TokenBudgetMiddleware::new();
        let terminal = MockTerminal::with_response("unlimited-ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut subsystems = test_subsystems_inner();
        subsystems.token_budget = Arc::new(TokenBudget::new(0));
        let subsystems = Arc::new(subsystems);
        let mut ctx = test_context(subsystems);

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("unlimited-ok"));
    }
}
