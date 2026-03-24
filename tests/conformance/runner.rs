use serde::Deserialize;
use serde_json::Value;
use tempfile::tempdir;
use tokio::fs;

use zeptoclaw::tools::filesystem::{EditFileTool, ListDirTool, ReadFileTool};
use zeptoclaw::tools::find::FindTool;
use zeptoclaw::tools::grep::GrepTool;
use zeptoclaw::tools::shell::ShellTool;
use zeptoclaw::tools::{Tool, ToolContext};

#[derive(Deserialize)]
pub struct FixtureFile {
    pub tool: String,
    pub cases: Vec<TestCase>,
}

#[derive(Deserialize)]
pub struct TestCase {
    pub name: String,
    #[serde(default)]
    pub setup: Vec<SetupStep>,
    pub input: Value,
    pub expected: Expected,
}

#[derive(Deserialize)]
pub struct SetupStep {
    #[serde(rename = "type")]
    pub step_type: String,
    pub path: String,
    pub content: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Expected {
    pub is_error: Option<bool>,
    pub error_contains: Option<String>,
    pub output_contains: Option<String>,
    pub output_exact: Option<String>,
    pub file_contains: Option<String>,
    pub file_not_contains: Option<String>,
    pub file_exact: Option<String>,
}

fn tool_by_name(name: &str) -> Box<dyn Tool> {
    match name {
        "edit_file" => Box::new(EditFileTool),
        "read_file" => Box::new(ReadFileTool),
        "shell" => Box::new(ShellTool::default()),
        "grep" => Box::new(GrepTool),
        "find" => Box::new(FindTool),
        "list_dir" => Box::new(ListDirTool),
        _ => panic!("Unknown fixture tool: {}", name),
    }
}

pub async fn run_fixture_file(fixture_path: &str) -> Vec<String> {
    let content = std::fs::read_to_string(fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {}", fixture_path, e));
    let fixture: FixtureFile = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse fixture {}: {}", fixture_path, e));

    let tool = tool_by_name(&fixture.tool);
    let mut failures = Vec::new();

    for case in &fixture.cases {
        let case_name = format!("{}::{}", fixture.tool, case.name);
        if let Err(msg) = run_single_case(&*tool, case).await {
            failures.push(format!("{} failed: {}", case_name, msg));
        }
    }

    failures
}

async fn run_single_case(tool: &dyn Tool, case: &TestCase) -> Result<(), String> {
    let dir = tempdir().map_err(|e| format!("tempdir: {}", e))?;
    // Canonicalize to resolve /var -> /private/var on macOS
    let canonical = dir
        .path()
        .canonicalize()
        .map_err(|e| format!("canonicalize: {}", e))?;
    let workspace = canonical.to_str().unwrap();

    // Setup
    for step in &case.setup {
        let full = canonical.join(&step.path);
        match step.step_type.as_str() {
            "create_file" => {
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent)
                        .await
                        .map_err(|e| format!("setup mkdir: {}", e))?;
                }
                let content = step.content.as_deref().unwrap_or("");
                fs::write(&full, content)
                    .await
                    .map_err(|e| format!("setup write: {}", e))?;
            }
            "create_dir" => {
                fs::create_dir_all(&full)
                    .await
                    .map_err(|e| format!("setup mkdir: {}", e))?;
            }
            other => return Err(format!("unknown setup type: {}", other)),
        }
    }

    // Rewrite paths in input to use the actual workspace directory.
    // Fixture JSON uses relative paths; tools expect absolute paths within workspace.
    let input = rewrite_paths(&case.input, workspace);

    let ctx = ToolContext::new().with_workspace(workspace);
    let result = tool.execute(input, &ctx).await;

    // Validate
    let expect_error = case.expected.is_error.unwrap_or(false);

    match (&result, expect_error) {
        (Err(e), true) => {
            if let Some(ref contains) = case.expected.error_contains {
                let msg = format!("{}", e);
                if !msg.contains(contains) {
                    return Err(format!(
                        "error message '{}' does not contain '{}'",
                        msg, contains
                    ));
                }
            }
        }
        (Err(e), false) => {
            return Err(format!("unexpected error: {}", e));
        }
        (Ok(output), true) => {
            if output.is_error {
                if let Some(ref contains) = case.expected.error_contains {
                    if !output.for_llm.contains(contains) {
                        return Err(format!(
                            "error output '{}' does not contain '{}'",
                            output.for_llm, contains
                        ));
                    }
                }
            } else {
                return Err("expected error but tool succeeded".into());
            }
        }
        (Ok(output), false) => {
            if output.is_error {
                return Err(format!("tool returned is_error: {}", output.for_llm));
            }
            if let Some(ref contains) = case.expected.output_contains {
                if !output.for_llm.contains(contains) {
                    return Err(format!(
                        "output '{}' does not contain '{}'",
                        output.for_llm, contains
                    ));
                }
            }
            if let Some(ref exact) = case.expected.output_exact {
                if output.for_llm != *exact {
                    return Err(format!(
                        "output '{}' != expected '{}'",
                        output.for_llm, exact
                    ));
                }
            }
        }
    }

    // File assertions
    if case.expected.file_contains.is_some()
        || case.expected.file_not_contains.is_some()
        || case.expected.file_exact.is_some()
    {
        // Determine the file path to check: prefer "path" key, fall back to "file_path"
        let rel_path = case
            .input
            .get("path")
            .or_else(|| case.input.get("file_path"))
            .and_then(|v| v.as_str());

        let path = rel_path
            .ok_or_else(|| "file_* assertions require input.path or input.file_path".to_string())?;
        let full = canonical.join(path);
        let file_content = fs::read_to_string(&full)
            .await
            .map_err(|e| format!("reading file '{}' for assertion: {}", path, e))?;

        if let Some(ref contains) = case.expected.file_contains {
            if !file_content.contains(contains) {
                return Err(format!("file '{}' does not contain '{}'", path, contains));
            }
        }
        if let Some(ref not_contains) = case.expected.file_not_contains {
            if file_content.contains(not_contains) {
                return Err(format!(
                    "file '{}' should not contain '{}' but does",
                    path, not_contains
                ));
            }
        }
        if let Some(ref exact) = case.expected.file_exact {
            if file_content != *exact {
                return Err(format!(
                    "file '{}' content '{}' != expected '{}'",
                    path, file_content, exact
                ));
            }
        }
    }

    Ok(())
}

/// Rewrite path-like fields in fixture input from relative to absolute.
///
/// Tools like `edit_file`, `read_file`, `grep`, etc. expect absolute paths
/// within the workspace. Fixture JSON uses relative paths for portability.
/// This function prepends the workspace root to known path fields.
fn rewrite_paths(input: &Value, workspace: &str) -> Value {
    let path_keys = ["path", "file_path", "directory"];
    let mut patched = input.clone();

    if let Some(obj) = patched.as_object_mut() {
        for key in &path_keys {
            if let Some(Value::String(val)) = obj.get(*key) {
                if !val.starts_with('/') {
                    let abs = format!("{}/{}", workspace, val);
                    obj.insert((*key).to_string(), Value::String(abs));
                }
            }
        }
    }

    patched
}
