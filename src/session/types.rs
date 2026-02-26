//! Session types for ZeptoClaw
//!
//! This module defines the core types for session and conversation management,
//! including messages, roles, and tool calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A conversation session containing messages and metadata.
///
/// Sessions are identified by a unique key and store the full conversation
/// history along with optional summary information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique identifier for this session (e.g., "telegram:chat123")
    pub key: String,
    /// Ordered list of messages in this conversation
    pub messages: Vec<Message>,
    /// Optional summary of previous conversation context
    pub summary: Option<String>,
    /// When this session was created
    pub created_at: DateTime<Utc>,
    /// When this session was last modified
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create a new empty session with the given key.
    ///
    /// # Arguments
    /// * `key` - Unique identifier for this session
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::Session;
    ///
    /// let session = Session::new("telegram:chat123");
    /// assert!(session.messages.is_empty());
    /// ```
    pub fn new(key: &str) -> Self {
        let now = Utc::now();
        Self {
            key: key.to_string(),
            messages: Vec::new(),
            summary: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a message to this session.
    ///
    /// Also updates the `updated_at` timestamp.
    ///
    /// # Arguments
    /// * `message` - The message to add
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Session, Message};
    ///
    /// let mut session = Session::new("test");
    /// session.add_message(Message::user("Hello!"));
    /// assert_eq!(session.messages.len(), 1);
    /// ```
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.updated_at = Utc::now();
    }

    /// Clear all messages and summary from this session.
    ///
    /// Also updates the `updated_at` timestamp.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Session, Message};
    ///
    /// let mut session = Session::new("test");
    /// session.add_message(Message::user("Hello!"));
    /// session.clear();
    /// assert!(session.messages.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.messages.clear();
        self.summary = None;
        self.updated_at = Utc::now();
    }

    /// Set a summary for this session.
    ///
    /// Summaries are used to condense long conversation histories.
    ///
    /// # Arguments
    /// * `summary` - The summary text
    pub fn set_summary(&mut self, summary: &str) {
        self.summary = Some(summary.to_string());
        self.updated_at = Utc::now();
    }

    /// Get the number of messages in this session.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Check if this session is empty (no messages).
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get the last message in this session, if any.
    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Get messages by role.
    pub fn messages_by_role(&self, role: Role) -> Vec<&Message> {
        self.messages.iter().filter(|m| m.role == role).collect()
    }
}

/// A single message in a conversation.
///
/// Messages can be from users, assistants, system prompts, or tool results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message sender
    pub role: Role,
    /// The text content of the message
    pub content: String,
    /// Tool calls made by the assistant (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// ID of the tool call this message is responding to (for tool results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Create a new user message.
    ///
    /// # Arguments
    /// * `content` - The message content
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Message, Role};
    ///
    /// let msg = Message::user("Hello, assistant!");
    /// assert_eq!(msg.role, Role::User);
    /// ```
    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a new assistant message.
    ///
    /// # Arguments
    /// * `content` - The message content
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Message, Role};
    ///
    /// let msg = Message::assistant("Hello, user!");
    /// assert_eq!(msg.role, Role::Assistant);
    /// ```
    pub fn assistant(content: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a new system message.
    ///
    /// System messages are used for prompts and instructions.
    ///
    /// # Arguments
    /// * `content` - The system prompt content
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Message, Role};
    ///
    /// let msg = Message::system("You are a helpful assistant.");
    /// assert_eq!(msg.role, Role::System);
    /// ```
    pub fn system(content: &str) -> Self {
        Self {
            role: Role::System,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a new tool result message.
    ///
    /// Tool results are responses from tool executions.
    ///
    /// # Arguments
    /// * `tool_call_id` - The ID of the tool call this is responding to
    /// * `content` - The result content from the tool
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Message, Role};
    ///
    /// let msg = Message::tool_result("call_123", "Tool executed successfully");
    /// assert_eq!(msg.role, Role::Tool);
    /// assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
    /// ```
    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: Role::Tool,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
        }
    }

    /// Create an assistant message with tool calls.
    ///
    /// # Arguments
    /// * `content` - Optional text content
    /// * `tool_calls` - The tool calls to include
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::{Message, ToolCall, Role};
    ///
    /// let tool_call = ToolCall::new("call_1", "search", r#"{"query": "rust"}"#);
    /// let msg = Message::assistant_with_tools("Let me search for that.", vec![tool_call]);
    /// assert!(msg.tool_calls.is_some());
    /// ```
    pub fn assistant_with_tools(content: &str, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.to_string(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    /// Check if this message has tool calls.
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls
            .as_ref()
            .map(|tc| !tc.is_empty())
            .unwrap_or(false)
    }

    /// Check if this is a tool result message.
    pub fn is_tool_result(&self) -> bool {
        self.role == Role::Tool && self.tool_call_id.is_some()
    }
}

