//! Context builder for agent conversations
//!
//! This module provides the `ContextBuilder` for constructing the system prompt
//! and message history for LLM conversations. It also provides `RuntimeContext`
//! for injecting environment-awareness into the agent's system prompt.

use crate::session::Message;

/// Default system prompt for ZeptoClaw agent
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are ZeptoClaw, an ultra-lightweight personal AI assistant.

You have access to tools to help accomplish tasks. Use them when needed.

Be concise but helpful. Focus on completing the user's request efficiently."#;

/// Runtime context injected into the system prompt to make agents environment-aware.
///
/// This struct captures information about the agent's runtime environment such as
/// the channel it is running on, available tools, current time, workspace path,
/// and OS/platform details. When rendered, it produces a `## Runtime Context`
/// section appended to the system prompt.
///
/// # Example
///
/// ```rust
/// use zeptoclaw::agent::RuntimeContext;
///
/// let ctx = RuntimeContext::new()
///     .with_channel("telegram")
///     .with_tools(vec!["shell".to_string(), "web_search".to_string()])
///     .with_workspace("/home/user/project")
///     .with_os_info();
///
/// let rendered = ctx.render().unwrap();
/// assert!(rendered.contains("Channel: telegram"));
/// assert!(rendered.contains("shell, web_search"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct RuntimeContext {
    /// The channel the agent is running on (e.g., "telegram", "cli", "whatsapp", "discord")
    pub channel: Option<String>,
    /// Names of available tools
    pub available_tools: Vec<String>,
    /// Current timestamp (ISO 8601)
    pub current_time: Option<String>,
    /// Workspace path
    pub workspace: Option<String>,
    /// OS/platform info (e.g., "linux aarch64", "macos aarch64")
    pub os_info: Option<String>,
}

impl RuntimeContext {
    /// Create a new empty runtime context.
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::RuntimeContext;
    ///
    /// let ctx = RuntimeContext::new();
    /// assert!(ctx.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the channel name.
    ///
    /// # Arguments
    /// * `channel` - The channel identifier (e.g., "telegram", "cli", "discord")
    pub fn with_channel(mut self, channel: &str) -> Self {
        self.channel = Some(channel.to_string());
        self
    }

    /// Set the list of available tool names.
    ///
    /// # Arguments
    /// * `tools` - Vector of tool name strings
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.available_tools = tools;
        self
    }

    /// Set the current time to now (UTC, ISO 8601 / RFC 3339).
    pub fn with_current_time(mut self) -> Self {
        self.current_time = Some(chrono::Utc::now().to_rfc3339());
        self
    }

    /// Set the workspace path.
    ///
    /// # Arguments
    /// * `workspace` - The workspace directory path
    pub fn with_workspace(mut self, workspace: &str) -> Self {
        self.workspace = Some(workspace.to_string());
        self
    }

    /// Set the OS/platform info from the current environment.
    pub fn with_os_info(mut self) -> Self {
        self.os_info = Some(format!(
            "{} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        self
    }

    /// Check if any context field is set.
    ///
    /// Returns `true` if no fields have been populated.
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::RuntimeContext;
    ///
    /// assert!(RuntimeContext::new().is_empty());
    /// assert!(!RuntimeContext::new().with_channel("cli").is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.channel.is_none()
            && self.available_tools.is_empty()
            && self.current_time.is_none()
            && self.workspace.is_none()
            && self.os_info.is_none()
    }

    /// Render the context as a markdown section for the system prompt.
    ///
    /// Returns `None` if no context fields are set.
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::RuntimeContext;
    ///
    /// let ctx = RuntimeContext::new().with_channel("cli");
    /// let rendered = ctx.render().unwrap();
    /// assert!(rendered.starts_with("## Runtime Context"));
    /// assert!(rendered.contains("Channel: cli"));
    /// ```
    pub fn render(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        let mut parts = Vec::new();
        if let Some(ref channel) = self.channel {
            parts.push(format!("- Channel: {}", channel));
        }
        if !self.available_tools.is_empty() {
            parts.push(format!(
                "- Available tools: {}",
                self.available_tools.join(", ")
            ));
        }
        if let Some(ref time) = self.current_time {
            parts.push(format!("- Current time: {}", time));
        }
        if let Some(ref workspace) = self.workspace {
            parts.push(format!("- Workspace: {}", workspace));
        }
        if let Some(ref os) = self.os_info {
            parts.push(format!("- Platform: {}", os));
        }

        Some(format!("## Runtime Context\n\n{}", parts.join("\n")))
    }
}

