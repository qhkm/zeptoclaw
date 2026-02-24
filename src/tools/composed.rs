//! Natural language tool composition for ZeptoClaw.
//!
//! Lets users define new tools by describing them in natural language.
//! When a composed tool is called, the action description (with parameters
//! interpolated) is returned to the agent as instructions to follow.
//!
//! # Storage
//!
//! Definitions are persisted at `~/.zeptoclaw/composed_tools.json`.
//!
//! # Example
//!
//! ```json
//! {
//!   "name": "summarize_url",
//!   "description": "Fetch a URL and return a 3-bullet summary",
//!   "action": "Fetch the web page at {{url}} and produce a concise 3-bullet summary",
//!   "parameters": {
//!     "url": { "param_type": "string", "description": "URL to summarize", "required": true }
//!   }
//! }
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A single parameter definition for a composed tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDef {
    /// JSON Schema type: "string", "number", "boolean", "integer".
    #[serde(default = "default_param_type")]
    pub param_type: String,
    /// Human-readable description shown to the LLM.
    #[serde(default)]
    pub description: String,
    /// Whether the parameter is required.
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_param_type() -> String {
    "string".to_string()
}
fn default_true() -> bool {
    true
}

/// Persistent definition of a composed tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposedToolDef {
    /// Unique tool name (snake_case recommended).
    pub name: String,
    /// Description shown to the LLM so it knows when to call this tool.
    pub description: String,
    /// Natural language action template. `{{param}}` placeholders are replaced
    /// with parameter values at execution time.
    pub action: String,
    /// Optional parameter definitions. If empty, the tool takes no arguments.
    #[serde(default)]
    pub parameters: HashMap<String, ParamDef>,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Store (load / save)
// ---------------------------------------------------------------------------

/// Persistence layer for composed tool definitions.
pub struct ComposedToolStore;

impl ComposedToolStore {
    /// Default storage path: `~/.zeptoclaw/composed_tools.json`.
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zeptoclaw")
            .join("composed_tools.json")
    }

    /// Load all definitions from disk. Returns empty vec if file missing.
    pub fn load(path: &PathBuf) -> Result<Vec<ComposedToolDef>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| ZeptoError::Tool(format!("Failed to read composed tools file: {}", e)))?;
        if data.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&data)
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse composed tools file: {}", e)))
    }

    /// Save definitions to disk (atomic via write-then-rename is overkill for
    /// a config file — simple overwrite is fine).
    pub fn save(path: &PathBuf, defs: &[ComposedToolDef]) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ZeptoError::Tool(format!("Failed to create config dir: {}", e)))?;
        }
        let json = serde_json::to_string_pretty(defs)
            .map_err(|e| ZeptoError::Tool(format!("Failed to serialize composed tools: {}", e)))?;
        std::fs::write(path, json)
            .map_err(|e| ZeptoError::Tool(format!("Failed to write composed tools: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

/// Replace `{{key}}` placeholders in an action template with parameter values.
/// Unlike shell-based custom tools, no shell escaping is needed since the output
/// is natural language fed back to the LLM, not a shell command.
fn interpolate_action(template: &str, args: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in args {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

// ---------------------------------------------------------------------------
// ComposedTool — wraps a def and implements Tool
// ---------------------------------------------------------------------------

/// A tool created from a natural language description.
///
/// When executed, interpolates parameters into the action template and returns
/// the result as instructions for the agent to follow using existing tools.
pub struct ComposedTool {
    def: ComposedToolDef,
}

impl ComposedTool {
    /// Create a new composed tool from a definition.
    pub fn new(def: ComposedToolDef) -> Self {
        Self { def }
    }
}

#[async_trait]
impl Tool for ComposedTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn compact_description(&self) -> &str {
        self.description()
    }

    fn category(&self) -> ToolCategory {
        // Composed tools return instructions — they don't directly execute
        // dangerous operations. The agent follows up with real tools that have
        // their own categories. Memory is the safest fit.
        ToolCategory::Memory
    }

    fn parameters(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (name, param) in &self.def.parameters {
            let mut prop = serde_json::Map::new();
            prop.insert("type".to_string(), json!(param.param_type));
            if !param.description.is_empty() {
                prop.insert("description".to_string(), json!(param.description));
            }
            properties.insert(name.clone(), Value::Object(prop));
            if param.required {
                required.push(json!(name));
            }
        }

        json!({
            "type": "object",
            "properties": properties,
            "required": required
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        // Extract string values from args
        let string_args: HashMap<String, String> = if let Some(obj) = args.as_object() {
            obj.iter()
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), val)
                })
                .collect()
        } else {
            HashMap::new()
        };

        let instructions = interpolate_action(&self.def.action, &string_args);

        debug!(
            tool = %self.def.name,
            "Composed tool returning instructions to agent"
        );

        // Return the interpolated action as instructions for the agent.
        // The agent loop will treat this as the tool result and follow the
        // instructions using whatever real tools are available.
        Ok(ToolOutput::llm_only(format!(
            "[Composed tool instructions] {instructions}"
        )))
    }
}

// ---------------------------------------------------------------------------
// CreateToolTool — management tool for composed tools
// ---------------------------------------------------------------------------

/// Agent tool for creating, listing, deleting, and running composed tools.
///
/// Actions:
/// - `create` — define a new composed tool
/// - `list` — list all composed tools
/// - `delete` — remove a composed tool
/// - `run` — execute a composed tool in the current session
pub struct CreateToolTool {
    store_path: PathBuf,
}

impl Default for CreateToolTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateToolTool {
    /// Create with the default store path.
    pub fn new() -> Self {
        Self {
            store_path: ComposedToolStore::default_path(),
        }
    }

    /// Create with a custom store path (useful for testing).
    pub fn with_path(path: PathBuf) -> Self {
        Self { store_path: path }
    }

    fn handle_create(&self, args: &Value) -> Result<ToolOutput> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("'name' is required".into()))?;

        // Validate name: alphanumeric + underscores only
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ZeptoError::Tool(
                "Tool name must be alphanumeric with underscores/hyphens only".into(),
            ));
        }
        if name.is_empty() || name.len() > 64 {
            return Err(ZeptoError::Tool("Tool name must be 1-64 characters".into()));
        }

        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("'description' is required".into()))?;

        // Accept action_template (preferred) or action as the NL instruction
        let action = args
            .get("action_template")
            .or_else(|| args.get("action"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ZeptoError::Tool("'action_template' (or 'action') is required".into())
            })?;

        // Parse optional parameters
        let parameters: HashMap<String, ParamDef> = if let Some(params_val) = args.get("parameters")
        {
            if let Some(obj) = params_val.as_object() {
                obj.iter()
                    .map(|(k, v)| {
                        let param = if v.is_string() {
                            // Shorthand: just a type string
                            ParamDef {
                                param_type: v.as_str().unwrap_or("string").to_string(),
                                description: String::new(),
                                required: true,
                            }
                        } else {
                            // Full object
                            serde_json::from_value(v.clone()).unwrap_or(ParamDef {
                                param_type: "string".to_string(),
                                description: String::new(),
                                required: true,
                            })
                        };
                        (k.clone(), param)
                    })
                    .collect()
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        let mut defs = ComposedToolStore::load(&self.store_path)?;

        // Check uniqueness
        if defs.iter().any(|d| d.name == name) {
            return Err(ZeptoError::Tool(format!(
                "A composed tool named '{}' already exists. Delete it first or choose a different name.",
                name
            )));
        }

        let now = chrono::Utc::now().to_rfc3339();
        let def = ComposedToolDef {
            name: name.to_string(),
            description: description.to_string(),
            action: action.to_string(),
            parameters,
            created_at: now,
        };

        defs.push(def);
        ComposedToolStore::save(&self.store_path, &defs)?;

        info!(tool = %name, "Created composed tool");

        Ok(ToolOutput::user_visible(format!(
            "Created composed tool '{}'. It will be available as a first-class tool in your next session. Use action='run' with name='{}' to try it now.",
            name, name
        )))
    }

    fn handle_list(&self) -> Result<ToolOutput> {
        let defs = ComposedToolStore::load(&self.store_path)?;

        if defs.is_empty() {
            return Ok(ToolOutput::llm_only(
                "No composed tools defined. Use action='create' to define one.",
            ));
        }

        let mut lines = Vec::new();
        for def in &defs {
            let param_names: Vec<&str> = def.parameters.keys().map(|k| k.as_str()).collect();
            let params_str = if param_names.is_empty() {
                "(no params)".to_string()
            } else {
                param_names.join(", ")
            };
            lines.push(format!(
                "- {} — {} [params: {}]",
                def.name, def.description, params_str
            ));
        }

        Ok(ToolOutput::llm_only(format!(
            "Composed tools ({}):\n{}",
            defs.len(),
            lines.join("\n")
        )))
    }

    fn handle_delete(&self, args: &Value) -> Result<ToolOutput> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("'name' is required for delete".into()))?;

        let mut defs = ComposedToolStore::load(&self.store_path)?;
        let before = defs.len();
        defs.retain(|d| d.name != name);

        if defs.len() == before {
            return Err(ZeptoError::Tool(format!(
                "No composed tool named '{}'",
                name
            )));
        }

        ComposedToolStore::save(&self.store_path, &defs)?;
        info!(tool = %name, "Deleted composed tool");

        Ok(ToolOutput::user_visible(format!(
            "Deleted composed tool '{}'.",
            name
        )))
    }

    fn handle_run(&self, args: &Value) -> Result<ToolOutput> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("'name' is required for run".into()))?;

        let defs = ComposedToolStore::load(&self.store_path)?;
        let def = defs
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| ZeptoError::Tool(format!("No composed tool named '{}'", name)))?;

        // Extract params from the args (everything except action/name)
        let string_args: HashMap<String, String> = if let Some(obj) = args.as_object() {
            obj.iter()
                .filter(|(k, _)| *k != "action" && *k != "name")
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), val)
                })
                .collect()
        } else {
            HashMap::new()
        };

        let instructions = interpolate_action(&def.action, &string_args);

        debug!(tool = %name, "Running composed tool via create_tool");

        Ok(ToolOutput::llm_only(format!(
            "[Composed tool instructions] {instructions}"
        )))
    }
}

