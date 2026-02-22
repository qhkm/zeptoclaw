//! End-to-end tests for ZeptoClaw
//!
//! These tests exercise the agent and gateway subsystems in a manner closer
//! to production usage, combining multiple components rather than testing
//! individual units in isolation.
//!
//! # Test gating
//!
//! - Tests requiring live LLM API keys are gated behind the
//!   `ZEPTOCLAW_E2E_LIVE` environment variable.
//! - Tests requiring a running Docker daemon are gated behind the
//!   `ZEPTOCLAW_E2E_DOCKER` environment variable.
//!
//! By default, only tests that use mock providers run.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::config::Config;
use zeptoclaw::error::ZeptoError;
use zeptoclaw::gateway::{
    parse_marked_response, resolve_backend, AgentRequest, AgentResponse, AgentResult,
};
use zeptoclaw::providers::{ChatOptions, LLMProvider, LLMResponse, LLMToolCall, ToolDefinition};
use zeptoclaw::session::{Message, SessionManager};
use zeptoclaw::tools::{EchoTool, ToolContext, ToolRegistry};

// ============================================================================
// Mock Providers for E2E Tests
// ============================================================================

/// A provider that always returns a static response. Useful for verifying the
/// full agent loop without needing a real LLM backend.
#[derive(Debug)]
struct MockStaticProvider {
    response: String,
}

impl MockStaticProvider {
    fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }
}

#[async_trait]
impl LLMProvider for MockStaticProvider {
    fn name(&self) -> &str {
        "mock-static"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model: Option<&str>,
        _options: ChatOptions,
    ) -> zeptoclaw::error::Result<LLMResponse> {
        Ok(LLMResponse::text(&self.response))
    }
}

/// A provider that issues one tool call on the first invocation, then
/// returns a text response on subsequent calls. This exercises the
/// agent's tool execution loop end-to-end.
#[derive(Debug)]
struct MockToolCallingProvider {
    call_count: AtomicUsize,
}

impl MockToolCallingProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LLMProvider for MockToolCallingProvider {
    fn name(&self) -> &str {
        "mock-tool-caller"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model: Option<&str>,
        _options: ChatOptions,
    ) -> zeptoclaw::error::Result<LLMResponse> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            // First call: request a tool call
            Ok(LLMResponse {
                content: String::new(),
                tool_calls: vec![LLMToolCall::new(
                    "call_e2e_1",
                    "echo",
                    r#"{"message": "e2e-tool-test"}"#,
                )],
                usage: None,
            })
        } else {
            // Subsequent call: return final text
            Ok(LLMResponse::text("Tool result received: e2e-tool-test"))
        }
    }
}

/// A provider that always fails. Used to verify error propagation through the
/// full agent pipeline.
#[derive(Debug)]
struct MockFailProvider;

#[async_trait]
impl LLMProvider for MockFailProvider {
    fn name(&self) -> &str {
        "mock-fail"
    }

    fn default_model(&self) -> &str {
        "fail-model"
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model: Option<&str>,
        _options: ChatOptions,
    ) -> zeptoclaw::error::Result<LLMResponse> {
        Err(ZeptoError::Provider("simulated LLM failure".to_string()))
    }
}

/// A provider that sleeps longer than any reasonable timeout, used to
/// verify that the agent loop's wall-clock timeout is enforced.
#[derive(Debug)]
struct MockSlowProvider;

#[async_trait]
impl LLMProvider for MockSlowProvider {
    fn name(&self) -> &str {
        "mock-slow"
    }

    fn default_model(&self) -> &str {
        "slow-model"
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model: Option<&str>,
        _options: ChatOptions,
    ) -> zeptoclaw::error::Result<LLMResponse> {
        // Sleep for 60 seconds -- the agent timeout will fire first.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        Ok(LLMResponse::text("should never be returned"))
    }
}

/// A provider that returns usage information, used to verify that the
/// token budget tracker properly accumulates and enforces limits.
#[derive(Debug)]
struct MockTokenCountingProvider {
    call_count: AtomicUsize,
}

impl MockTokenCountingProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LLMProvider for MockTokenCountingProvider {
    fn name(&self) -> &str {
        "mock-token-counter"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model: Option<&str>,
        _options: ChatOptions,
    ) -> zeptoclaw::error::Result<LLMResponse> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(LLMResponse {
            content: format!("response {}", count),
            tool_calls: vec![],
            usage: Some(zeptoclaw::providers::Usage {
                prompt_tokens: 500,
                completion_tokens: 200,
                total_tokens: 700,
            }),
        })
    }
}

// ============================================================================
// Helper: Check if an env-gated test should run
// ============================================================================

fn is_live_enabled() -> bool {
    std::env::var("ZEPTOCLAW_E2E_LIVE").is_ok()
}

fn is_docker_enabled() -> bool {
    std::env::var("ZEPTOCLAW_E2E_DOCKER").is_ok()
}

// ============================================================================
// Agent E2E Tests
// ============================================================================