/// Builder for constructing conversation context for LLM calls.
///
/// The `ContextBuilder` helps construct the full message list including
/// system prompts, skills information, conversation history, and user input.
///
/// # Example
///
/// ```rust
/// use zeptoclaw::agent::ContextBuilder;
/// use zeptoclaw::session::Message;
///
/// let builder = ContextBuilder::new()
///     .with_skills("- /help: Show help information");
///
/// let messages = builder.build_messages(&[], "Hello!");
/// assert_eq!(messages.len(), 2); // system + user message
/// ```
pub struct ContextBuilder {
    /// The system prompt to use
    system_prompt: String,
    /// Optional SOUL.md content prepended before system prompt
    soul_prompt: Option<String>,
    /// Optional skills content to append to system prompt
    skills_prompt: Option<String>,
    /// Optional runtime context to append to system prompt
    runtime_context: Option<RuntimeContext>,
    /// Optional memory context to append to system prompt
    memory_context: Option<String>,
}

impl ContextBuilder {
    /// Create a new context builder with the default system prompt.
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    ///
    /// let builder = ContextBuilder::new();
    /// let system = builder.build_system_message();
    /// assert!(system.content.contains("ZeptoClaw"));
    /// ```
    pub fn new() -> Self {
        Self {
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            soul_prompt: None,
            skills_prompt: None,
            runtime_context: None,
            memory_context: None,
        }
    }