#[async_trait]
impl Tool for CreateToolTool {
    fn name(&self) -> &str {
        "create_tool"
    }

    fn description(&self) -> &str {
        "Create, list, delete, or run composed tools defined in natural language. \
         Composed tools let you define new capabilities by describing what they do — \
         no code needed. Actions: create, list, delete, run."
    }

    fn compact_description(&self) -> &str {
        "Manage composed tools (create/list/delete/run)"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform: create, list, delete, run",
                    "enum": ["create", "list", "delete", "run"]
                },
                "name": {
                    "type": "string",
                    "description": "Tool name (for create/delete/run). Snake_case recommended."
                },
                "description": {
                    "type": "string",
                    "description": "Tool description for the LLM (for create)."
                },
                "action_template": {
                    "type": "string",
                    "description": "Natural language action with {{param}} placeholders (for create). This is what the agent will execute when the tool is called."
                },
                "parameters": {
                    "type": "object",
                    "description": "Parameter definitions: {\"param_name\": \"type\"} or {\"param_name\": {\"param_type\": \"string\", \"description\": \"...\", \"required\": true}} (for create)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("list")
            .to_string();

        match action.as_str() {
            "create" => self.handle_create(&args),
            "list" => self.handle_list(),
            "delete" => self.handle_delete(&args),
            "run" => self.handle_run(&args),
            other => Err(ZeptoError::Tool(format!(
                "Unknown action '{}'. Use: create, list, delete, run",
                other
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Public helper: load composed tools as Tool objects for agent registration
// ---------------------------------------------------------------------------

/// Load all composed tool definitions from the default path and return
/// them as boxed `Tool` objects ready for registration.
pub fn load_composed_tools() -> Vec<Box<dyn Tool>> {
    load_composed_tools_from(&ComposedToolStore::default_path())
}

/// Load composed tools from a specific path.
pub fn load_composed_tools_from(path: &PathBuf) -> Vec<Box<dyn Tool>> {
    match ComposedToolStore::load(path) {
        Ok(defs) => defs
            .into_iter()
            .map(|def| {
                let name = def.name.clone();
                let tool: Box<dyn Tool> = Box::new(ComposedTool::new(def));
                debug!(tool = %name, "Loaded composed tool");
                tool
            })
            .collect(),
        Err(e) => {
            warn!("Failed to load composed tools: {}", e);
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn temp_store_path() -> PathBuf {
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        // Remove so tests start clean
        let _ = std::fs::remove_file(&path);
        path
    }

    fn test_ctx() -> ToolContext {
        ToolContext::new()
    }

    // === ParamDef ===

    #[test]
    fn test_param_def_defaults() {
        let p: ParamDef = serde_json::from_str("{}").unwrap();
        assert_eq!(p.param_type, "string");
        assert!(p.required);
        assert!(p.description.is_empty());
    }

    #[test]
    fn test_param_def_full() {
        let p: ParamDef = serde_json::from_str(
            r#"{"param_type":"number","description":"Count","required":false}"#,
        )
        .unwrap();
        assert_eq!(p.param_type, "number");
        assert_eq!(p.description, "Count");
        assert!(!p.required);
    }

    // === ComposedToolDef serde ===

    #[test]
    fn test_composed_tool_def_roundtrip() {
        let def = ComposedToolDef {
            name: "test_tool".into(),
            description: "A test".into(),
            action: "Do {{thing}}".into(),
            parameters: HashMap::from([(
                "thing".into(),
                ParamDef {
                    param_type: "string".into(),
                    description: "The thing".into(),
                    required: true,
                },
            )]),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&def).unwrap();
        let back: ComposedToolDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "test_tool");
        assert_eq!(back.parameters.len(), 1);
    }

    // === Store ===

    #[test]
    fn test_store_load_missing_file() {
        let path = PathBuf::from("/tmp/nonexistent_composed_tools_test_12345.json");
        let defs = ComposedToolStore::load(&path).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn test_store_save_and_load() {
        let path = temp_store_path();
        let defs = vec![ComposedToolDef {
            name: "greet".into(),
            description: "Greet someone".into(),
            action: "Say hello to {{name}}".into(),
            parameters: HashMap::from([(
                "name".into(),
                ParamDef {
                    param_type: "string".into(),
                    description: "Person name".into(),
                    required: true,
                },
            )]),
            created_at: "2026-01-01T00:00:00Z".into(),
        }];
        ComposedToolStore::save(&path, &defs).unwrap();
        let loaded = ComposedToolStore::load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "greet");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_store_empty_file() {
        let path = temp_store_path();
        std::fs::write(&path, "").unwrap();
        let defs = ComposedToolStore::load(&path).unwrap();
        assert!(defs.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    // === Interpolation ===

    #[test]
    fn test_interpolate_basic() {
        let args = HashMap::from([("url".into(), "https://example.com".into())]);
        let result = interpolate_action("Fetch {{url}} and summarize", &args);
        assert_eq!(result, "Fetch https://example.com and summarize");
    }

    #[test]
    fn test_interpolate_multiple() {
        let args = HashMap::from([
            ("name".into(), "Alice".into()),
            ("topic".into(), "Rust".into()),
        ]);
        let result = interpolate_action("Tell {{name}} about {{topic}}", &args);
        assert_eq!(result, "Tell Alice about Rust");
    }

    #[test]
    fn test_interpolate_missing_param() {
        let args = HashMap::new();
        let result = interpolate_action("Do {{thing}}", &args);
        assert_eq!(result, "Do {{thing}}");
    }

    #[test]
    fn test_interpolate_no_placeholders() {
        let args = HashMap::from([("unused".into(), "val".into())]);
        let result = interpolate_action("Just do it", &args);
        assert_eq!(result, "Just do it");
    }

    // === ComposedTool ===

    #[test]
    fn test_composed_tool_name() {
        let tool = ComposedTool::new(ComposedToolDef {
            name: "my_tool".into(),
            description: "desc".into(),
            action: "action".into(),
            parameters: HashMap::new(),
            created_at: String::new(),
        });
        assert_eq!(tool.name(), "my_tool");
        assert_eq!(tool.description(), "desc");
    }

    #[test]
    fn test_composed_tool_category() {
        let tool = ComposedTool::new(ComposedToolDef {
            name: "t".into(),
            description: "d".into(),
            action: "a".into(),
            parameters: HashMap::new(),
            created_at: String::new(),
        });
        assert_eq!(tool.category(), ToolCategory::Memory);
    }

    #[test]
    fn test_composed_tool_parameters_empty() {
        let tool = ComposedTool::new(ComposedToolDef {
            name: "t".into(),
            description: "d".into(),
            action: "a".into(),
            parameters: HashMap::new(),
            created_at: String::new(),
        });
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_composed_tool_parameters_with_defs() {
        let tool = ComposedTool::new(ComposedToolDef {
            name: "t".into(),
            description: "d".into(),
            action: "a".into(),
            parameters: HashMap::from([(
                "query".into(),
                ParamDef {
                    param_type: "string".into(),
                    description: "Search query".into(),
                    required: true,
                },
            )]),
            created_at: String::new(),
        });
        let params = tool.parameters();
        let props = params["properties"].as_object().unwrap();
        assert_eq!(props["query"]["type"], "string");
        assert_eq!(props["query"]["description"], "Search query");
        let req = params["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v.as_str() == Some("query")));
    }

    #[tokio::test]
    async fn test_composed_tool_execute() {
        let tool = ComposedTool::new(ComposedToolDef {
            name: "greet".into(),
            description: "Greet someone".into(),
            action: "Say hello to {{name}} in {{language}}".into(),
            parameters: HashMap::from([
                (
                    "name".into(),
                    ParamDef {
                        param_type: "string".into(),
                        description: "".into(),
                        required: true,
                    },
                ),
                (
                    "language".into(),
                    ParamDef {
                        param_type: "string".into(),
                        description: "".into(),
                        required: true,
                    },
                ),
            ]),
            created_at: String::new(),
        });

        let result = tool
            .execute(json!({"name": "Alice", "language": "French"}), &test_ctx())
            .await
            .unwrap();

        assert!(result.for_llm.contains("Say hello to Alice in French"));
        assert!(result.for_llm.contains("[Composed tool instructions]"));
    }

    #[tokio::test]
    async fn test_composed_tool_execute_no_params() {
        let tool = ComposedTool::new(ComposedToolDef {
            name: "daily".into(),
            description: "Daily brief".into(),
            action: "Generate a daily briefing".into(),
            parameters: HashMap::new(),
            created_at: String::new(),
        });
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(result.for_llm.contains("Generate a daily briefing"));
    }

    // === CreateToolTool ===

    #[test]
    fn test_create_tool_tool_name() {
        let tool = CreateToolTool::new();
        assert_eq!(tool.name(), "create_tool");
    }

    #[tokio::test]
    async fn test_create_action() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(
                json!({
                    "action": "create",
                    "name": "test_tool",
                    "description": "A test tool",
                    "action_template": "Do the thing with {{item}}",
                    "parameters": {"item": "string"}
                }),
                &test_ctx(),
            )
            .await
            .unwrap();

        assert!(result.for_llm.contains("Created composed tool"));
        assert!(result.for_llm.contains("test_tool"));

        // Verify persisted
        let defs = ComposedToolStore::load(&path).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "test_tool");
        assert_eq!(defs[0].action, "Do the thing with {{item}}");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_create_duplicate_rejected() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        // Create first
        tool.execute(
            json!({
                "action": "create",
                "name": "dup_tool",
                "description": "d",
                "action_template": "a"
            }),
            &test_ctx(),
        )
        .await
        .unwrap();

        // Create duplicate
        let result = tool
            .execute(
                json!({
                    "action": "create",
                    "name": "dup_tool",
                    "description": "d2",
                    "action_template": "a2"
                }),
                &test_ctx(),
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_create_invalid_name() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(
                json!({
                    "action": "create",
                    "name": "bad name!",
                    "description": "d",
                    "action_template": "a"
                }),
                &test_ctx(),
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("alphanumeric"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_list_empty() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(json!({"action": "list"}), &test_ctx())
            .await
            .unwrap();

        assert!(result.for_llm.contains("No composed tools"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_list_with_tools() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        tool.execute(
            json!({
                "action": "create",
                "name": "tool_a",
                "description": "Tool A",
                "action_template": "Do A"
            }),
            &test_ctx(),
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "list"}), &test_ctx())
            .await
            .unwrap();

        assert!(result.for_llm.contains("tool_a"));
        assert!(result.for_llm.contains("Tool A"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_delete_action() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        tool.execute(
            json!({
                "action": "create",
                "name": "doomed",
                "description": "d",
                "action_template": "a"
            }),
            &test_ctx(),
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "delete", "name": "doomed"}), &test_ctx())
            .await
            .unwrap();

        assert!(result.for_llm.contains("Deleted"));
        let defs = ComposedToolStore::load(&path).unwrap();
        assert!(defs.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(json!({"action": "delete", "name": "ghost"}), &test_ctx())
            .await;

        assert!(result.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_run_action() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        tool.execute(
            json!({
                "action": "create",
                "name": "searcher",
                "description": "Search for things",
                "action_template": "Search the web for {{query}} and summarize top 3 results",
                "parameters": {"query": "string"}
            }),
            &test_ctx(),
        )
        .await
        .unwrap();

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "name": "searcher",
                    "query": "Rust async"
                }),
                &test_ctx(),
            )
            .await
            .unwrap();

        assert!(result
            .for_llm
            .contains("Search the web for Rust async and summarize top 3 results"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_run_nonexistent() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(json!({"action": "run", "name": "missing"}), &test_ctx())
            .await;

        assert!(result.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(json!({"action": "explode"}), &test_ctx())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown action"));
        let _ = std::fs::remove_file(&path);
    }

    // === load_composed_tools_from ===

    #[test]
    fn test_load_composed_tools_empty() {
        let path = temp_store_path();
        let tools = load_composed_tools_from(&path);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_load_composed_tools_with_defs() {
        let path = temp_store_path();
        let defs = vec![
            ComposedToolDef {
                name: "a".into(),
                description: "da".into(),
                action: "do a".into(),
                parameters: HashMap::new(),
                created_at: String::new(),
            },
            ComposedToolDef {
                name: "b".into(),
                description: "db".into(),
                action: "do b".into(),
                parameters: HashMap::new(),
                created_at: String::new(),
            },
        ];
        ComposedToolStore::save(&path, &defs).unwrap();
        let tools = load_composed_tools_from(&path);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name(), "a");
        assert_eq!(tools[1].name(), "b");
        let _ = std::fs::remove_file(&path);
    }

    // === Full parameter object in create ===

    #[tokio::test]
    async fn test_create_with_full_param_objects() {
        let path = temp_store_path();
        let tool = CreateToolTool::with_path(path.clone());

        let result = tool
            .execute(
                json!({
                    "action": "create",
                    "name": "analyzer",
                    "description": "Analyze data",
                    "action_template": "Analyze {{data}} with depth {{depth}}",
                    "parameters": {
                        "data": {
                            "param_type": "string",
                            "description": "Data to analyze",
                            "required": true
                        },
                        "depth": {
                            "param_type": "integer",
                            "description": "Analysis depth",
                            "required": false
                        }
                    }
                }),
                &test_ctx(),
            )
            .await
            .unwrap();

        assert!(result.for_llm.contains("Created"));
        let defs = ComposedToolStore::load(&path).unwrap();
        assert_eq!(defs[0].parameters.len(), 2);
        assert_eq!(defs[0].parameters["data"].param_type, "string");
        assert_eq!(defs[0].parameters["depth"].param_type, "integer");
        assert!(!defs[0].parameters["depth"].required);
        let _ = std::fs::remove_file(&path);
    }
}
