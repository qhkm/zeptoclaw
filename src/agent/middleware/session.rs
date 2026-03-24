//! Middleware for session loading and creation.
//!
//! Loads or creates a session from the session manager based on the
//! inbound message's session key.  Populates `ctx.session` and ensures
//! `ctx.session_key` is set.

use async_trait::async_trait;

use super::{Middleware, PipelineContext, PipelineOutput};
use crate::agent::pipeline::Next;
use crate::error::Result;

/// Loads or creates the session for the current message.
///
/// After this middleware:
/// - `ctx.session` is `Some(session)` with the loaded/created session
/// - `ctx.session_key` matches `ctx.inbound.session_key`
///
/// The user message from the inbound content is also appended to the
/// session so that all downstream middleware (context build, compaction)
/// see it in the history.
#[derive(Debug)]
pub struct SessionMiddleware;

impl SessionMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SessionMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for SessionMiddleware {
    fn name(&self) -> &'static str {
        "session"
    }

    async fn handle(&self, ctx: &mut PipelineContext, next: Next<'_>) -> Result<PipelineOutput> {
        // Ensure session_key is populated from the inbound message.
        ctx.session_key.clone_from(&ctx.inbound.session_key);

        // Load or create the session.
        let mut session = ctx
            .subsystems
            .session_manager
            .get_or_create(&ctx.session_key)
            .await?;

        // Append the current user message to the session so downstream
        // middleware sees it in the history.
        // TODO: Wire image media handling (inbound_to_message equivalent)
        // for multimodal support through the pipeline.
        let user_message = crate::session::Message::user(&ctx.inbound.content);
        session.add_message(user_message);

        ctx.session = Some(session);
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

    #[tokio::test]
    async fn sets_session_and_key() {
        let mw = SessionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        ctx.inbound.session_key = "sess-42".to_string();

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(ctx.session_key, "sess-42");
        assert!(ctx.session.is_some());
    }

    #[tokio::test]
    async fn user_message_appended_to_session() {
        let mw = SessionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        ctx.inbound.content = "Hello there".to_string();

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        let session = ctx.session.as_ref().unwrap();
        assert!(
            session
                .messages
                .iter()
                .any(|m| m.content.contains("Hello there")),
            "User message should be in session history"
        );
    }

    #[tokio::test]
    async fn session_key_from_inbound() {
        let mw = SessionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        ctx.session_key = "stale-key".to_string();
        ctx.inbound.session_key = "fresh-key".to_string();

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(ctx.session_key, "fresh-key");
    }

    #[tokio::test]
    async fn second_call_loads_existing_session() {
        let subsystems = test_subsystems();

        // First call: create session
        let mw = SessionMiddleware::new();
        let terminal = MockTerminal::with_response("ok");
        let pipeline = crate::agent::pipeline::Pipeline::builder()
            .add(mw)
            .build(terminal);

        let mut ctx = test_context(Arc::clone(&subsystems));
        ctx.inbound.session_key = "persist-key".to_string();
        ctx.inbound.content = "first message".to_string();
        let _ = pipeline.execute(&mut ctx).await.unwrap();

        // Save it so the second call can find it
        if let Some(ref session) = ctx.session {
            subsystems.session_manager.save(session).await.unwrap();
        }

        // Second call: should load the existing session with the first message
        let mw2 = SessionMiddleware::new();
        let terminal2 = MockTerminal::with_response("ok2");
        let pipeline2 = crate::agent::pipeline::Pipeline::builder()
            .add(mw2)
            .build(terminal2);

        let mut ctx2 = test_context(Arc::clone(&subsystems));
        ctx2.inbound.session_key = "persist-key".to_string();
        ctx2.inbound.content = "second message".to_string();
        let _ = pipeline2.execute(&mut ctx2).await.unwrap();

        let session = ctx2.session.as_ref().unwrap();
        // Should have both the first and second user messages
        assert!(session.messages.len() >= 2);
    }

    use std::sync::Arc;
}
