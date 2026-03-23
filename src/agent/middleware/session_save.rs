//! Middleware for persisting the session after successful pipeline execution.
//!
//! Wraps the inner pipeline: after a successful terminal path, saves the
//! session via the session manager.  On error, the session is NOT saved
//! so that failed runs don't corrupt the conversation state.

use async_trait::async_trait;
use tracing::{debug, warn};

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::error::Result;

/// Saves the session after successful pipeline completion.
///
/// This is the outermost wrap middleware: it runs `next.run(ctx)` and
/// then persists `ctx.session` only when the result is `Ok`.  If the
/// pipeline errors, the session is left unsaved.
///
/// Requires `ctx.session` to be populated by a prior middleware
/// (typically [`SessionMiddleware`](super::session::SessionMiddleware)).
#[derive(Debug)]
pub struct SessionSaveMiddleware;

impl SessionSaveMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SessionSaveMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for SessionSaveMiddleware {
    fn name(&self) -> &'static str {
        "session_save"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        let result = next.run(ctx).await;

        if result.is_ok() {
            if let Some(ref session) = ctx.session {
                match ctx.subsystems.session_manager.save(session).await {
                    Ok(()) => {
                        debug!(
                            session_key = %ctx.session_key,
                            "Session saved after successful pipeline execution"
                        );
                    }
                    Err(e) => {
                        warn!(
                            session_key = %ctx.session_key,
                            error = %e,
                            "Failed to save session after pipeline execution"
                        );
                        // We don't propagate save errors — the pipeline
                        // succeeded and the user should get their response.
                        // Session persistence failure is a degraded state,
                        // not a hard error.
                    }
                }
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
    use crate::session::Session;
    use std::sync::Arc;

    #[tokio::test]
    async fn saves_session_on_success() {
        let mw = SessionSaveMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(Arc::clone(&subsystems));
        let mut session = Session::new("save-test");
        session.add_message(crate::session::Message::user("hello"));
        ctx.session = Some(session);
        ctx.session_key = "save-test".to_string();

        let _ = pipeline.execute(&mut ctx).await.unwrap();

        // Verify session was saved by loading it back.
        let loaded = subsystems
            .session_manager
            .get_or_create("save-test")
            .await
            .unwrap();
        assert!(
            !loaded.messages.is_empty(),
            "Session should have been saved with messages"
        );
    }

    #[tokio::test]
    async fn does_not_save_on_error() {
        let mw = SessionSaveMiddleware::new();
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(ErrorTerminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(Arc::clone(&subsystems));
        let mut session = Session::new("no-save-test");
        session.add_message(crate::session::Message::user("should not persist"));
        ctx.session = Some(session);
        ctx.session_key = "no-save-test".to_string();

        let result = pipeline.execute(&mut ctx).await;
        assert!(result.is_err());

        // Verify session was NOT saved.
        let loaded = subsystems
            .session_manager
            .get_or_create("no-save-test")
            .await
            .unwrap();
        // A new session should be empty (no messages from the failed run).
        assert!(
            loaded.messages.is_empty(),
            "Session should not have been saved on error"
        );
    }

    #[tokio::test]
    async fn no_session_does_not_panic() {
        let mw = SessionSaveMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        // session is None

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("ok"));
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
