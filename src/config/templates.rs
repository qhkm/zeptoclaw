//! Agent template system for ZeptoClaw
//!
//! Provides predefined agent configurations (templates) that users can reference
//! by name instead of manually configuring system prompts, tool whitelists, and
//! model settings. Templates can be built-in or loaded from JSON files in
//! `~/.zeptoclaw/templates/`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::error::{Result, ZeptoError};

// ============================================================================
// AgentTemplate
// ============================================================================

/// A predefined agent configuration that can be referenced by name.
///
/// Templates encapsulate a system prompt, optional model/generation overrides,
/// and tool access policies so users can quickly switch between agent personas
/// (e.g., "coder", "researcher", "writer") without manual configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    /// Unique template name (e.g., "coder", "researcher", "writer").
    pub name: String,

    /// Human-readable description of this template's purpose.
    pub description: String,

    /// The system prompt injected for this agent role.
    pub system_prompt: String,

    /// Optional model override (e.g., "claude-sonnet-4-5-20250929").
    /// When `None`, the agent uses the default model from config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Optional max tokens override for responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Optional temperature override for generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Whitelist of tool names the agent is allowed to use.
    /// `None` means all tools are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,

    /// Blacklist of tool names the agent is forbidden from using.
    /// Applied after `allowed_tools` filtering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_tools: Option<Vec<String>>,

    /// Optional max tool iterations override per turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_iterations: Option<u32>,

    /// Metadata tags for categorization and filtering.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Shell command allowlist — binary names the agent may execute.
    /// `None` = use default shell config (blocklist-only, no allowlist).
    /// `Some(vec![])` = deny all shell commands (Strict + empty allowlist).
    /// `Some(vec!["git", "cargo"])` = only these binaries allowed (Strict mode).
    /// Applied to ShellTool, CustomTool, and PluginTool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_allowlist: Option<Vec<String>>,

    /// Token budget for this agent run (input + output tokens combined).
    /// `None` = inherit from config.agents.defaults.token_budget.
    /// When both template and global are set, the lower value wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_token_budget: Option<u64>,

    /// Hard cap on total tool calls across the entire agent run.
    /// `None` = no cap (only per-turn max_tool_iterations applies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,
}

// ============================================================================
// Built-in Templates
// ============================================================================

/// Creates the built-in "coder" template.
///
/// An expert software engineering persona with access to all tools.
fn builtin_coder() -> AgentTemplate {
    AgentTemplate {
        name: "coder".to_string(),
        description: "Expert software engineer that writes clean, well-tested code".to_string(),
        system_prompt: concat!(
            "You are an expert software engineer. You write clean, idiomatic, ",
            "well-structured code following best practices for the language and ",
            "framework in use. You explain your reasoning, consider edge cases, ",
            "handle errors properly, and write tests when appropriate. You prefer ",
            "simple, readable solutions over clever ones.\n\n",
            "You have access to grep (regex search across files) and find (glob file discovery) ",
            "tools for navigating and understanding codebases.\n\n",
            "When asked to fix bugs or modify code, follow this workflow:\n",
            "1. Before writing any tests, check for existing test files (e.g., test_*.py, *_test.go, ",
            "*Test.java). Use find or grep to discover them.\n",
            "2. If existing tests exist, run them FIRST to see which tests fail and understand the ",
            "current state of the code.\n",
            "3. Fix bugs by editing the original source file in-place using edit_file. Do NOT create ",
            "new copies of the file or rewrite it with write_file unless explicitly asked to.\n",
            "4. After making changes, verify by re-running the EXISTING test suite — not by writing ",
            "your own tests. Only write new tests if no relevant tests exist or the user asks for them.\n",
            "5. Show a clear summary of what you changed and why."
        )
        .to_string(),
        model: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: None, // all tools
        blocked_tools: None,
        max_tool_iterations: None,
        tags: vec!["development".to_string(), "coding".to_string()],
        shell_allowlist: None,
        max_token_budget: None,
        max_tool_calls: None,
    }
}

