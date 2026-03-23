//! Pipeline composition and execution for the middleware chain.

use std::fmt::Debug;
use std::sync::Arc;

use crate::error::Result;
use crate::session::types::ToolCall;
use crate::tools::ToolOutput;

use super::middleware::{
    Middleware, PipelineContext, PipelineOutput, ToolExecutionContext, ToolMiddleware,
};

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// An ordered middleware chain with a terminal executor.
pub struct Pipeline {
    middlewares: Vec<Arc<dyn Middleware>>,
    terminal: Arc<dyn Terminal>,
}

/// The terminal executor at the end of the middleware chain.
///
/// In production this will be `CoreLoop` (Phase 4a). During testing,
/// use [`test_helpers::MockTerminal`].
#[async_trait::async_trait]
pub trait Terminal: Send + Sync + Debug {
    async fn execute(&self, ctx: &mut PipelineContext) -> Result<PipelineOutput>;
}

impl Pipeline {
    pub fn builder() -> PipelineBuilder {
        PipelineBuilder {
            middlewares: Vec::new(),
        }
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<PipelineOutput> {
        let next = Next {
            chain: &self.middlewares,
            terminal: &*self.terminal,
        };
        next.run(ctx).await
    }

    pub fn len(&self) -> usize {
        self.middlewares.len()
    }

    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }
}

// ---------------------------------------------------------------------------
// PipelineBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing a [`Pipeline`] with ordered middlewares.
pub struct PipelineBuilder {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl PipelineBuilder {
    /// Add a middleware. First added = outermost wrapper.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, middleware: impl Middleware + 'static) -> Self {
        self.middlewares.push(Arc::new(middleware));
        self
    }

    /// Conditionally add a middleware.
    pub fn add_if(self, condition: bool, middleware: impl Middleware + 'static) -> Self {
        if condition {
            self.add(middleware)
        } else {
            self
        }
    }

    /// Build the pipeline with the given terminal executor.
    pub fn build(self, terminal: impl Terminal + 'static) -> Pipeline {
        Pipeline {
            middlewares: self.middlewares,
            terminal: Arc::new(terminal),
        }
    }
}

// ---------------------------------------------------------------------------
// Next (pipeline-level continuation)
// ---------------------------------------------------------------------------

/// Continuation handle for pipeline-level middlewares.
pub struct Next<'a> {
    chain: &'a [Arc<dyn Middleware>],
    terminal: &'a dyn Terminal,
}

impl<'a> Next<'a> {
    /// Execute the next middleware, or the terminal if none remain.
    pub async fn run(self, ctx: &mut PipelineContext) -> Result<PipelineOutput> {
        if let Some((head, tail)) = self.chain.split_first() {
            let next = Next {
                chain: tail,
                terminal: self.terminal,
            };
            head.handle(ctx, next).await
        } else {
            self.terminal.execute(ctx).await
        }
    }
}

// ---------------------------------------------------------------------------
// ToolNext (tool-level continuation)
// ---------------------------------------------------------------------------

/// Terminal executor for tool-level middleware chains.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync + Debug {
    async fn execute_tool(&self, call: &ToolCall, ctx: &ToolExecutionContext)
        -> Result<ToolOutput>;
}

/// Continuation handle for tool-level middlewares.
pub struct ToolNext<'a> {
    chain: &'a [Arc<dyn ToolMiddleware>],
    executor: &'a dyn ToolExecutor,
}

impl<'a> ToolNext<'a> {
    pub fn new(chain: &'a [Arc<dyn ToolMiddleware>], executor: &'a dyn ToolExecutor) -> Self {
        Self { chain, executor }
    }

    pub async fn run(self, call: &ToolCall, ctx: &ToolExecutionContext) -> Result<ToolOutput> {
        if let Some((head, tail)) = self.chain.split_first() {
            let next = ToolNext {
                chain: tail,
                executor: self.executor,
            };
            head.handle_tool(call, ctx, next).await
        } else {
            self.executor.execute_tool(call, ctx).await
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::tools::ToolContext;

    #[tokio::test]
    async fn empty_pipeline_calls_terminal() {
        let terminal = MockTerminal::with_response("hello from terminal");
        let pipeline = Pipeline::builder().build(terminal);
        assert!(pipeline.is_empty());

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "hello from terminal"),
            _ => panic!("expected Sync output"),
        }
    }

    #[tokio::test]
    async fn single_middleware_wraps_terminal() {
        let terminal = MockTerminal::with_response("terminal");
        let pipeline = Pipeline::builder()
            .add(PassthroughMiddleware)
            .build(terminal);
        assert_eq!(pipeline.len(), 1);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "terminal"),
            _ => panic!("expected Sync output"),
        }
    }

    #[tokio::test]
    async fn middleware_can_short_circuit() {
        let terminal = MockTerminal::panicking();
        let pipeline = Pipeline::builder()
            .add(ShortCircuitMiddleware::with_response("short-circuited"))
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "short-circuited"),
            _ => panic!("expected Sync output"),
        }
    }

    #[tokio::test]
    async fn middleware_ordering_is_outermost_first() {
        let terminal = MockTerminal::with_response("T");
        let pipeline = Pipeline::builder()
            .add(PrefixMiddleware("A:"))
            .add(PrefixMiddleware("B:"))
            .build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "A:B:T"),
            _ => panic!("expected Sync output"),
        }
    }

    #[tokio::test]
    async fn add_if_true_includes_middleware() {
        let terminal = MockTerminal::with_response("T");
        let pipeline = Pipeline::builder()
            .add_if(true, PrefixMiddleware("Y:"))
            .build(terminal);
        assert_eq!(pipeline.len(), 1);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "Y:T"),
            _ => panic!("expected Sync"),
        }
    }

    #[tokio::test]
    async fn add_if_false_excludes_middleware() {
        let terminal = MockTerminal::with_response("T");
        let pipeline = Pipeline::builder()
            .add_if(false, PrefixMiddleware("N:"))
            .build(terminal);
        assert_eq!(pipeline.len(), 0);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "T"),
            _ => panic!("expected Sync"),
        }
    }

    #[tokio::test]
    async fn middleware_error_propagates() {
        let terminal = MockTerminal::with_response("should not reach");
        let pipeline = Pipeline::builder().add(ErrorMiddleware).build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let result = pipeline.execute(&mut ctx).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("middleware error"));
    }

    #[tokio::test]
    async fn wrap_middleware_runs_before_and_after() {
        let terminal = MockTerminal::with_response("inner");
        let pipeline = Pipeline::builder().add(WrapMiddleware).build(terminal);

        let subsystems = test_subsystems();
        let mut ctx = test_context(subsystems);
        let output = pipeline.execute(&mut ctx).await.unwrap();

        match output {
            PipelineOutput::Sync { response, .. } => assert_eq!(response, "inner"),
            _ => panic!("expected Sync"),
        }
        assert_eq!(ctx.session_key, "wrapped");
    }

    #[tokio::test]
    async fn tool_next_empty_chain_calls_executor() {
        let executor = MockToolExecutor;
        let chain: Vec<Arc<dyn ToolMiddleware>> = vec![];
        let next = ToolNext::new(&chain, &executor);

        let call = ToolCall {
            id: "1".into(),
            name: "test_tool".into(),
            arguments: "{}".into(),
        };
        let subsystems = test_subsystems();
        let tctx = ToolExecutionContext {
            tool_context: ToolContext::new(),
            subsystems,
            dry_run: false,
            tool_result_budget: 50_000,
        };

        let output = next.run(&call, &tctx).await.unwrap();
        assert_eq!(output.for_llm, "executed: test_tool");
    }
}