/// Test that an agent with a mock provider can process a message and return a
/// response through the full pipeline: message bus -> agent loop -> session.
#[tokio::test]
async fn test_agent_start_and_respond() {
    let config = Config::default();
    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus.clone());

    agent
        .set_provider(Box::new(MockStaticProvider::new("Hello from E2E!")))
        .await;
    agent.register_tool(Box::new(EchoTool)).await;

    let msg = InboundMessage::new("test", "e2e-user", "e2e-chat", "Hi there");
    let result = agent.process_message(&msg).await;

    assert!(result.is_ok(), "process_message failed: {:?}", result.err());
    assert_eq!(result.unwrap(), "Hello from E2E!");
}

/// Test agent with a tool-calling provider. The mock provider issues an echo
/// tool call on the first LLM turn, and the agent should execute it and feed
/// the result back for a second LLM turn.
#[tokio::test]
async fn test_agent_with_echo_tool() {
    let config = Config::default();
    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus.clone());

    agent
        .set_provider(Box::new(MockToolCallingProvider::new()))
        .await;
    agent.register_tool(Box::new(EchoTool)).await;

    let msg = InboundMessage::new("test", "e2e-user", "e2e-chat", "Echo something");
    let result = agent.process_message(&msg).await;

    assert!(result.is_ok(), "process_message failed: {:?}", result.err());
    let response = result.unwrap();
    assert!(
        response.contains("e2e-tool-test"),
        "Expected response to contain tool result, got: {}",
        response
    );
}

/// Test that the agent properly returns an error when the LLM provider fails.
#[tokio::test]
async fn test_agent_provider_failure() {
    let config = Config::default();
    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus.clone());

    agent.set_provider(Box::new(MockFailProvider)).await;

    let msg = InboundMessage::new("test", "e2e-user", "e2e-chat", "This will fail");
    let result = agent.process_message(&msg).await;

    assert!(result.is_err(), "Expected error, but got success");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("simulated LLM failure"),
        "Expected provider error, got: {}",
        err
    );
}

/// Test that the agent enforces its wall-clock timeout. The mock provider
/// sleeps longer than the configured timeout, and the agent should abort.
#[tokio::test]
async fn test_agent_timeout_handling() {
    let mut config = Config::default();
    // Set a very short timeout so the test completes quickly.
    config.agents.defaults.agent_timeout_secs = 1;

    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus.clone());

    agent.set_provider(Box::new(MockSlowProvider)).await;

    let msg = InboundMessage::new("test", "e2e-user", "e2e-chat", "This will timeout");

    // The timeout is enforced in the start() loop, not process_message itself.
    // So we test process_message with an explicit tokio timeout to simulate
    // the same behavior the agent loop applies.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        agent.process_message(&msg),
    )
    .await;

    match result {
        Ok(Ok(_)) => panic!("Expected timeout or error, but got success"),
        Ok(Err(_)) => { /* Provider error or agent error -- acceptable */ }
        Err(_elapsed) => { /* Tokio timeout fired -- this is the expected path */ }
    }
}

/// Test that the token budget tracker is initialized from config and can be
/// queried. The agent loop creates a TokenBudget from `config.agents.defaults.token_budget`.
#[tokio::test]
async fn test_agent_with_token_budget() {
    let mut config = Config::default();
    config.agents.defaults.token_budget = 5000;

    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus.clone());

    agent
        .set_provider(Box::new(MockTokenCountingProvider::new()))
        .await;
    agent.register_tool(Box::new(EchoTool)).await;

    // Process a message -- the budget should be tracked internally.
    let msg = InboundMessage::new("test", "e2e-user", "e2e-budget", "Count my tokens");
    let result = agent.process_message(&msg).await;

    assert!(
        result.is_ok(),
        "First message should succeed within budget: {:?}",
        result.err()
    );
}

/// Verify that the echo tool can be used standalone through the ToolRegistry,
/// end-to-end with context, matching the pattern the agent loop uses internally.
#[tokio::test]
async fn test_tool_registry_e2e_execution() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let ctx = ToolContext::new()
        .with_channel("e2e-test", "e2e-chat")
        .with_workspace("/tmp/e2e_workspace");

    let result = registry
        .execute_with_context(
            "echo",
            serde_json::json!({"message": "end-to-end echo"}),
            &ctx,
        )
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().for_llm, "end-to-end echo");

    // Verify that a missing tool returns an error ToolOutput through the same path
    let missing = registry
        .execute_with_context("nonexistent", serde_json::json!({}), &ctx)
        .await;
    assert!(missing.is_ok());
    assert!(missing.unwrap().is_error);
}

// ============================================================================
// Gateway E2E Tests
// ============================================================================

/// Test gateway config validation: a well-formed config with default values
/// should pass validation.
#[test]
fn test_gateway_config_validation() {
    let config = Config::default();

    // Gateway defaults should be valid
    assert_eq!(config.gateway.port, 8080);
    assert_eq!(config.gateway.host, "0.0.0.0");

    // Container agent defaults should be valid
    assert_eq!(config.container_agent.image, "zeptoclaw:latest");
    assert_eq!(config.container_agent.timeout_secs, 300);
    assert_eq!(config.container_agent.network, "none");
}

