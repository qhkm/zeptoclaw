//! Integration tests for PicoClaw
//!
//! These tests verify that the various components work together correctly,
//! testing the full message flow, tool execution, session persistence, and
//! configuration handling.

use async_trait::async_trait;
use std::sync::Arc;
use tempfile::tempdir;
use zeptoclaw::error::ZeptoError;
use zeptoclaw::{
    bus::{InboundMessage, MessageBus, OutboundMessage},
    config::{Config, MemoryBackend, MemoryCitationsMode},
    heartbeat::{HeartbeatService, HEARTBEAT_PROMPT},
    providers::{ChatOptions, FallbackProvider, LLMProvider, LLMResponse, ToolDefinition},
    security::ShellSecurityConfig,
    session::{Message, SessionManager},
    skills::SkillsLoader,
    tools::filesystem::ReadFileTool,
    tools::shell::ShellTool,
    tools::{
        EchoTool, GoogleSheetsTool, MemoryGetTool, MemorySearchTool, MessageTool, Tool,
        ToolContext, ToolRegistry, WebFetchTool, WebSearchTool, WhatsAppTool,
    },
};

#[derive(Debug)]
struct AlwaysFailProvider;

#[derive(Debug)]
struct StaticSuccessProvider {
    provider_name: &'static str,
    response: &'static str,
}

#[async_trait]
impl LLMProvider for AlwaysFailProvider {
    fn name(&self) -> &str {
        "always-fail"
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
        Err(ZeptoError::Provider(
            "simulated primary failure".to_string(),
        ))
    }
}

#[async_trait]
impl LLMProvider for StaticSuccessProvider {
    fn name(&self) -> &str {
        self.provider_name
    }

    fn default_model(&self) -> &str {
        "success-model"
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model: Option<&str>,
        _options: ChatOptions,
    ) -> zeptoclaw::error::Result<LLMResponse> {
        Ok(LLMResponse::text(self.response))
    }
}

// ============================================================================
// Message Bus Integration Tests
// ============================================================================

#[tokio::test]
async fn test_message_flow() {
    let bus = Arc::new(MessageBus::new());
    let msg = InboundMessage::new("test", "user1", "chat1", "Hello");
    bus.publish_inbound(msg).await.unwrap();
    let received = bus.consume_inbound().await.unwrap();
    assert_eq!(received.content, "Hello");
    assert_eq!(received.channel, "test");
}

#[tokio::test]
async fn test_message_bus_roundtrip() {
    let bus = MessageBus::new();

    // Simulate a message from a channel
    let inbound = InboundMessage::new("telegram", "user123", "chat456", "Hello bot!");
    bus.publish_inbound(inbound).await.unwrap();

    // Agent receives the message
    let received = bus.consume_inbound().await.unwrap();
    assert_eq!(received.content, "Hello bot!");
    assert_eq!(received.session_key, "telegram:chat456");

    // Agent sends a response
    let response = OutboundMessage::reply_to(&received, "Hello human!");
    bus.publish_outbound(response).await.unwrap();

    // Channel receives the response
    let outgoing = bus.consume_outbound().await.unwrap();
    assert_eq!(outgoing.content, "Hello human!");
    assert_eq!(outgoing.channel, "telegram");
    assert_eq!(outgoing.chat_id, "chat456");
}

#[tokio::test]
async fn test_concurrent_message_producers() {
    let bus = Arc::new(MessageBus::new());
    let mut handles = vec![];

    // Spawn multiple producers (simulating multiple channels)
    for channel in ["telegram", "discord", "slack"] {
        let bus_clone = Arc::clone(&bus);
        let channel = channel.to_string();
        let handle = tokio::spawn(async move {
            for i in 0..5 {
                let msg =
                    InboundMessage::new(&channel, "user", "chat", &format!("{}:{}", channel, i));
                bus_clone.publish_inbound(msg).await.unwrap();
            }
        });
        handles.push(handle);
    }

    // Wait for all producers to finish
    for handle in handles {
        handle.await.unwrap();
    }

    // Consume all messages
    let mut count = 0;
    while count < 15 {
        if bus.consume_inbound().await.is_some() {
            count += 1;
        }
    }
    assert_eq!(count, 15);
}

// ============================================================================
// Tool Execution Integration Tests
// ============================================================================

#[tokio::test]
async fn test_tool_execution() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let result = registry
        .execute("echo", serde_json::json!({"message": "test"}))
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "test");
}

#[tokio::test]
async fn test_tool_registry_multiple_tools() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    // Verify tool is registered
    assert!(registry.has("echo"));
    assert_eq!(registry.len(), 1);

    // Execute the tool
    let result = registry
        .execute("echo", serde_json::json!({"message": "Hello, World!"}))
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello, World!");

    // Verify non-existent tool returns error
    let missing = registry.execute("nonexistent", serde_json::json!({})).await;
    assert!(missing.is_err());
}

#[tokio::test]
async fn test_tool_execution_with_context() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let ctx = ToolContext::new()
        .with_channel("telegram", "12345")
        .with_workspace("/tmp/test_workspace");

    let result = registry
        .execute_with_context("echo", serde_json::json!({"message": "Context test"}), &ctx)
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Context test");
}