/// Creates the built-in "researcher" template.
///
/// A research assistant with tool access limited to information-gathering tools.
fn builtin_researcher() -> AgentTemplate {
    AgentTemplate {
        name: "researcher".to_string(),
        description: "Research assistant that finds, analyzes, and summarizes information"
            .to_string(),
        system_prompt: concat!(
            "You are a thorough research assistant. You find relevant information, ",
            "analyze it critically, summarize your findings clearly, and cite your ",
            "sources. You distinguish between facts and opinions, flag uncertainties, ",
            "and present multiple perspectives when they exist."
        )
        .to_string(),
        model: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: Some(vec![
            "web_search".to_string(),
            "web_fetch".to_string(),
            "memory_search".to_string(),
            "memory_get".to_string(),
            "longterm_memory".to_string(),
        ]),
        blocked_tools: None,
        max_tool_iterations: None,
        tags: vec!["research".to_string(), "information".to_string()],
        shell_allowlist: None,
        max_token_budget: None,
        max_tool_calls: None,
    }
}

/// Creates the built-in "writer" template.
///
/// A professional writer with tool access limited to file and memory tools.
fn builtin_writer() -> AgentTemplate {
    AgentTemplate {
        name: "writer".to_string(),
        description: "Professional writer that produces clear, concise, well-structured content"
            .to_string(),
        system_prompt: concat!(
            "You are a professional writer. You produce clear, concise, and ",
            "well-structured content. You adapt your tone and style to the context ",
            "and audience. You pay attention to grammar, flow, and readability. ",
            "You organize ideas logically and use examples to illustrate points."
        )
        .to_string(),
        model: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: Some(vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "edit_file".to_string(),
            "memory_search".to_string(),
            "memory_get".to_string(),
        ]),
        blocked_tools: None,
        max_tool_iterations: None,
        tags: vec!["writing".to_string(), "content".to_string()],
        shell_allowlist: None,
        max_token_budget: None,
        max_tool_calls: None,
    }
}

/// Creates the built-in "assistant" template.
///
/// A general-purpose helpful assistant with access to all tools.
fn builtin_assistant() -> AgentTemplate {
    AgentTemplate {
        name: "assistant".to_string(),
        description: "Helpful general assistant for everyday tasks and questions".to_string(),
        system_prompt: concat!(
            "You are a helpful general assistant. You answer questions accurately, ",
            "perform tasks efficiently, and communicate clearly. You ask for ",
            "clarification when a request is ambiguous and provide concise but ",
            "complete responses."
        )
        .to_string(),
        model: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: None, // all tools
        blocked_tools: None,
        max_tool_iterations: None,
        tags: vec!["general".to_string()],
        shell_allowlist: None,
        max_token_budget: None,
        max_tool_calls: None,
    }
}

/// Creates the built-in "task-manager" template.
///
/// A task/project management persona that uses reminders and long-term memory
/// to capture, track, and manage tasks via chat.
fn builtin_task_manager() -> AgentTemplate {
    AgentTemplate {
        name: "task-manager".to_string(),
        description: "AI project manager that captures tasks, sets reminders, and tracks progress"
            .to_string(),
        system_prompt: concat!(
            "You are an AI task and project manager. You help the user capture tasks from ",
            "natural conversation, set reminders for deadlines, track completion, and provide ",
            "daily summaries.\n\n",
            "When the user mentions something to do, proactively offer to add it as a reminder. ",
            "When asked about their list, show pending reminders grouped by category. When a ",
            "task is completed, mark it done and congratulate briefly.\n\n",
            "Use long-term memory to remember user preferences (preferred categories, working ",
            "hours, recurring patterns). Use the reminder tool for all task tracking. Use the ",
            "message tool to send proactive updates.\n\n",
            "Keep responses concise and action-oriented. Use bullet points for lists. ",
            "Always confirm actions taken (e.g., 'Added reminder: Call dentist at 2pm').",
        )
        .to_string(),
        model: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: Some(vec![
            "reminder".to_string(),
            "longterm_memory".to_string(),
            "message".to_string(),
            "cron".to_string(),
        ]),
        blocked_tools: None,
        max_tool_iterations: None,
        tags: vec![
            "productivity".to_string(),
            "tasks".to_string(),
            "personal-assistant".to_string(),
        ],
        shell_allowlist: None,
        max_token_budget: None,
        max_tool_calls: None,
    }
}