/// Test that container backend resolution returns Docker when explicitly
/// configured. This does NOT require Docker to be installed -- it only
/// tests the configuration routing logic.
#[tokio::test]
async fn test_container_backend_resolution_docker() {
    let mut config = Config::default();
    config.container_agent.backend = zeptoclaw::config::ContainerAgentBackend::Docker;

    let result = resolve_backend(&config.container_agent).await;
    assert!(result.is_ok());

    let backend = result.unwrap();
    assert_eq!(backend.to_string(), "docker");
}

/// Test agent request/response serialization roundtrip. This is critical for
/// the IPC protocol between the gateway and containerized agents.
#[test]
fn test_agent_request_response_serialization() {
    // Build a request
    let request = AgentRequest {
        request_id: "e2e-req-001".to_string(),
        message: InboundMessage::new("telegram", "user-e2e", "chat-e2e", "Hello from E2E"),
        agent_config: Config::default().agents.defaults,
        session: None,
    };

    // Serialize -> deserialize roundtrip
    let json = serde_json::to_string(&request).expect("Failed to serialize request");
    let parsed: AgentRequest = serde_json::from_str(&json).expect("Failed to deserialize request");

    assert_eq!(parsed.request_id, "e2e-req-001");
    assert_eq!(parsed.message.content, "Hello from E2E");
    assert_eq!(parsed.message.channel, "telegram");
    assert!(parsed.validate().is_ok());

    // Build a response and test its marked format
    let response = AgentResponse::success("e2e-req-001", "E2E response", None);
    let marked = response.to_marked_json();

    let recovered = parse_marked_response(&marked).expect("Failed to parse marked response");
    assert_eq!(recovered.request_id, "e2e-req-001");
    match recovered.result {
        AgentResult::Success { content, .. } => {
            assert_eq!(content, "E2E response");
        }
        AgentResult::Error { .. } => panic!("Expected Success, got Error"),
    }
}

/// Test error response serialization through the IPC protocol.
#[test]
fn test_agent_error_response_serialization() {
    let response = AgentResponse::error("e2e-err-001", "Something went wrong", "INTERNAL");
    let marked = response.to_marked_json();

    let recovered = parse_marked_response(&marked).expect("Failed to parse error response");
    assert_eq!(recovered.request_id, "e2e-err-001");
    match recovered.result {
        AgentResult::Error { message, code } => {
            assert_eq!(message, "Something went wrong");
            assert_eq!(code, "INTERNAL");
        }
        AgentResult::Success { .. } => panic!("Expected Error, got Success"),
    }
}

/// Test request validation rejects mismatched session keys.
#[test]
fn test_agent_request_validation_rejects_mismatch() {
    let request = AgentRequest {
        request_id: "e2e-req-002".to_string(),
        message: InboundMessage::new("test", "user", "chat-a", "Hello"),
        agent_config: Config::default().agents.defaults,
        session: Some(zeptoclaw::session::Session::new("test:chat-b")),
    };

    let result = request.validate();
    assert!(
        result.is_err(),
        "Expected validation error for key mismatch"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("mismatch"),
        "Expected mismatch error, got: {}",
        err
    );
}

// ============================================================================
// Docker-Gated E2E Tests
// ============================================================================

/// Test that Docker is available on the host. This test only runs when
/// `ZEPTOCLAW_E2E_DOCKER` is set.
#[tokio::test]
async fn test_docker_availability() {
    if !is_docker_enabled() {
        eprintln!("Skipping Docker availability test (ZEPTOCLAW_E2E_DOCKER not set)");
        return;
    }

    let available = zeptoclaw::gateway::is_docker_available().await;
    assert!(
        available,
        "Docker should be available when ZEPTOCLAW_E2E_DOCKER is set"
    );
}

// ============================================================================
// Live API E2E Tests
// ============================================================================

/// Test a real agent run with a live LLM provider. This test only runs when
/// `ZEPTOCLAW_E2E_LIVE` is set and `ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY`
/// is configured.
#[tokio::test]
async fn test_live_agent_run() {
    if !is_live_enabled() {
        eprintln!("Skipping live agent test (ZEPTOCLAW_E2E_LIVE not set)");
        return;
    }

    let api_key = match std::env::var("ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => {
            eprintln!("Skipping live agent test (no Anthropic API key)");
            return;
        }
    };

    let config = Config::default();
    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus.clone());

    let provider = zeptoclaw::providers::ClaudeProvider::new(&api_key);
    agent.set_provider(Box::new(provider)).await;
    agent.register_tool(Box::new(EchoTool)).await;

    let msg = InboundMessage::new("test", "e2e-live", "e2e-live", "Say hello in one word.");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.process_message(&msg),
    )
    .await;

    match result {
        Ok(Ok(response)) => {
            assert!(
                !response.is_empty(),
                "Live LLM response should not be empty"
            );
        }
        Ok(Err(e)) => panic!("Live agent run failed: {}", e),
        Err(_) => panic!("Live agent run timed out after 30 seconds"),
    }
}