#[tokio::test]
async fn test_tool_definitions_for_llm() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));

    let definitions = registry.definitions();
    assert_eq!(definitions.len(), 1);

    let echo_def = &definitions[0];
    assert_eq!(echo_def.name, "echo");
    assert!(!echo_def.description.is_empty());
    assert!(echo_def.parameters.is_object());
    assert!(echo_def.parameters["properties"]["message"].is_object());
}

// ============================================================================
// Session Persistence Integration Tests
// ============================================================================

#[tokio::test]
async fn test_session_persistence() {
    let manager = SessionManager::new_memory();
    let mut session = manager.get_or_create("test-key").await.unwrap();
    session.add_message(Message::user("Hello"));
    manager.save(&session).await.unwrap();
    let loaded = manager.get_or_create("test-key").await.unwrap();
    assert_eq!(loaded.messages.len(), 1);
}

#[tokio::test]
async fn test_session_full_conversation() {
    let manager = SessionManager::new_memory();

    // Simulate a full conversation
    let mut session = manager.get_or_create("telegram:chat123").await.unwrap();

    // Add system prompt
    session.add_message(Message::system("You are a helpful assistant."));

    // User asks a question
    session.add_message(Message::user("What is Rust?"));

    // Assistant responds
    session.add_message(Message::assistant(
        "Rust is a systems programming language focused on safety and performance.",
    ));

    // User follows up
    session.add_message(Message::user("What are its main features?"));

    // Save session
    manager.save(&session).await.unwrap();

    // Load and verify
    let loaded = manager.get_or_create("telegram:chat123").await.unwrap();
    assert_eq!(loaded.messages.len(), 4);
    assert_eq!(loaded.messages[0].role, zeptoclaw::session::Role::System);
    assert_eq!(loaded.messages[1].role, zeptoclaw::session::Role::User);
    assert_eq!(loaded.messages[2].role, zeptoclaw::session::Role::Assistant);
    assert_eq!(loaded.messages[3].role, zeptoclaw::session::Role::User);
}

#[tokio::test]
async fn test_session_with_tool_calls() {
    let manager = SessionManager::new_memory();
    let mut session = manager.get_or_create("tool-session").await.unwrap();

    // User message
    session.add_message(Message::user("Echo this: Hello World"));

    // Assistant with tool call
    let tool_call =
        zeptoclaw::session::ToolCall::new("call_1", "echo", r#"{"message": "Hello World"}"#);
    session.add_message(Message::assistant_with_tools(
        "Let me echo that.",
        vec![tool_call],
    ));

    // Tool result
    session.add_message(Message::tool_result("call_1", "Hello World"));

    // Final assistant response
    session.add_message(Message::assistant("I echoed your message: Hello World"));

    manager.save(&session).await.unwrap();

    // Verify
    let loaded = manager.get_or_create("tool-session").await.unwrap();
    assert_eq!(loaded.messages.len(), 4);
    assert!(loaded.messages[1].has_tool_calls());
    assert!(loaded.messages[2].is_tool_result());
}

#[tokio::test]
async fn test_session_isolation() {
    let manager = SessionManager::new_memory();

    // Create two separate sessions
    let mut session1 = manager.get_or_create("user1:chat1").await.unwrap();
    let mut session2 = manager.get_or_create("user2:chat2").await.unwrap();

    session1.add_message(Message::user("Message from user 1"));
    session2.add_message(Message::user("Message from user 2"));

    manager.save(&session1).await.unwrap();
    manager.save(&session2).await.unwrap();

    // Verify isolation
    let loaded1 = manager.get_or_create("user1:chat1").await.unwrap();
    let loaded2 = manager.get_or_create("user2:chat2").await.unwrap();

    assert_eq!(loaded1.messages.len(), 1);
    assert_eq!(loaded1.messages[0].content, "Message from user 1");

    assert_eq!(loaded2.messages.len(), 1);
    assert_eq!(loaded2.messages[0].content, "Message from user 2");
}

#[tokio::test]
async fn test_session_manager_concurrent_access() {
    let manager = Arc::new(SessionManager::new_memory());
    let mut handles = vec![];

    for i in 0..5 {
        let manager_clone = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let mut session = manager_clone
                .get_or_create(&format!("concurrent-{}", i))
                .await
                .unwrap();
            session.add_message(Message::user(&format!("Message {}", i)));
            manager_clone.save(&session).await.unwrap();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all sessions were created
    let keys = manager.list().await.unwrap();
    assert_eq!(keys.len(), 5);
}

// ============================================================================
// Configuration Integration Tests
// ============================================================================

#[test]
fn test_config_serialization() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();
    let parsed: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(config.agents.defaults.model, parsed.agents.defaults.model);
}

#[test]
fn test_config_partial_override() {
    // Test that partial JSON properly inherits defaults
    let json = r#"{"agents": {"defaults": {"model": "gpt-4"}}}"#;
    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.agents.defaults.model, "gpt-4");
    // Other defaults should still be set
    assert_eq!(config.agents.defaults.max_tokens, 8192);
    assert_eq!(config.agents.defaults.temperature, 0.7);
    assert_eq!(config.gateway.port, 8080);
}