/// Returns all built-in templates as a vector.
fn builtin_templates() -> Vec<AgentTemplate> {
    vec![
        builtin_coder(),
        builtin_researcher(),
        builtin_writer(),
        builtin_assistant(),
        builtin_task_manager(),
    ]
}

// ============================================================================
// TemplateRegistry
// ============================================================================

/// Registry of agent templates, combining built-in and user-defined templates.
///
/// Templates are stored in a `HashMap` keyed by name. User-defined templates
/// loaded from `~/.zeptoclaw/templates/` can override built-in templates with
/// the same name.
#[derive(Debug, Clone)]
pub struct TemplateRegistry {
    templates: HashMap<String, AgentTemplate>,
}

impl TemplateRegistry {
    /// Creates a new registry pre-loaded with all built-in templates.
    pub fn new() -> Self {
        let mut templates = HashMap::new();
        for tpl in builtin_templates() {
            templates.insert(tpl.name.clone(), tpl);
        }
        Self { templates }
    }

    /// Registers a template, overriding any existing template with the same name.
    pub fn register(&mut self, template: AgentTemplate) {
        self.templates.insert(template.name.clone(), template);
    }

    /// Looks up a template by name.
    pub fn get(&self, name: &str) -> Option<&AgentTemplate> {
        self.templates.get(name)
    }

    /// Returns all registered templates (in arbitrary order).
    pub fn list(&self) -> Vec<&AgentTemplate> {
        self.templates.values().collect()
    }

    /// Returns templates that contain the given tag.
    pub fn list_by_tag(&self, tag: &str) -> Vec<&AgentTemplate> {
        self.templates
            .values()
            .filter(|t| t.tags.iter().any(|t_tag| t_tag == tag))
            .collect()
    }

    /// Returns just the template names (in arbitrary order).
    pub fn names(&self) -> Vec<&str> {
        self.templates.keys().map(|k| k.as_str()).collect()
    }

    /// Loads all `.json` and `.toml` template files from a directory.
    ///
    /// Returns the successfully parsed templates. Files that are not valid JSON/TOML
    /// or do not deserialize into `AgentTemplate` are skipped with a warning
    /// logged via `tracing`.
    ///
    /// Returns `Err` only if the directory itself cannot be read. A nonexistent
    /// directory returns an empty `Vec` (not an error) for convenience.
    pub fn load_from_dir(dir: &Path) -> Result<Vec<AgentTemplate>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }

        if !dir.is_dir() {
            return Err(ZeptoError::Config(format!(
                "Template path is not a directory: {}",
                dir.display()
            )));
        }

        let entries = std::fs::read_dir(dir)?;
        let mut templates = Vec::new();

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only process .json and .toml files
            let ext = path.extension().and_then(|e| e.to_str());
            if ext != Some("json") && ext != Some("toml") {
                continue;
            }

            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let parse_result: std::result::Result<AgentTemplate, String> =
                        if ext == Some("toml") {
                            toml::from_str(&content).map_err(|e| e.to_string())
                        } else {
                            serde_json::from_str(&content).map_err(|e| e.to_string())
                        };
                    match parse_result {
                        Ok(template) => {
                            templates.push(template);
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "Skipping invalid template file"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "Failed to read template file"
                    );
                }
            }
        }

        Ok(templates)
    }

    /// Loads templates from a directory and registers them in this registry.
    ///
    /// Returns the number of templates successfully loaded and registered.
    /// User-defined templates override any existing templates with the same name.
    pub fn merge_from_dir(&mut self, dir: &Path) -> Result<usize> {
        let templates = Self::load_from_dir(dir)?;
        let count = templates.len();
        for template in templates {
            self.register(template);
        }
        Ok(count)
    }
}