    /// Set a custom system prompt.
    ///
    /// # Arguments
    /// * `prompt` - The custom system prompt to use
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    ///
    /// let builder = ContextBuilder::new()
    ///     .with_system_prompt("You are a helpful assistant.");
    /// let system = builder.build_system_message();
    /// assert!(system.content.contains("helpful assistant"));
    /// ```
    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        self.system_prompt = prompt.to_string();
        self
    }

    /// Set SOUL.md identity content, prepended before the system prompt.
    ///
    /// SOUL.md defines the agent's personality, values, and behavioral
    /// constraints. Content is prepended to the system prompt so it takes
    /// priority in the LLM's context.
    ///
    /// # Arguments
    /// * `content` - The SOUL.md content to prepend
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    ///
    /// let builder = ContextBuilder::new()
    ///     .with_soul("You are kind and empathetic.");
    /// let system = builder.build_system_message();
    /// assert!(system.content.starts_with("You are kind"));
    /// ```
    pub fn with_soul(mut self, content: &str) -> Self {
        self.soul_prompt = Some(content.to_string());
        self
    }

    /// Add skills information to the system prompt.
    ///
    /// Skills content is appended to the system prompt under an
    /// "Available Skills" section.
    ///
    /// # Arguments
    /// * `skills_content` - The skills documentation to include
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    ///
    /// let builder = ContextBuilder::new()
    ///     .with_skills("- /search: Search the web\n- /help: Show help");
    /// let system = builder.build_system_message();
    /// assert!(system.content.contains("Available Skills"));
    /// assert!(system.content.contains("/search"));
    /// ```
    pub fn with_skills(mut self, skills_content: &str) -> Self {
        self.skills_prompt = Some(skills_content.to_string());
        self
    }

    /// Add runtime context to the system prompt.
    ///
    /// Runtime context provides the agent with awareness of its environment
    /// including the channel, available tools, current time, workspace, and
    /// platform information.
    ///
    /// If the provided context is empty (no fields set), it is ignored.
    ///
    /// # Arguments
    /// * `ctx` - The runtime context to inject
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::{ContextBuilder, RuntimeContext};
    ///
    /// let ctx = RuntimeContext::new()
    ///     .with_channel("discord")
    ///     .with_os_info();
    /// let builder = ContextBuilder::new().with_runtime_context(ctx);
    /// let system = builder.build_system_message();
    /// assert!(system.content.contains("Runtime Context"));
    /// assert!(system.content.contains("discord"));
    /// ```
    pub fn with_runtime_context(mut self, ctx: RuntimeContext) -> Self {
        if !ctx.is_empty() {
            self.runtime_context = Some(ctx);
        }
        self
    }

    /// Add memory context to the system prompt.
    ///
    /// Injects long-term memory content (pinned + relevant entries) as a
    /// `## Memory` section. If the provided string is empty, it is ignored.
    ///
    /// # Arguments
    /// * `memory_context` - Pre-built memory injection string
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    ///
    /// let builder = ContextBuilder::new()
    ///     .with_memory_context("## Memory\n\n### Pinned\n- user:name: Alice".to_string());
    /// let system = builder.build_system_message();
    /// assert!(system.content.contains("## Memory"));
    /// ```
    pub fn with_memory_context(mut self, memory_context: String) -> Self {
        if !memory_context.is_empty() {
            self.memory_context = Some(memory_context);
        }
        self
    }

    /// Build the system message with all configured content.
    ///
    /// # Returns
    /// A `Message` with role `System` containing the full system prompt.
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    /// use zeptoclaw::session::Role;
    ///
    /// let builder = ContextBuilder::new();
    /// let system = builder.build_system_message();
    /// assert_eq!(system.role, Role::System);
    /// ```
    pub fn build_system_message(&self) -> Message {
        let mut content = String::new();
        if let Some(ref soul) = self.soul_prompt {
            content.push_str(soul);
            content.push_str("\n\n");
        }
        content.push_str(&self.system_prompt);
        if let Some(ref skills) = self.skills_prompt {
            content.push_str("\n\n## Available Skills\n\n");
            content.push_str(skills);
        }
        if let Some(ref ctx) = self.runtime_context {
            if let Some(rendered) = ctx.render() {
                content.push_str("\n\n");
                content.push_str(&rendered);
            }
        }
        if let Some(ref mem) = self.memory_context {
            content.push_str("\n\n");
            content.push_str(mem);
        }
        Message::system(&content)
    }

    /// Build the full message list for an LLM call.
    ///
    /// This constructs a message list with:
    /// 1. System message (with skills if configured)
    /// 2. Conversation history
    /// 3. New user input (if non-empty)
    ///
    /// # Arguments
    /// * `history` - The conversation history to include
    /// * `user_input` - The new user message (empty string is ignored)
    ///
    /// # Returns
    /// A vector of messages ready for the LLM.
    ///
    /// # Example
    /// ```rust
    /// use zeptoclaw::agent::ContextBuilder;
    /// use zeptoclaw::session::Message;
    ///
    /// let builder = ContextBuilder::new();
    /// let history = vec![
    ///     Message::user("Hello"),
    ///     Message::assistant("Hi there!"),
    /// ];
    /// let messages = builder.build_messages(&history, "How are you?");
    /// assert_eq!(messages.len(), 4); // system + 2 history + new user
    /// ```
    pub fn build_messages(&self, history: &[Message], user_input: &str) -> Vec<Message> {
        let mut messages = vec![self.build_system_message()];
        messages.extend(history.iter().cloned());
        if !user_input.is_empty() {
            messages.push(Message::user(user_input));
        }
        messages
    }

    /// Get the current system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Check if a SOUL.md identity is configured.
    pub fn has_soul(&self) -> bool {
        self.soul_prompt.is_some()
    }

    /// Check if skills are configured.
    pub fn has_skills(&self) -> bool {
        self.skills_prompt.is_some()
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Role;

    #[test]
    fn test_context_builder_new() {
        let builder = ContextBuilder::new();
        assert!(builder.system_prompt().contains("ZeptoClaw"));
        assert!(!builder.has_skills());
    }

    #[test]
    fn test_context_builder_default() {
        let builder = ContextBuilder::default();
        assert!(builder.system_prompt().contains("ZeptoClaw"));
    }

    #[test]
    fn test_context_builder_custom_system_prompt() {
        let builder = ContextBuilder::new().with_system_prompt("Custom prompt here");
        assert_eq!(builder.system_prompt(), "Custom prompt here");
    }

    #[test]
    fn test_context_builder_with_skills() {
        let builder = ContextBuilder::new().with_skills("- /test: Test skill");
        assert!(builder.has_skills());

        let system = builder.build_system_message();
        assert!(system.content.contains("Available Skills"));
        assert!(system.content.contains("/test"));
    }

    #[test]
    fn test_build_system_message() {
        let builder = ContextBuilder::new();
        let system = builder.build_system_message();

        assert_eq!(system.role, Role::System);
        assert!(system.content.contains("ZeptoClaw"));
    }

    #[test]
    fn test_build_messages_empty_input() {
        let builder = ContextBuilder::new();
        let messages = builder.build_messages(&[], "");

        // Only system message when input is empty
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
    }

    #[test]
    fn test_build_messages_with_input() {
        let builder = ContextBuilder::new();
        let messages = builder.build_messages(&[], "Hello");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].content, "Hello");
    }

    #[test]
    fn test_build_messages_with_history() {
        let builder = ContextBuilder::new();
        let history = vec![
            Message::user("Previous message"),
            Message::assistant("Previous response"),
        ];
        let messages = builder.build_messages(&history, "New message");

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].content, "Previous message");
        assert_eq!(messages[2].role, Role::Assistant);
        assert_eq!(messages[3].role, Role::User);
        assert_eq!(messages[3].content, "New message");
    }

    #[test]
    fn test_build_messages_preserves_history_order() {
        let builder = ContextBuilder::new();
        let history = vec![
            Message::user("First"),
            Message::assistant("Second"),
            Message::user("Third"),
            Message::assistant("Fourth"),
        ];
        let messages = builder.build_messages(&history, "");

        // System + 4 history messages (no new input since it's empty)
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[1].content, "First");
        assert_eq!(messages[2].content, "Second");
        assert_eq!(messages[3].content, "Third");
        assert_eq!(messages[4].content, "Fourth");
    }

    #[test]
    fn test_context_builder_with_soul() {
        let builder = ContextBuilder::new().with_soul("You are a pirate captain.");
        assert!(builder.has_soul());

        let system = builder.build_system_message();
        assert!(system.content.starts_with("You are a pirate captain."));
        assert!(system.content.contains("ZeptoClaw"));
    }

    #[test]
    fn test_soul_prepended_before_system_prompt() {
        let builder = ContextBuilder::new()
            .with_soul("SOUL: Be kind.")
            .with_system_prompt("SYSTEM: Do tasks.");
        let system = builder.build_system_message();

        let soul_pos = system.content.find("SOUL: Be kind.").unwrap();
        let system_pos = system.content.find("SYSTEM: Do tasks.").unwrap();
        assert!(soul_pos < system_pos);
    }

    #[test]
    fn test_soul_with_skills() {
        let builder = ContextBuilder::new()
            .with_soul("Identity: helper")
            .with_skills("- /test: Test");
        let system = builder.build_system_message();

        assert!(system.content.starts_with("Identity: helper"));
        assert!(system.content.contains("ZeptoClaw"));
        assert!(system.content.contains("Available Skills"));
        assert!(system.content.contains("/test"));
    }

    #[test]
    fn test_no_soul_by_default() {
        let builder = ContextBuilder::new();
        assert!(!builder.has_soul());

        let system = builder.build_system_message();
        assert!(system.content.starts_with("You are ZeptoClaw"));
    }

    #[test]
    fn test_context_builder_chaining() {
        let builder = ContextBuilder::new()
            .with_system_prompt("Custom prompt")
            .with_skills("- /skill1: Do something");

        let system = builder.build_system_message();
        assert!(system.content.contains("Custom prompt"));
        assert!(system.content.contains("/skill1"));
    }

    #[test]
    fn test_build_messages_with_tool_calls_in_history() {
        use crate::session::ToolCall;

        let builder = ContextBuilder::new();
        let history = vec![
            Message::user("Search for rust"),
            Message::assistant_with_tools(
                "Let me search for that.",
                vec![ToolCall::new("call_1", "search", r#"{"q": "rust"}"#)],
            ),
            Message::tool_result("call_1", "Found 100 results"),
            Message::assistant("I found 100 results about Rust."),
        ];
        let messages = builder.build_messages(&history, "Thanks!");

        // System + 4 history + new user message
        assert_eq!(messages.len(), 6);
        assert!(messages[2].has_tool_calls());
        assert!(messages[3].is_tool_result());
    }

    // ---- RuntimeContext tests ----

    #[test]
    fn test_runtime_context_empty() {
        let ctx = RuntimeContext::new();
        assert!(ctx.is_empty());
        assert!(ctx.render().is_none());
    }

    #[test]
    fn test_runtime_context_default() {
        let ctx = RuntimeContext::default();
        assert!(ctx.is_empty());
        assert!(ctx.channel.is_none());
        assert!(ctx.available_tools.is_empty());
        assert!(ctx.current_time.is_none());
        assert!(ctx.workspace.is_none());
        assert!(ctx.os_info.is_none());
    }

    #[test]
    fn test_runtime_context_with_channel() {
        let ctx = RuntimeContext::new().with_channel("telegram");
        assert!(!ctx.is_empty());
        let rendered = ctx.render().unwrap();
        assert!(rendered.contains("Channel: telegram"));
    }

    #[test]
    fn test_runtime_context_with_tools() {
        let ctx =
            RuntimeContext::new().with_tools(vec!["shell".to_string(), "web_search".to_string()]);
        assert!(!ctx.is_empty());
        let rendered = ctx.render().unwrap();
        assert!(rendered.contains("shell, web_search"));
    }

    #[test]
    fn test_runtime_context_with_empty_tools() {
        let ctx = RuntimeContext::new().with_tools(vec![]);
        assert!(ctx.is_empty());
        assert!(ctx.render().is_none());
    }

    #[test]
    fn test_runtime_context_with_current_time() {
        let ctx = RuntimeContext::new().with_current_time();
        assert!(!ctx.is_empty());
        let rendered = ctx.render().unwrap();
        assert!(rendered.contains("Current time:"));
        // Should contain a valid RFC 3339 timestamp-like string
        assert!(rendered.contains("20"));
    }

    #[test]
    fn test_runtime_context_with_os_info() {
        let ctx = RuntimeContext::new().with_os_info();
        assert!(!ctx.is_empty());
        let rendered = ctx.render().unwrap();
        assert!(rendered.contains("Platform:"));
        // Should contain the current OS
        assert!(rendered.contains(std::env::consts::OS));
    }

    #[test]
    fn test_runtime_context_with_workspace() {
        let ctx = RuntimeContext::new().with_workspace("/home/user/project");
        assert!(!ctx.is_empty());
        let rendered = ctx.render().unwrap();
        assert!(rendered.contains("Workspace: /home/user/project"));
    }

    #[test]
    fn test_runtime_context_full() {
        let ctx = RuntimeContext::new()
            .with_channel("whatsapp")
            .with_tools(vec!["shell".to_string()])
            .with_workspace("/tmp/test")
            .with_os_info();
        let rendered = ctx.render().unwrap();
        assert!(rendered.contains("## Runtime Context"));
        assert!(rendered.contains("Channel: whatsapp"));
        assert!(rendered.contains("Available tools: shell"));
        assert!(rendered.contains("Workspace: /tmp/test"));
        assert!(rendered.contains("Platform:"));
    }

    #[test]
    fn test_runtime_context_render_ordering() {
        let ctx = RuntimeContext::new()
            .with_channel("cli")
            .with_tools(vec!["echo".to_string()])
            .with_workspace("/work");
        let rendered = ctx.render().unwrap();
        let channel_pos = rendered.find("Channel:").unwrap();
        let tools_pos = rendered.find("Available tools:").unwrap();
        let workspace_pos = rendered.find("Workspace:").unwrap();
        // Channel comes before tools, tools before workspace
        assert!(channel_pos < tools_pos);
        assert!(tools_pos < workspace_pos);
    }

    #[test]
    fn test_runtime_context_clone() {
        let ctx = RuntimeContext::new()
            .with_channel("discord")
            .with_workspace("/tmp");
        let cloned = ctx.clone();
        assert_eq!(ctx.channel, cloned.channel);
        assert_eq!(ctx.workspace, cloned.workspace);
    }

    // ---- ContextBuilder + RuntimeContext integration tests ----

    #[test]
    fn test_context_builder_with_runtime_context() {
        let ctx = RuntimeContext::new().with_channel("discord");
        let builder = ContextBuilder::new().with_runtime_context(ctx);
        let system = builder.build_system_message();
        assert!(system.content.contains("Runtime Context"));
        assert!(system.content.contains("discord"));
    }

    #[test]
    fn test_context_builder_empty_runtime_context_adds_nothing() {
        let ctx = RuntimeContext::new();
        let builder = ContextBuilder::new().with_runtime_context(ctx);
        let system = builder.build_system_message();
        assert!(!system.content.contains("Runtime Context"));
    }

    #[test]
    fn test_context_builder_all_sections() {
        let ctx = RuntimeContext::new().with_channel("cli");
        let builder = ContextBuilder::new()
            .with_skills("- /help: Show help")
            .with_runtime_context(ctx);
        let system = builder.build_system_message();
        assert!(system.content.contains("ZeptoClaw"));
        assert!(system.content.contains("Available Skills"));
        assert!(system.content.contains("## Runtime Context"));
        assert!(system.content.contains("cli"));
    }

    #[test]
    fn test_context_builder_section_ordering() {
        let ctx = RuntimeContext::new().with_channel("slack");
        let builder = ContextBuilder::new()
            .with_skills("- /deploy: Deploy app")
            .with_runtime_context(ctx);
        let system = builder.build_system_message();
        let skills_pos = system.content.find("Available Skills").unwrap();
        let runtime_pos = system.content.find("Runtime Context").unwrap();
        // Skills section should come before runtime context
        assert!(skills_pos < runtime_pos);
    }

    #[test]
    fn test_context_builder_runtime_context_in_messages() {
        let ctx = RuntimeContext::new()
            .with_channel("telegram")
            .with_os_info();
        let builder = ContextBuilder::new().with_runtime_context(ctx);
        let messages = builder.build_messages(&[], "Hello");
        assert_eq!(messages.len(), 2);
        assert!(messages[0].content.contains("Runtime Context"));
        assert!(messages[0].content.contains("telegram"));
    }

    // ---- Memory context tests ----

    #[test]
    fn test_context_builder_with_memory_context() {
        let builder = ContextBuilder::new()
            .with_memory_context("## Memory\n\n### Pinned\n- user:name: Alice".to_string());
        let system = builder.build_system_message();
        assert!(system.content.contains("## Memory"));
        assert!(system.content.contains("user:name: Alice"));
    }

    #[test]
    fn test_context_builder_empty_memory_context_skipped() {
        let builder = ContextBuilder::new().with_memory_context(String::new());
        let system = builder.build_system_message();
        assert!(!system.content.contains("## Memory"));
    }

    #[test]
    fn test_context_builder_memory_after_runtime() {
        let ctx = RuntimeContext::new().with_channel("cli");
        let builder = ContextBuilder::new()
            .with_runtime_context(ctx)
            .with_memory_context("## Memory\n\n### Pinned\n- k: v".to_string());
        let system = builder.build_system_message();
        let runtime_pos = system.content.find("Runtime Context").unwrap();
        let memory_pos = system.content.find("## Memory").unwrap();
        assert!(runtime_pos < memory_pos);
    }
}
