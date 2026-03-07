//! Agent module - Core AI agent logic and conversation handling
//!
//! This module provides the core agent loop and context building functionality
//! for ZeptoClaw. The agent is responsible for:
//!
//! - Processing inbound messages from channels
//! - Building conversation context with system prompts and history
//! - Calling LLM providers for responses
//! - Executing tool calls and feeding results back to the LLM
//! - Managing conversation sessions
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │  MessageBus │────>│  AgentLoop  │────>│ LLMProvider │
//! │  (inbound)  │     │             │     │  (Claude)   │
//! └─────────────┘     └─────────────┘     └─────────────┘
//!                            │                   │
//!                            │                   │
//!                            ▼                   ▼
//!                     ┌─────────────┐     ┌─────────────┐
//!                     │   Session   │     │    Tools    │
//!                     │   Manager   │     │  Registry   │
//!                     └─────────────┘     └─────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use zeptoclaw::agent::AgentLoop;
//! use zeptoclaw::bus::MessageBus;
//! use zeptoclaw::config::Config;
//! use zeptoclaw::session::SessionManager;
//! use zeptoclaw::providers::ClaudeProvider;
//! use zeptoclaw::tools::EchoTool;
//!
//! async fn run_agent() {
//!     let config = Config::default();
//!     let session_manager = SessionManager::new_memory();
//!     let bus = Arc::new(MessageBus::new());
//!     let agent = AgentLoop::new(config, session_manager, bus);
//!
//!     // Configure provider
//!     let provider = ClaudeProvider::new("your-api-key");
//!     agent.set_provider(Box::new(provider)).await;
//!
//!     // Register tools
//!     agent.register_tool(Box::new(EchoTool)).await;
//!
//!     // Start the agent loop
//!     agent.start().await.unwrap();
//! }
//! ```

pub mod budget;
pub mod compaction;
mod context;
pub mod context_monitor;
pub mod facade;
mod r#loop;
pub mod loop_guard;
pub mod scratchpad;
pub mod tool_call_limit;

pub use budget::TokenBudget;
pub use context::{format_message_envelope, ContextBuilder, RuntimeContext};
pub use context_monitor::{CompactionStrategy, ContextMonitor};
pub use facade::{ZeptoAgent, ZeptoAgentBuilder};
pub use r#loop::AgentLoop;
pub use r#loop::{ToolFeedback, ToolFeedbackPhase};
pub use scratchpad::SwarmScratchpad;
pub use tool_call_limit::ToolCallLimitTracker;