/// The role of a message sender in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompts and instructions
    System,
    /// Messages from the user
    User,
    /// Messages from the AI assistant
    Assistant,
    /// Results from tool executions
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// A tool call made by the assistant.
///
/// Tool calls represent requests to execute specific tools with given arguments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    /// Unique identifier for this tool call
    pub id: String,
    /// Name of the tool to call
    pub name: String,
    /// JSON-encoded arguments for the tool
    pub arguments: String,
}

impl ToolCall {
    /// Create a new tool call.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this call
    /// * `name` - Name of the tool
    /// * `arguments` - JSON-encoded arguments
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::session::ToolCall;
    ///
    /// let call = ToolCall::new("call_123", "web_search", r#"{"query": "rust programming"}"#);
    /// assert_eq!(call.name, "web_search");
    /// ```
    pub fn new(id: &str, name: &str, arguments: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    /// Parse the arguments as a specific type.
    ///
    /// # Returns
    /// The parsed arguments, or an error if parsing fails.
    pub fn parse_arguments<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(&self.arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_new() {
        let session = Session::new("test-session");
        assert_eq!(session.key, "test-session");
        assert!(session.messages.is_empty());
        assert!(session.summary.is_none());
        assert!(session.created_at <= session.updated_at);
    }

    #[test]
    fn test_session_add_message() {
        let mut session = Session::new("test");
        let initial_updated = session.updated_at;

        // Small delay to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));

        session.add_message(Message::user("Hello"));
        assert_eq!(session.messages.len(), 1);
        assert!(session.updated_at >= initial_updated);
    }

    #[test]
    fn test_session_clear() {
        let mut session = Session::new("test");
        session.add_message(Message::user("Hello"));
        session.set_summary("A greeting");

        session.clear();

        assert!(session.messages.is_empty());
        assert!(session.summary.is_none());
    }

    #[test]
    fn test_session_helpers() {
        let mut session = Session::new("test");
        assert!(session.is_empty());
        assert_eq!(session.message_count(), 0);
        assert!(session.last_message().is_none());

        session.add_message(Message::user("Hello"));
        session.add_message(Message::assistant("Hi!"));

        assert!(!session.is_empty());
        assert_eq!(session.message_count(), 2);
        assert_eq!(session.last_message().unwrap().role, Role::Assistant);
        assert_eq!(session.messages_by_role(Role::User).len(), 1);
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("Hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "Hello");
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("Hi there");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "Hi there");
    }

    #[test]
    fn test_message_system() {
        let msg = Message::system("You are helpful");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.content, "You are helpful");
    }

    #[test]
    fn test_message_tool_result() {
        let msg = Message::tool_result("call_123", "Success");
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.content, "Success");
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
        assert!(msg.is_tool_result());
    }

    #[test]
    fn test_message_with_tool_calls() {
        let tool_call = ToolCall::new("call_1", "search", r#"{"q": "test"}"#);
        let msg = Message::assistant_with_tools("Searching...", vec![tool_call]);

        assert!(msg.has_tool_calls());
        let calls = msg.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
    }

    #[test]
    fn test_role_display() {
        assert_eq!(Role::System.to_string(), "system");
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
        assert_eq!(Role::Tool.to_string(), "tool");
    }

    #[test]
    fn test_role_serialize() {
        let user = Role::User;
        let json = serde_json::to_string(&user).unwrap();
        assert_eq!(json, r#""user""#);

        let parsed: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Role::User);
    }

    #[test]
    fn test_tool_call_new() {
        let call = ToolCall::new("call_123", "web_search", r#"{"query": "rust"}"#);
        assert_eq!(call.id, "call_123");
        assert_eq!(call.name, "web_search");
        assert_eq!(call.arguments, r#"{"query": "rust"}"#);
    }

    #[test]
    fn test_tool_call_parse_arguments() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct SearchArgs {
            query: String,
        }

        let call = ToolCall::new("call_1", "search", r#"{"query": "rust"}"#);
        let args: SearchArgs = call.parse_arguments().unwrap();
        assert_eq!(args.query, "rust");
    }

    #[test]
    fn test_session_serialization() {
        let mut session = Session::new("test-session");
        session.add_message(Message::user("Hello"));
        session.add_message(Message::assistant("Hi!"));

        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.key, "test-session");
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, Role::User);
        assert_eq!(parsed.messages[1].role, Role::Assistant);
    }

    #[test]
    fn test_message_serialization_skips_none() {
        let msg = Message::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();

        // tool_calls and tool_call_id should not be in JSON when None
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_call_id"));
    }
}