impl Default for TemplateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_builtin_coder_exists() {
        let registry = TemplateRegistry::new();
        let coder = registry.get("coder");
        assert!(coder.is_some());
        let coder = coder.unwrap();
        assert_eq!(coder.name, "coder");
        assert!(coder.allowed_tools.is_none()); // all tools
        assert!(coder.tags.contains(&"development".to_string()));
        assert!(coder.tags.contains(&"coding".to_string()));
    }

    #[test]
    fn test_coder_prompt_instructs_existing_test_workflow() {
        let registry = TemplateRegistry::new();
        let coder = registry.get("coder").unwrap();
        let prompt = &coder.system_prompt;
        // Must instruct to check for existing tests before writing new ones
        assert!(
            prompt.contains("existing test"),
            "coder prompt must mention existing tests"
        );
        // Must instruct to use edit_file for in-place fixes
        assert!(
            prompt.contains("edit_file"),
            "coder prompt must instruct using edit_file"
        );
        // Must instruct to run existing tests for verification
        assert!(
            prompt.contains("re-running the EXISTING test suite"),
            "coder prompt must instruct re-running existing tests"
        );
        // Must NOT encourage writing own tests as default verification
        assert!(
            prompt.contains("Only write new tests if no relevant tests exist"),
            "coder prompt must discourage writing new tests when existing ones are available"
        );
    }

    #[test]
    fn test_builtin_researcher_exists() {
        let registry = TemplateRegistry::new();
        let researcher = registry.get("researcher");
        assert!(researcher.is_some());
        let researcher = researcher.unwrap();
        assert_eq!(researcher.name, "researcher");
        assert!(researcher.allowed_tools.is_some());
        let tools = researcher.allowed_tools.as_ref().unwrap();
        assert!(tools.contains(&"web_search".to_string()));
        assert!(tools.contains(&"web_fetch".to_string()));
        assert!(tools.contains(&"memory_search".to_string()));
        assert!(researcher.tags.contains(&"research".to_string()));
    }

    #[test]
    fn test_builtin_writer_exists() {
        let registry = TemplateRegistry::new();
        let writer = registry.get("writer");
        assert!(writer.is_some());
        let writer = writer.unwrap();
        assert_eq!(writer.name, "writer");
        assert!(writer.allowed_tools.is_some());
        let tools = writer.allowed_tools.as_ref().unwrap();
        assert!(tools.contains(&"read_file".to_string()));
        assert!(tools.contains(&"write_file".to_string()));
        assert!(tools.contains(&"edit_file".to_string()));
        assert!(writer.tags.contains(&"writing".to_string()));
    }

    #[test]
    fn test_builtin_assistant_exists() {
        let registry = TemplateRegistry::new();
        let assistant = registry.get("assistant");
        assert!(assistant.is_some());
        let assistant = assistant.unwrap();
        assert_eq!(assistant.name, "assistant");
        assert!(assistant.allowed_tools.is_none()); // all tools
        assert!(assistant.tags.contains(&"general".to_string()));
    }

    #[test]
    fn test_builtin_task_manager_exists() {
        let registry = TemplateRegistry::new();
        let tpl = registry.get("task-manager");
        assert!(tpl.is_some());
        let tpl = tpl.unwrap();
        assert_eq!(tpl.name, "task-manager");
        let tools = tpl.allowed_tools.as_ref().unwrap();
        assert!(tools.contains(&"reminder".to_string()));
        assert!(tools.contains(&"longterm_memory".to_string()));
        assert!(tools.contains(&"message".to_string()));
        assert!(tools.contains(&"cron".to_string()));
        assert!(tpl.tags.contains(&"productivity".to_string()));
        assert!(tpl.tags.contains(&"personal-assistant".to_string()));
    }

    #[test]
    fn test_task_manager_by_tag() {
        let registry = TemplateRegistry::new();
        let personal = registry.list_by_tag("personal-assistant");
        assert_eq!(personal.len(), 1);
        assert_eq!(personal[0].name, "task-manager");
    }

    #[test]
    fn test_lookup_returns_none_for_unknown() {
        let registry = TemplateRegistry::new();
        assert!(registry.get("nonexistent-template").is_none());
        assert!(registry.get("").is_none());
        assert!(registry.get("CODER").is_none()); // case-sensitive
    }

    #[test]
    fn test_list_all_templates() {
        let registry = TemplateRegistry::new();
        let all = registry.list();
        assert_eq!(all.len(), 5);

        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"coder"));
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"writer"));
        assert!(names.contains(&"assistant"));
        assert!(names.contains(&"task-manager"));
    }

    #[test]
    fn test_list_by_tag() {
        let registry = TemplateRegistry::new();

        let dev = registry.list_by_tag("development");
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].name, "coder");

        let research = registry.list_by_tag("research");
        assert_eq!(research.len(), 1);
        assert_eq!(research[0].name, "researcher");

        let general = registry.list_by_tag("general");
        assert_eq!(general.len(), 1);
        assert_eq!(general[0].name, "assistant");

        // Tag that does not exist
        let empty = registry.list_by_tag("nonexistent-tag");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_register_custom_template() {
        let mut registry = TemplateRegistry::new();
        let custom = AgentTemplate {
            name: "devops".to_string(),
            description: "DevOps engineer".to_string(),
            system_prompt: "You are a DevOps engineer.".to_string(),
            model: Some("gpt-5.1".to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.3),
            allowed_tools: Some(vec!["shell".to_string()]),
            blocked_tools: None,
            max_tool_iterations: Some(10),
            tags: vec!["devops".to_string(), "infrastructure".to_string()],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };

        registry.register(custom);
        assert_eq!(registry.list().len(), 6);

        let devops = registry.get("devops").unwrap();
        assert_eq!(devops.model, Some("gpt-5.1".to_string()));
        assert_eq!(devops.max_tokens, Some(4096));
        assert_eq!(devops.temperature, Some(0.3));
        assert_eq!(devops.max_tool_iterations, Some(10));
    }

    #[test]
    fn test_custom_template_overrides_builtin() {
        let mut registry = TemplateRegistry::new();

        // Verify original
        let original = registry.get("coder").unwrap();
        assert!(original.model.is_none());

        // Override
        let custom_coder = AgentTemplate {
            name: "coder".to_string(),
            description: "Custom coder".to_string(),
            system_prompt: "You are a Rust expert.".to_string(),
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            max_tokens: None,
            temperature: None,
            allowed_tools: Some(vec!["shell".to_string(), "read_file".to_string()]),
            blocked_tools: None,
            max_tool_iterations: None,
            tags: vec!["development".to_string(), "rust".to_string()],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };
        registry.register(custom_coder);

        // Verify override
        let overridden = registry.get("coder").unwrap();
        assert_eq!(overridden.description, "Custom coder");
        assert_eq!(
            overridden.model,
            Some("claude-sonnet-4-5-20250929".to_string())
        );
        assert!(overridden.tags.contains(&"rust".to_string()));

        // Total count should remain the same
        assert_eq!(registry.list().len(), 5);
    }

    #[test]
    fn test_names_list() {
        let registry = TemplateRegistry::new();
        let names = registry.names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"coder"));
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"writer"));
        assert!(names.contains(&"assistant"));
        assert!(names.contains(&"task-manager"));
    }

    #[test]
    fn test_load_from_directory_with_json_files() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_load");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Write two valid template files
        let tpl1 = r#"{
            "name": "ops-agent",
            "description": "Operations agent",
            "system_prompt": "You handle operations.",
            "model": "gpt-5.1",
            "tags": ["ops"]
        }"#;
        fs::write(temp_dir.join("ops-agent.json"), tpl1).unwrap();

        let tpl2 = r#"{
            "name": "data-analyst",
            "description": "Data analysis agent",
            "system_prompt": "You analyze data.",
            "allowed_tools": ["read_file", "shell"],
            "tags": ["data", "analysis"]
        }"#;
        fs::write(temp_dir.join("data-analyst.json"), tpl2).unwrap();

        // Write a non-json file that should be skipped
        fs::write(temp_dir.join("README.md"), "# Templates").unwrap();

        let templates = TemplateRegistry::load_from_dir(&temp_dir).unwrap();
        assert_eq!(templates.len(), 2);

        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"ops-agent"));
        assert!(names.contains(&"data-analyst"));

        // Verify fields on one of them
        let ops = templates.iter().find(|t| t.name == "ops-agent").unwrap();
        assert_eq!(ops.model, Some("gpt-5.1".to_string()));

        // Clean up
        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_from_empty_directory() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_empty");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let templates = TemplateRegistry::load_from_dir(&temp_dir).unwrap();
        assert!(templates.is_empty());

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_from_nonexistent_directory() {
        let path = Path::new("/tmp/zeptoclaw_tpl_nonexistent_98765");
        let templates = TemplateRegistry::load_from_dir(path).unwrap();
        assert!(templates.is_empty());
    }

    #[test]
    fn test_load_invalid_json_file() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_invalid");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Write an invalid JSON file
        fs::write(temp_dir.join("broken.json"), "{ not valid json }}}").unwrap();

        // Write a valid one alongside it
        let valid = r#"{
            "name": "valid-one",
            "description": "Valid template",
            "system_prompt": "Hello.",
            "tags": []
        }"#;
        fs::write(temp_dir.join("valid.json"), valid).unwrap();

        let templates = TemplateRegistry::load_from_dir(&temp_dir).unwrap();
        // Only the valid template should load; invalid is skipped
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "valid-one");

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_template_serialization_roundtrip() {
        let template = AgentTemplate {
            name: "roundtrip".to_string(),
            description: "Roundtrip test".to_string(),
            system_prompt: "Test prompt.".to_string(),
            model: Some("gpt-5.1".to_string()),
            max_tokens: Some(2048),
            temperature: Some(0.5),
            allowed_tools: Some(vec!["shell".to_string(), "read_file".to_string()]),
            blocked_tools: Some(vec!["web_search".to_string()]),
            max_tool_iterations: Some(15),
            tags: vec!["test".to_string()],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };

        let json = serde_json::to_string_pretty(&template).unwrap();
        let deserialized: AgentTemplate = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "roundtrip");
        assert_eq!(deserialized.description, "Roundtrip test");
        assert_eq!(deserialized.system_prompt, "Test prompt.");
        assert_eq!(deserialized.model, Some("gpt-5.1".to_string()));
        assert_eq!(deserialized.max_tokens, Some(2048));
        assert_eq!(deserialized.temperature, Some(0.5));
        assert_eq!(
            deserialized.allowed_tools,
            Some(vec!["shell".to_string(), "read_file".to_string()])
        );
        assert_eq!(
            deserialized.blocked_tools,
            Some(vec!["web_search".to_string()])
        );
        assert_eq!(deserialized.max_tool_iterations, Some(15));
        assert_eq!(deserialized.tags, vec!["test".to_string()]);
    }

    #[test]
    fn test_blocked_tools_field() {
        let json = r#"{
            "name": "restricted",
            "description": "Restricted agent",
            "system_prompt": "You have restrictions.",
            "blocked_tools": ["shell", "write_file"],
            "tags": ["restricted"]
        }"#;

        let template: AgentTemplate = serde_json::from_str(json).unwrap();
        assert!(template.allowed_tools.is_none());
        assert!(template.blocked_tools.is_some());
        let blocked = template.blocked_tools.unwrap();
        assert_eq!(blocked.len(), 2);
        assert!(blocked.contains(&"shell".to_string()));
        assert!(blocked.contains(&"write_file".to_string()));
    }

    #[test]
    fn test_allowed_tools_none_means_all_tools() {
        let json = r#"{
            "name": "unrestricted",
            "description": "Unrestricted agent",
            "system_prompt": "You have all tools.",
            "tags": []
        }"#;

        let template: AgentTemplate = serde_json::from_str(json).unwrap();
        assert!(template.allowed_tools.is_none());
        assert!(template.blocked_tools.is_none());

        // Verify built-in "coder" and "assistant" also use None for all tools
        let registry = TemplateRegistry::new();
        assert!(registry.get("coder").unwrap().allowed_tools.is_none());
        assert!(registry.get("assistant").unwrap().allowed_tools.is_none());
    }

    #[test]
    fn test_merge_from_dir() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_merge");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Write a custom template that overrides "coder"
        let custom_coder = r#"{
            "name": "coder",
            "description": "Custom coder from file",
            "system_prompt": "You are a Go expert.",
            "model": "gpt-5.1",
            "tags": ["development", "go"]
        }"#;
        fs::write(temp_dir.join("coder.json"), custom_coder).unwrap();

        // Write a brand new template
        let new_tpl = r#"{
            "name": "translator",
            "description": "Language translator",
            "system_prompt": "You translate text between languages.",
            "tags": ["translation"]
        }"#;
        fs::write(temp_dir.join("translator.json"), new_tpl).unwrap();

        let mut registry = TemplateRegistry::new();
        assert_eq!(registry.list().len(), 5);

        let count = registry.merge_from_dir(&temp_dir).unwrap();
        assert_eq!(count, 2);
        assert_eq!(registry.list().len(), 6); // 5 built-in + 1 new (coder was overridden)

        // Verify override took effect
        let coder = registry.get("coder").unwrap();
        assert_eq!(coder.description, "Custom coder from file");
        assert_eq!(coder.model, Some("gpt-5.1".to_string()));

        // Verify new template exists
        let translator = registry.get("translator").unwrap();
        assert_eq!(translator.description, "Language translator");

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_default_impl_matches_new() {
        let from_new = TemplateRegistry::new();
        let from_default = TemplateRegistry::default();
        assert_eq!(from_new.names().len(), from_default.names().len());
    }

    #[test]
    fn test_template_optional_fields_default_to_none() {
        let minimal_json = r#"{
            "name": "minimal",
            "description": "Minimal template",
            "system_prompt": "Hello."
        }"#;

        let template: AgentTemplate = serde_json::from_str(minimal_json).unwrap();
        assert_eq!(template.name, "minimal");
        assert!(template.model.is_none());
        assert!(template.max_tokens.is_none());
        assert!(template.temperature.is_none());
        assert!(template.allowed_tools.is_none());
        assert!(template.blocked_tools.is_none());
        assert!(template.max_tool_iterations.is_none());
        assert!(template.tags.is_empty());
    }

    #[test]
    fn test_serialization_skips_none_fields() {
        let template = AgentTemplate {
            name: "sparse".to_string(),
            description: "Sparse template".to_string(),
            system_prompt: "Hello.".to_string(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: None,
            blocked_tools: None,
            max_tool_iterations: None,
            tags: vec![],
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
        };

        let json = serde_json::to_string(&template).unwrap();
        assert!(!json.contains("model"));
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("temperature"));
        assert!(!json.contains("allowed_tools"));
        assert!(!json.contains("blocked_tools"));
        assert!(!json.contains("max_tool_iterations"));
    }

    #[test]
    fn test_new_sandbox_fields_deserialize_from_json() {
        let json = r#"{
            "name": "sandboxed",
            "description": "Sandboxed agent",
            "system_prompt": "You are sandboxed.",
            "shell_allowlist": ["git", "cargo"],
            "max_token_budget": 50000,
            "max_tool_calls": 100,
            "tags": []
        }"#;
        let tpl: AgentTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(
            tpl.shell_allowlist,
            Some(vec!["git".to_string(), "cargo".to_string()])
        );
        assert_eq!(tpl.max_token_budget, Some(50000));
        assert_eq!(tpl.max_tool_calls, Some(100));
    }

    #[test]
    fn test_new_sandbox_fields_default_to_none() {
        let json = r#"{
            "name": "minimal",
            "description": "Minimal template",
            "system_prompt": "Hello."
        }"#;
        let tpl: AgentTemplate = serde_json::from_str(json).unwrap();
        assert!(tpl.shell_allowlist.is_none());
        assert!(tpl.max_token_budget.is_none());
        assert!(tpl.max_tool_calls.is_none());
    }

    #[test]
    fn test_empty_shell_allowlist_means_deny_all() {
        let json = r#"{
            "name": "no-shell",
            "description": "No shell access",
            "system_prompt": "No shell.",
            "shell_allowlist": [],
            "tags": []
        }"#;
        let tpl: AgentTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(tpl.shell_allowlist, Some(vec![]));
    }

    #[test]
    fn test_serialization_skips_none_sandbox_fields() {
        let tpl = AgentTemplate {
            name: "sparse".to_string(),
            description: "Sparse".to_string(),
            system_prompt: "Hello.".to_string(),
            model: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: None,
            blocked_tools: None,
            max_tool_iterations: None,
            shell_allowlist: None,
            max_token_budget: None,
            max_tool_calls: None,
            tags: vec![],
        };
        let json = serde_json::to_string(&tpl).unwrap();
        assert!(!json.contains("shell_allowlist"));
        assert!(!json.contains("max_token_budget"));
        assert!(!json.contains("max_tool_calls"));
    }

    #[test]
    fn test_load_from_dir_not_a_directory() {
        let temp_file = std::env::temp_dir().join("zeptoclaw_tpl_test_notdir.txt");
        fs::write(&temp_file, "not a directory").unwrap();

        let result = TemplateRegistry::load_from_dir(&temp_file);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ZeptoError::Config(msg) if msg.contains("not a directory")));

        fs::remove_file(&temp_file).ok();
    }

    #[test]
    fn test_load_toml_template_from_dir() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_toml");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let toml_content = r#"
