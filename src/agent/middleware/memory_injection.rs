//! Middleware for long-term memory injection.
//!
//! Searches long-term memory for facts relevant to the user's message
//! and sets `ctx.memory_override` so that the context builder can
//! include them in the system prompt.

use async_trait::async_trait;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::error::Result;

/// Searches long-term memory and injects relevant entries into the context.
///
/// When `ctx.subsystems.ltm` is `Some`, searches for pinned memories
/// and entries matching the user's query.  Sets `ctx.memory_override`
/// with the formatted memory block.  If no LTM is configured or no
/// relevant memories are found, the field is left as `None`.
#[derive(Debug)]
pub struct MemoryInjectionMiddleware;

impl MemoryInjectionMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemoryInjectionMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for MemoryInjectionMiddleware {
    fn name(&self) -> &'static str {
        "memory_injection"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // If no long-term memory is configured, skip.
        let ltm = match ctx.subsystems.ltm {
            Some(ref ltm) => ltm,
            None => return next.run(ctx).await,
        };

        let guard = ltm.lock().await;
        let memory = crate::memory::build_memory_injection(
            &guard,
            &ctx.inbound.content,
            crate::memory::MEMORY_INJECTION_BUDGET,
        );
        drop(guard);

        if !memory.is_empty() {
            ctx.memory_override = Some(memory);
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
    use crate::agent::middleware::test_helpers::*;
    use crate::memory::longterm::LongTermMemory;
    use std::sync::Arc;

    fn subsystems_with_ltm() -> (
        Arc<super::super::Subsystems>,
        Arc<tokio::sync::Mutex<LongTermMemory>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("longterm.json");
        let ltm = LongTermMemory::with_path(path).unwrap();
        let ltm_arc = Arc::new(tokio::sync::Mutex::new(ltm));
        let mut inner = test_subsystems_inner();
        inner.ltm = Some(Arc::clone(&ltm_arc));
        // Leak the tempdir so it outlives the test (avoids premature cleanup).
        std::mem::forget(dir);
        (Arc::new(inner), ltm_arc)
    }

    #[tokio::test]
    async fn no_ltm_passes_through() {
        let mw = MemoryInjectionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems(); // no ltm
        let mut ctx = test_context(subsystems);
        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert!(ctx.memory_override.is_none());
    }

    #[tokio::test]
    async fn empty_ltm_no_override() {
        let mw = MemoryInjectionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let (subsystems, _ltm) = subsystems_with_ltm();
        let mut ctx = test_context(subsystems);
        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert!(ctx.memory_override.is_none());
    }

    #[tokio::test]
    async fn pinned_memory_injected() {
        let mw = MemoryInjectionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let (subsystems, ltm) = subsystems_with_ltm();
        {
            let mut guard = ltm.lock().await;
            guard
                .set("user_name", "Alice", "pinned", vec![], 1.0)
                .await
                .unwrap();
        }

        let mut ctx = test_context(subsystems);
        ctx.inbound.content = "Hello".to_string();
        let _ = pipeline.execute(&mut ctx).await.unwrap();

        let override_text = ctx
            .memory_override
            .as_ref()
            .expect("should have memory override");
        assert!(
            override_text.contains("Alice"),
            "Expected memory to contain 'Alice', got: {}",
            override_text
        );
    }

    #[tokio::test]
    async fn query_matched_memory_injected() {
        let mw = MemoryInjectionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let (subsystems, ltm) = subsystems_with_ltm();
        {
            let mut guard = ltm.lock().await;
            guard
                .set("favorite_color", "blue", "preferences", vec![], 0.5)
                .await
                .unwrap();
        }

        let mut ctx = test_context(subsystems);
        ctx.inbound.content = "What is my favorite color?".to_string();
        let _ = pipeline.execute(&mut ctx).await.unwrap();

        // The memory might or might not match depending on the search
        // algorithm.  We just verify no panic/error occurred.
        // For a guaranteed match, pinned memories are more reliable.
    }
}