#[test]
fn test_config_all_fields() {
    let json = r#"{
        "agents": {
            "defaults": {
                "model": "claude-sonnet-4-5-20250929",
                "max_tokens": 4096,
                "temperature": 0.5,
                "max_tool_iterations": 10,
                "workspace": "/custom/workspace"
            }
        },
        "gateway": {
            "host": "127.0.0.1",
            "port": 9000
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.agents.defaults.model, "claude-sonnet-4-5-20250929");
    assert_eq!(config.agents.defaults.max_tokens, 4096);
    assert_eq!(config.agents.defaults.temperature, 0.5);
    assert_eq!(config.agents.defaults.max_tool_iterations, 10);
    assert_eq!(config.agents.defaults.workspace, "/custom/workspace");
    assert_eq!(config.gateway.host, "127.0.0.1");
    assert_eq!(config.gateway.port, 9000);
}

#[test]
fn test_config_provider_settings() {
    let json = r#"{
        "providers": {
            "anthropic": {"api_key": "sk-ant-test"},
            "openai": {"api_key": "sk-test", "api_base": "https://api.openai.com/v1"}
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    let anthropic = config.providers.anthropic.as_ref().unwrap();
    assert_eq!(anthropic.api_key, Some("sk-ant-test".to_string()));

    let openai = config.providers.openai.as_ref().unwrap();
    assert_eq!(openai.api_key, Some("sk-test".to_string()));
    assert_eq!(
        openai.api_base,
        Some("https://api.openai.com/v1".to_string())
    );
}

#[test]
fn test_config_openai_provider() {
    let json = r#"{
        "providers": {
            "openai": {
                "api_key": "sk-test-key",
                "api_base": "https://custom.openai.com/v1"
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    let openai = config.providers.openai.as_ref().unwrap();
    assert_eq!(openai.api_key, Some("sk-test-key".to_string()));
    assert_eq!(
        openai.api_base,
        Some("https://custom.openai.com/v1".to_string())
    );
}

#[test]
fn test_config_openai_provider_minimal() {
    // Test OpenAI config with just API key (no custom base URL)
    let json = r#"{
        "providers": {
            "openai": {
                "api_key": "sk-minimal-key"
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    let openai = config.providers.openai.as_ref().unwrap();
    assert_eq!(openai.api_key, Some("sk-minimal-key".to_string()));
    assert!(openai.api_base.is_none());
}

#[test]
fn test_config_channel_settings() {
    let json = r#"{
        "channels": {
            "telegram": {
                "enabled": true,
                "token": "123456:ABC-DEF",
                "allow_from": ["user1", "user2"]
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    let telegram = config.channels.telegram.as_ref().unwrap();
    assert!(telegram.enabled);
    assert_eq!(telegram.token, "123456:ABC-DEF");
    assert_eq!(telegram.allow_from, vec!["user1", "user2"]);
}

#[test]
fn test_config_web_search_settings() {
    let json = r#"{
        "tools": {
            "web": {
                "search": {
                    "api_key": "test-brave-key",
                    "max_results": 7
                }
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(
        config.tools.web.search.api_key,
        Some("test-brave-key".to_string())
    );
    assert_eq!(config.tools.web.search.max_results, 7);
}

#[test]
fn test_config_memory_settings() {
    let json = r#"{
        "memory": {
            "backend": "qmd",
            "citations": "off",
            "include_default_memory": false,
            "max_results": 8,
            "min_score": 0.4,
            "max_snippet_chars": 512,
            "extra_paths": ["notes", "memory/archive"]
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.memory.backend, MemoryBackend::Qmd);
    assert_eq!(config.memory.citations, MemoryCitationsMode::Off);
    assert!(!config.memory.include_default_memory);
    assert_eq!(config.memory.max_results, 8);
    assert_eq!(config.memory.min_score, 0.4);
    assert_eq!(config.memory.max_snippet_chars, 512);
    assert_eq!(config.memory.extra_paths.len(), 2);
}

#[test]
fn test_config_heartbeat_settings() {
    let json = r#"{
        "heartbeat": {
            "enabled": true,
            "interval_secs": 900,
            "file_path": "/tmp/heartbeat.md"
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert!(config.heartbeat.enabled);
    assert_eq!(config.heartbeat.interval_secs, 900);
    assert_eq!(
        config.heartbeat.file_path,
        Some("/tmp/heartbeat.md".to_string())
    );
}

#[test]
fn test_config_skills_settings() {
    let json = r#"{
        "skills": {
            "enabled": true,
            "always_load": ["github", "weather"],
            "disabled": ["legacy"]
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert!(config.skills.enabled);
    assert_eq!(config.skills.always_load, vec!["github", "weather"]);
    assert_eq!(config.skills.disabled, vec!["legacy"]);
}

#[test]
fn test_config_whatsapp_and_gsheets_settings() {
    let json = r#"{
        "tools": {
            "whatsapp": {
                "phone_number_id": "123456789",
                "access_token": "wa-token",
                "default_language": "ms"
            },
            "google_sheets": {
                "access_token": "gs-token"
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(
        config.tools.whatsapp.phone_number_id,
        Some("123456789".to_string())
    );
    assert_eq!(
        config.tools.whatsapp.access_token,
        Some("wa-token".to_string())
    );
    assert_eq!(config.tools.whatsapp.default_language, "ms");
    assert_eq!(
        config.tools.google_sheets.access_token,
        Some("gs-token".to_string())
    );
}

#[test]
fn test_web_search_tool_creation() {
    let tool = WebSearchTool::new("test-key");
    assert_eq!(tool.name(), "web_search");
}

#[test]
fn test_web_fetch_tool_creation() {
    let tool = WebFetchTool::new();
    assert_eq!(tool.name(), "web_fetch");
}

#[test]
fn test_message_tool_creation() {
    let bus = Arc::new(MessageBus::new());
    let tool = MessageTool::new(bus);
    assert_eq!(tool.name(), "message");
}

#[test]
fn test_memory_search_tool_creation() {
    let tool = MemorySearchTool::new(Config::default().memory);
    assert_eq!(tool.name(), "memory_search");
}

#[test]
fn test_memory_get_tool_creation() {
    let tool = MemoryGetTool::new(Config::default().memory);
    assert_eq!(tool.name(), "memory_get");
}

#[test]
fn test_whatsapp_tool_creation() {
    let tool = WhatsAppTool::new("123", "token");
    assert_eq!(tool.name(), "whatsapp_send");
}

#[test]
fn test_google_sheets_tool_creation() {
    let tool = GoogleSheetsTool::new("token");
    assert_eq!(tool.name(), "google_sheets");
}

#[test]
fn test_heartbeat_is_empty_behavior() {
    assert!(HeartbeatService::is_empty(""));
    assert!(HeartbeatService::is_empty("# Header\n<!-- comment -->"));
    assert!(!HeartbeatService::is_empty("Check order updates"));
}

#[test]
fn test_skills_loader_frontmatter_parsing() {
    let root = tempdir().unwrap();
    let workspace = root.path().join("workspace_skills");
    let builtin = root.path().join("builtin_skills");
    std::fs::create_dir_all(workspace.join("demo")).unwrap();
    std::fs::create_dir_all(builtin.join("demo")).unwrap();

    std::fs::write(
        workspace.join("demo/SKILL.md"),
        "---\nname: demo\ndescription: Demo skill\n---\n# Demo",
    )
    .unwrap();
    std::fs::write(
        builtin.join("demo/SKILL.md"),
        "---\nname: demo\ndescription: Builtin demo\n---\n# Builtin",
    )
    .unwrap();

    let loader = SkillsLoader::new(workspace, Some(builtin));
    let skill = loader.load_skill("demo").unwrap();
    assert_eq!(skill.source, "workspace");
    assert_eq!(skill.description, "Demo skill");
}

#[test]
fn test_skills_loader_workspace_override_and_filter_unavailable() {
    let root = tempdir().unwrap();
    let workspace = root.path().join("workspace_skills");
    let builtin = root.path().join("builtin_skills");
    std::fs::create_dir_all(workspace.join("demo")).unwrap();
    std::fs::create_dir_all(builtin.join("demo")).unwrap();
    std::fs::create_dir_all(builtin.join("needs_env")).unwrap();

    std::fs::write(
        workspace.join("demo/SKILL.md"),
        "---\nname: demo\ndescription: Workspace demo\n---\n# Workspace Demo",
    )
    .unwrap();
    std::fs::write(
        builtin.join("demo/SKILL.md"),
        "---\nname: demo\ndescription: Builtin demo\n---\n# Builtin Demo",
    )
    .unwrap();
    std::fs::write(
        builtin.join("needs_env/SKILL.md"),
        "---\nname: needs_env\ndescription: Needs env var\nmetadata: {\"zeptoclaw\":{\"requires\":{\"env\":[\"ZEPTOCLAW_INTEGRATION_TEST_MISSING_ENV_2B9F2E16\"]}}}\n---\n# Needs Env",
    )
    .unwrap();

    let loader = SkillsLoader::new(workspace, Some(builtin));

    let demo = loader.load_skill("demo").unwrap();
    assert_eq!(demo.source, "workspace");
    assert_eq!(demo.description, "Workspace demo");

    let all_names: Vec<String> = loader
        .list_skills(false)
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(all_names.iter().any(|name| name == "demo"));
    assert!(all_names.iter().any(|name| name == "needs_env"));

    let available_names: Vec<String> = loader
        .list_skills(true)
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(available_names.iter().any(|name| name == "demo"));
    assert!(!available_names.iter().any(|name| name == "needs_env"));
}

#[tokio::test]
async fn test_multi_provider_fallback_uses_secondary_provider() {
    let provider = FallbackProvider::new(
        Box::new(AlwaysFailProvider),
        Box::new(StaticSuccessProvider {
            provider_name: "secondary",
            response: "fallback response",
        }),
    );

    let response = provider
        .chat(
            vec![Message::user("hello")],
            vec![],
            None,
            ChatOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(response.content, "fallback response");
}

// ============================================================================
// End-to-End Integration Tests
// ============================================================================

#[tokio::test]
async fn test_message_to_session_flow() {
    // Simulates the flow: Channel -> MessageBus -> Session
    let bus = MessageBus::new();
    let session_manager = SessionManager::new_memory();

    // Channel publishes a message
    let inbound = InboundMessage::new("telegram", "user123", "chat456", "Hello!");
    bus.publish_inbound(inbound).await.unwrap();

    // Agent receives from bus
    let received = bus.consume_inbound().await.unwrap();

    // Agent adds to session
    let mut session = session_manager
        .get_or_create(&received.session_key)
        .await
        .unwrap();
    session.add_message(Message::user(&received.content));

    // Simulate AI response
    let response_text = "Hello! How can I help you?";
    session.add_message(Message::assistant(response_text));
    session_manager.save(&session).await.unwrap();

    // Agent publishes response to bus
    let outbound = OutboundMessage::new(&received.channel, &received.chat_id, response_text);
    bus.publish_outbound(outbound).await.unwrap();

    // Verify full flow
    let saved_session = session_manager
        .get_or_create("telegram:chat456")
        .await
        .unwrap();
    assert_eq!(saved_session.messages.len(), 2);

    let outgoing = bus.consume_outbound().await.unwrap();
    assert_eq!(outgoing.content, "Hello! How can I help you?");
}

#[tokio::test]
async fn test_tool_call_flow() {
    // Simulates: User message -> Tool call -> Tool result -> Response
    let session_manager = SessionManager::new_memory();
    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Box::new(EchoTool));

    let mut session = session_manager.get_or_create("test-flow").await.unwrap();

    // User request
    session.add_message(Message::user("Echo this: Test message"));

    // Simulate LLM deciding to call a tool
    let tool_call =
        zeptoclaw::session::ToolCall::new("call_001", "echo", r#"{"message": "Test message"}"#);
    session.add_message(Message::assistant_with_tools(
        "I'll echo that for you.",
        vec![tool_call.clone()],
    ));

    // Execute the tool
    let args: serde_json::Value = tool_call.parse_arguments().unwrap();
    let tool_result = tool_registry.execute("echo", args).await.unwrap();
    assert_eq!(tool_result, "Test message");

    // Add tool result to session
    session.add_message(Message::tool_result(&tool_call.id, &tool_result));

    // Final response
    session.add_message(Message::assistant("I echoed your message: Test message"));

    session_manager.save(&session).await.unwrap();

    // Verify complete conversation
    let loaded = session_manager.get_or_create("test-flow").await.unwrap();
    assert_eq!(loaded.messages.len(), 4);
    assert!(loaded.messages[1].has_tool_calls());
    assert_eq!(loaded.messages[2].content, "Test message");
    assert!(loaded.messages[3].content.contains("echoed"));
}

#[tokio::test]
async fn test_multi_channel_sessions() {
    // Test that sessions from different channels are isolated
    let session_manager = SessionManager::new_memory();

    let channels = ["telegram", "discord", "slack"];
    for channel in channels {
        let session_key = format!("{}:user1:chat1", channel);
        let mut session = session_manager.get_or_create(&session_key).await.unwrap();
        session.add_message(Message::user(&format!("Hello from {}", channel)));
        session_manager.save(&session).await.unwrap();
    }

    // Verify each channel has its own session
    for channel in channels {
        let session_key = format!("{}:user1:chat1", channel);
        let session = session_manager.get_or_create(&session_key).await.unwrap();
        assert_eq!(session.messages.len(), 1);
        assert!(session.messages[0].content.contains(channel));
    }

    // Verify total session count
    let keys = session_manager.list().await.unwrap();
    assert_eq!(keys.len(), 3);
}

// ============================================================================
// Security Integration Tests
// ============================================================================

#[tokio::test]
async fn test_filesystem_path_traversal_protection() {
    let dir = tempdir().unwrap();
    let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
    let tool = ReadFileTool;

    // Attempt to read /etc/passwd via traversal
    let result = tool
        .execute(serde_json::json!({"path": "../../../etc/passwd"}), &ctx)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Security violation") || err.contains("escapes workspace"),
        "Expected security error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_shell_dangerous_command_blocked() {
    let tool = ShellTool::new();
    let ctx = ToolContext::new();

    let result = tool
        .execute(serde_json::json!({"command": "rm -rf /"}), &ctx)
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Security violation"));
}

#[tokio::test]
async fn test_security_config_customization() {
    // Create tool with custom blocked pattern
    let config = ShellSecurityConfig::new().block_pattern("custom_forbidden");
    let tool = ShellTool::with_security(config);
    let ctx = ToolContext::new();

    // Custom pattern should be blocked
    let result = tool
        .execute(
            serde_json::json!({"command": "echo custom_forbidden"}),
            &ctx,
        )
        .await;
    assert!(result.is_err());

    // Default tool should allow it
    let default_tool = ShellTool::new();
    let result = default_tool
        .execute(
            serde_json::json!({"command": "echo custom_forbidden"}),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_filesystem_absolute_path_outside_workspace_blocked() {
    let dir = tempdir().unwrap();
    let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());
    let tool = ReadFileTool;

    let result = tool
        .execute(serde_json::json!({"path": "/etc/passwd"}), &ctx)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_shell_credential_access_blocked() {
    let tool = ShellTool::new();
    let ctx = ToolContext::new();

    let result = tool
        .execute(serde_json::json!({"command": "cat /etc/shadow"}), &ctx)
        .await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Security violation"));
}

#[tokio::test]
async fn test_shell_permissive_mode() {
    let tool = ShellTool::permissive();
    let ctx = ToolContext::new();

    // In permissive mode, we can run commands that would normally be blocked
    // We use echo to safely test without actually running dangerous commands
    let result = tool
        .execute(
            serde_json::json!({"command": "echo 'test permissive'"}),
            &ctx,
        )
        .await;

    assert!(result.is_ok());
}

// ============================================================================
// Runtime Integration Tests
// ============================================================================

#[tokio::test]
async fn test_runtime_factory_native() {
    use zeptoclaw::config::RuntimeConfig;
    use zeptoclaw::runtime::create_runtime;

    let config = RuntimeConfig::default();
    let runtime = create_runtime(&config).await.unwrap();
    assert_eq!(runtime.name(), "native");
}

#[tokio::test]
async fn test_cron_scheduling_dispatches_message() {
    use chrono::Utc;
    use tokio::time::{timeout, Duration};
    use zeptoclaw::cron::{CronPayload, CronSchedule, CronService, OnMiss};

    let root = tempdir().unwrap();
    let bus = Arc::new(MessageBus::new());
    let service = CronService::new(root.path().join("jobs.json"), Arc::clone(&bus));
    service.start(&OnMiss::Skip).await.unwrap();

    let at_ms = (Utc::now() + chrono::Duration::seconds(1)).timestamp_millis();
    service
        .add_job(
            "once".to_string(),
            CronSchedule::At { at_ms },
            CronPayload {
                message: "scheduled message".to_string(),
                channel: "telegram".to_string(),
                chat_id: "cron-chat".to_string(),
            },
            true,
        )
        .await
        .unwrap();

    let inbound = timeout(Duration::from_secs(5), bus.consume_inbound())
        .await
        .expect("timed out waiting for cron message")
        .expect("message bus closed");

    assert_eq!(inbound.sender_id, "cron");
    assert_eq!(inbound.channel, "telegram");
    assert_eq!(inbound.chat_id, "cron-chat");
    assert_eq!(inbound.content, "scheduled message");

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(service.list_jobs(true).await.is_empty());
    service.stop().await;
}

#[tokio::test]
async fn test_heartbeat_trigger_now_enqueues_message() {
    use tokio::time::{timeout, Duration};

    let root = tempdir().unwrap();
    let heartbeat_path = root.path().join("HEARTBEAT.md");
    std::fs::write(
        &heartbeat_path,
        "- [ ] investigate failed webhook deliveries",
    )
    .unwrap();

    let bus = Arc::new(MessageBus::new());
    let service = HeartbeatService::new(heartbeat_path, 60, Arc::clone(&bus), "ops-chat");
    service.trigger_now().await.unwrap();

    let inbound = timeout(Duration::from_secs(1), bus.consume_inbound())
        .await
        .expect("timed out waiting for heartbeat message")
        .expect("message bus closed");

    assert_eq!(inbound.channel, "heartbeat");
    assert_eq!(inbound.sender_id, "system");
    assert_eq!(inbound.chat_id, "ops-chat");
    assert_eq!(inbound.content, HEARTBEAT_PROMPT);
}

#[tokio::test]
async fn test_heartbeat_trigger_now_skips_non_actionable_content() {
    use tokio::time::{timeout, Duration};

    let root = tempdir().unwrap();
    let heartbeat_path = root.path().join("HEARTBEAT.md");
    std::fs::write(&heartbeat_path, "# Heartbeat\n<!-- no tasks -->\n- [ ]").unwrap();

    let bus = Arc::new(MessageBus::new());
    let service = HeartbeatService::new(heartbeat_path, 60, Arc::clone(&bus), "ops-chat");
    service.trigger_now().await.unwrap();

    let receive_result = timeout(Duration::from_millis(300), bus.consume_inbound()).await;
    assert!(receive_result.is_err(), "expected no heartbeat message");
}

#[tokio::test]
async fn test_available_runtimes_includes_native() {
    use zeptoclaw::runtime::available_runtimes;

    let runtimes = available_runtimes().await;
    assert!(runtimes.contains(&"native"));
}

#[tokio::test]
async fn test_shell_tool_with_native_runtime() {
    use std::sync::Arc;
    use zeptoclaw::runtime::NativeRuntime;
    use zeptoclaw::tools::shell::ShellTool;
    use zeptoclaw::tools::{Tool, ToolContext};

    let runtime = Arc::new(NativeRuntime::new());
    let tool = ShellTool::with_runtime(runtime);
    let ctx = ToolContext::new();

    let result = tool
        .execute(serde_json::json!({"command": "echo hello"}), &ctx)
        .await;

    assert!(result.is_ok());
    assert!(result.unwrap().contains("hello"));
}

#[tokio::test]
async fn test_shell_tool_runtime_with_workspace() {
    use std::sync::Arc;
    use tempfile::tempdir;
    use zeptoclaw::runtime::NativeRuntime;
    use zeptoclaw::tools::shell::ShellTool;
    use zeptoclaw::tools::{Tool, ToolContext};

    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("test.txt"), "content").unwrap();

    let runtime = Arc::new(NativeRuntime::new());
    let tool = ShellTool::with_runtime(runtime);
    let ctx = ToolContext::new().with_workspace(dir.path().to_str().unwrap());

    let result = tool
        .execute(serde_json::json!({"command": "cat test.txt"}), &ctx)
        .await;

    assert!(result.is_ok());
    assert!(result.unwrap().contains("content"));
}

#[tokio::test]
async fn test_config_runtime_serialization() {
    use zeptoclaw::config::{RuntimeConfig, RuntimeType};

    let mut config = RuntimeConfig::default();
    config.runtime_type = RuntimeType::Docker;
    config.allow_fallback_to_native = true;
    config.docker.image = "ubuntu:22.04".to_string();

    let json = serde_json::to_string(&config).unwrap();
    let parsed: RuntimeConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.runtime_type, RuntimeType::Docker);
    assert!(parsed.allow_fallback_to_native);
    assert_eq!(parsed.docker.image, "ubuntu:22.04");
}

// ============================================================================
// Containerized Agent Integration Tests
// ============================================================================

#[test]
fn test_container_agent_config_deserialization() {
    let json = r#"{
        "container_agent": {
            "image": "zeptoclaw:custom",
            "memory_limit": "2g",
            "timeout_secs": 600
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.container_agent.image, "zeptoclaw:custom");
    assert_eq!(config.container_agent.memory_limit, Some("2g".to_string()));
    assert_eq!(config.container_agent.timeout_secs, 600);
    // Defaults should still be set
    assert_eq!(config.container_agent.docker_binary, None);
    assert_eq!(config.container_agent.cpu_limit, Some("2.0".to_string()));
    assert_eq!(config.container_agent.network, "none");
    assert!(config.container_agent.extra_mounts.is_empty());
}

#[test]
fn test_container_agent_config_defaults() {
    let config = Config::default();
    assert_eq!(config.container_agent.image, "zeptoclaw:latest");
    assert_eq!(config.container_agent.memory_limit, Some("1g".to_string()));
    assert_eq!(config.container_agent.docker_binary, None);
    assert_eq!(config.container_agent.cpu_limit, Some("2.0".to_string()));
    assert_eq!(config.container_agent.timeout_secs, 300);
    assert_eq!(config.container_agent.network, "none");
}

#[test]
fn test_ipc_response_markers() {
    use zeptoclaw::gateway::{parse_marked_response, AgentResponse, AgentResult};

    let response = AgentResponse::success("req-123", "Hello!", None);
    let marked = response.to_marked_json();

    assert!(marked.contains("<<<AGENT_RESPONSE_START>>>"));
    assert!(marked.contains("<<<AGENT_RESPONSE_END>>>"));

    let parsed = parse_marked_response(&marked).unwrap();
    assert_eq!(parsed.request_id, "req-123");

    match parsed.result {
        AgentResult::Success { content, .. } => {
            assert_eq!(content, "Hello!");
        }
        _ => panic!("Expected Success result"),
    }
}

#[test]
fn test_ipc_error_response_roundtrip() {
    use zeptoclaw::gateway::{parse_marked_response, AgentResponse, AgentResult};

    let response = AgentResponse::error("req-err", "Timeout exceeded", "TIMEOUT");
    let marked = response.to_marked_json();
    let parsed = parse_marked_response(&marked).unwrap();

    assert_eq!(parsed.request_id, "req-err");
    match parsed.result {
        AgentResult::Error { message, code } => {
            assert_eq!(message, "Timeout exceeded");
            assert_eq!(code, "TIMEOUT");
        }
        _ => panic!("Expected Error result"),
    }
}

#[test]
fn test_ipc_parse_with_noisy_stdout() {
    use zeptoclaw::gateway::{parse_marked_response, AgentResponse};

    let response = AgentResponse::success("noisy-req", "Result", None);
    let marked = response.to_marked_json();
    let noisy = format!(
        "INFO: Loading config...\nDEBUG: Provider ready\n{}\nDEBUG: Shutting down",
        marked
    );

    let parsed = parse_marked_response(&noisy).unwrap();
    assert_eq!(parsed.request_id, "noisy-req");
}

#[test]
fn test_ipc_parse_missing_markers_returns_none() {
    use zeptoclaw::gateway::parse_marked_response;

    assert!(parse_marked_response("no markers here").is_none());
    assert!(parse_marked_response(
        "<<<AGENT_RESPONSE_START>>>\n{invalid json\n<<<AGENT_RESPONSE_END>>>"
    )
    .is_none());
}

#[test]
fn test_container_agent_proxy_creation() {
    use zeptoclaw::gateway::{ContainerAgentProxy, ResolvedBackend};

    let config = Config::default();
    let bus = Arc::new(MessageBus::new());
    let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);
    assert!(!proxy.is_running());
}

#[test]
fn test_agent_request_serialization() {
    use zeptoclaw::gateway::AgentRequest;

    let request = AgentRequest {
        request_id: "test-req".to_string(),
        message: InboundMessage::new("test", "user1", "chat1", "Hello"),
        agent_config: Config::default().agents.defaults,
        session: None,
    };

    let json = serde_json::to_string(&request).unwrap();
    let parsed: AgentRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.request_id, "test-req");
    assert_eq!(parsed.message.content, "Hello");
    assert_eq!(parsed.message.channel, "test");
    assert!(parsed.validate().is_ok());
}

#[test]
fn test_agent_request_validation_rejects_mismatched_session_key() {
    use zeptoclaw::gateway::AgentRequest;
    use zeptoclaw::session::Session;

    let request = AgentRequest {
        request_id: "test-req-2".to_string(),
        message: InboundMessage::new("test", "user1", "chat1", "Hello"),
        agent_config: Config::default().agents.defaults,
        session: Some(Session::new("test:chat-mismatch")),
    };

    assert!(request.validate().is_err());
}

// ============================================================================
// DelegateTool Integration Tests
// ============================================================================

#[tokio::test]
async fn test_delegate_tool_recursion_blocking() {
    use zeptoclaw::providers::ClaudeProvider;
    use zeptoclaw::tools::delegate::DelegateTool;

    let config = Config::default();
    let bus = Arc::new(MessageBus::new());
    let provider: Arc<dyn zeptoclaw::providers::LLMProvider> =
        Arc::new(ClaudeProvider::new("fake-key"));

    let tool = DelegateTool::new(config, provider, bus);

    // Should block when called from delegate context (recursion prevention)
    let ctx = ToolContext::new().with_channel("delegate", "sub-1");
    let result = tool
        .execute(serde_json::json!({"role": "Test", "task": "hello"}), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("recursion"));
}

#[tokio::test]
async fn test_delegate_tool_disabled_config() {
    use zeptoclaw::providers::ClaudeProvider;
    use zeptoclaw::tools::delegate::DelegateTool;

    let mut config = Config::default();
    config.swarm.enabled = false;
    let bus = Arc::new(MessageBus::new());
    let provider: Arc<dyn zeptoclaw::providers::LLMProvider> =
        Arc::new(ClaudeProvider::new("fake-key"));

    let tool = DelegateTool::new(config, provider, bus);
    let ctx = ToolContext::new().with_channel("telegram", "chat-1");
    let result = tool
        .execute(serde_json::json!({"role": "Test", "task": "hello"}), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("disabled"));
}

#[tokio::test]
async fn test_delegate_tool_missing_args() {
    use zeptoclaw::providers::ClaudeProvider;
    use zeptoclaw::tools::delegate::DelegateTool;

    let config = Config::default();
    let bus = Arc::new(MessageBus::new());
    let provider: Arc<dyn zeptoclaw::providers::LLMProvider> =
        Arc::new(ClaudeProvider::new("fake-key"));

    let tool = DelegateTool::new(config, provider, bus);
    let ctx = ToolContext::new().with_channel("telegram", "chat-1");

    // Missing role
    let result = tool
        .execute(serde_json::json!({"task": "hello"}), &ctx)
        .await;
    assert!(result.is_err());

    // Missing task
    let result = tool
        .execute(serde_json::json!({"role": "Test"}), &ctx)
        .await;
    assert!(result.is_err());

    // Both missing
    let result = tool.execute(serde_json::json!({}), &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_delegate_tool_in_registry() {
    use zeptoclaw::providers::ClaudeProvider;
    use zeptoclaw::tools::delegate::DelegateTool;

    let config = Config::default();
    let bus = Arc::new(MessageBus::new());
    let provider: Arc<dyn zeptoclaw::providers::LLMProvider> =
        Arc::new(ClaudeProvider::new("fake-key"));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DelegateTool::new(config, provider, bus)));

    assert!(registry.has("delegate"));
    let defs = registry.definitions();
    let delegate_def = defs.iter().find(|d| d.name == "delegate");
    assert!(delegate_def.is_some());
    assert!(delegate_def.unwrap().description.contains("specialist"));
}

// ============================================================================
// Streaming Configuration Tests
// ============================================================================

#[test]
fn test_streaming_config_default_false() {
    let config = Config::default();
    assert!(!config.agents.defaults.streaming);
}

#[test]
fn test_streaming_config_json_roundtrip() {
    let json = r#"{"agents":{"defaults":{"streaming":true}}}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert!(config.agents.defaults.streaming);
}

#[tokio::test]
async fn test_agent_loop_streaming_accessors() {
    let config = Config::default();
    let session_manager = SessionManager::new_memory();
    let bus = Arc::new(MessageBus::new());
    let agent = zeptoclaw::agent::AgentLoop::new(config, session_manager, bus);

    assert!(!agent.is_streaming());
    agent.set_streaming(true);
    assert!(agent.is_streaming());
    agent.set_streaming(false);
    assert!(!agent.is_streaming());
}