name = "toml-agent"
description = "Agent from TOML"
system_prompt = "You are a TOML agent."
shell_allowlist = ["git", "cargo"]
max_token_budget = 30000
max_tool_calls = 50
tags = ["toml"]
"#;
        fs::write(temp_dir.join("toml-agent.toml"), toml_content).unwrap();

        let templates = TemplateRegistry::load_from_dir(&temp_dir).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "toml-agent");
        assert_eq!(
            templates[0].shell_allowlist,
            Some(vec!["git".to_string(), "cargo".to_string()])
        );
        assert_eq!(templates[0].max_token_budget, Some(30000));
        assert_eq!(templates[0].max_tool_calls, Some(50));

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_load_mixed_json_and_toml_templates() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_mixed");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let json_tpl = r#"{
            "name": "json-one",
            "description": "JSON template",
            "system_prompt": "Hi.",
            "tags": []
        }"#;
        fs::write(temp_dir.join("json-one.json"), json_tpl).unwrap();

        let toml_tpl =
            "name = \"toml-one\"\ndescription = \"TOML template\"\nsystem_prompt = \"Hi.\"\ntags = []\n";
        fs::write(temp_dir.join("toml-one.toml"), toml_tpl).unwrap();

        let templates = TemplateRegistry::load_from_dir(&temp_dir).unwrap();
        assert_eq!(templates.len(), 2);

        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"json-one"));
        assert!(names.contains(&"toml-one"));

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_invalid_toml_skipped() {
        let temp_dir = std::env::temp_dir().join("zeptoclaw_tpl_test_bad_toml");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(temp_dir.join("broken.toml"), "not = [valid toml {{{}}}").unwrap();

        let valid =
            "name = \"ok\"\ndescription = \"OK agent\"\nsystem_prompt = \"Hi.\"\ntags = []\n";
        fs::write(temp_dir.join("ok.toml"), valid).unwrap();

        let templates = TemplateRegistry::load_from_dir(&temp_dir).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "ok");

        fs::remove_dir_all(&temp_dir).ok();
    }
}
