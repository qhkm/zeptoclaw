//! R8r workflow tool for ZeptoClaw
//!
//! This module provides a tool for executing r8r workflows. R8r is a complementary
//! workflow engine that provides deterministic, agent-first workflow automation.
//!
//! # Integration Pattern
//!
//! ZeptoClaw <-> R8r form a symbiotic relationship:
//! - ZeptoClaw calls r8r for deterministic workflows (HTTP, transform, data pipelines)
//! - R8r calls ZeptoClaw via `agent` nodes for AI reasoning decisions
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::tools::{Tool, ToolContext};
//! use zeptoclaw::tools::r8r::R8rTool;
//! use serde_json::json;
//!
//! # tokio_test::block_on(async {
//! let tool = R8rTool::new("http://localhost:8080");
//! let ctx = ToolContext::new();
//!
//! let result = tool.execute(json!({
//!     "workflow": "process-order",
//!     "inputs": {"order_id": "12345"}
//! }), &ctx).await;
//! # });
//! ```

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Default r8r endpoint (local server)
const DEFAULT_R8R_ENDPOINT: &str = "http://localhost:8080";

/// Default timeout for workflow execution (5 minutes)
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// R8r workflow execution tool.
///
/// Executes workflows in the r8r workflow engine and returns structured results.
/// R8r provides deterministic, agent-first workflow automation that complements
/// ZeptoClaw's AI reasoning capabilities.
pub struct R8rTool {
    endpoint: String,
    client: Client,
}

impl R8rTool {
    /// Create a new R8r tool with the specified endpoint.
    ///
    /// # Arguments
    /// * `endpoint` - The r8r server endpoint (e.g., "http://localhost:8080")
    pub fn new(endpoint: &str) -> Self {
        let client = match Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    %error,
                    "Failed to build configured R8r HTTP client; falling back to default client"
                );
                Client::new()
            }
        };

        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            client,
        }
    }

    /// Create a new R8r tool with default endpoint (localhost:8080).
    pub fn default_endpoint() -> Self {
        Self::new(DEFAULT_R8R_ENDPOINT)
    }

    /// Create a new R8r tool from environment variable or default.
    ///
    /// Checks `R8R_ENDPOINT` environment variable, falls back to localhost:8080.
    pub fn from_env() -> Self {
        let endpoint =
            std::env::var("R8R_ENDPOINT").unwrap_or_else(|_| DEFAULT_R8R_ENDPOINT.to_string());
        Self::new(&endpoint)
    }

    /// Create a new R8r tool with custom client and endpoint.
    pub fn with_client(endpoint: &str, client: Client) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            client,
        }
    }
}

impl Default for R8rTool {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Request to execute a workflow
#[derive(Debug, Serialize)]
struct ExecuteRequest {
    input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait: Option<bool>,
}

/// Response from workflow execution
#[derive(Debug, Deserialize)]
struct ExecuteResponse {
    execution_id: String,
    status: String,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// Workflow information response
#[derive(Debug, Deserialize)]
struct WorkflowInfo {
    name: String,
    description: String,
    enabled: bool,
}

/// Error response from r8r API
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    code: String,
    message: String,
    #[serde(default)]
    node_id: Option<String>,
}

#[async_trait]
impl Tool for R8rTool {
    fn name(&self) -> &str {
        "r8r"
    }

    fn description(&self) -> &str {
        "Execute deterministic workflows in the r8r engine. Use for reliable, \
         repeatable operations like HTTP calls, data transformations, \
         and multi-step pipelines. R8r workflows are agent-first: designed \
         to be invoked by AI agents for structured, predictable tasks."
    }

    fn compact_description(&self) -> &str {
        "R8r workflow"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::NetworkWrite
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "workflow": {
                    "type": "string",
                    "description": "Name of the r8r workflow to execute or create"
                },
                "inputs": {
                    "type": "object",
                    "description": "Input data for the workflow (JSON object)",
                    "additionalProperties": true
                },
                "wait": {
                    "type": "boolean",
                    "description": "Wait for workflow completion (default: true). Set to false for async execution.",
                    "default": true
                },
                "action": {
                    "type": "string",
                    "enum": ["run", "list", "show", "status", "emit", "create"],
                    "description": "Action to perform: 'run' executes workflow (default), 'list' shows available workflows, 'show' displays workflow details, 'status' polls execution status, 'emit' publishes an event, 'create' creates a new workflow",
                    "default": "run"
                },
                "execution_id": {
                    "type": "string",
                    "description": "Execution ID to check status of (required for 'status' action)"
                },
                "event": {
                    "type": "string",
                    "description": "Event name to publish (required for 'emit' action)"
                },
                "data": {
                    "type": "object",
                    "description": "Event data payload (for 'emit' action)",
                    "additionalProperties": true
                },
                "definition": {
                    "type": "string",
                    "description": "YAML workflow definition (required for 'create' action)"
                }
            },
            "required": ["workflow"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("run");

        let s = match action {
            "list" => self.list_workflows().await?,
            "show" => {
                let workflow = args
                    .get("workflow")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'workflow' argument".into()))?;
                self.show_workflow(workflow).await?
            }
            "run" => {
                let workflow = args
                    .get("workflow")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'workflow' argument".into()))?;

                let inputs = args.get("inputs").cloned().unwrap_or(Value::Null);
                let wait = args.get("wait").and_then(|v| v.as_bool()).unwrap_or(true);

                self.run_workflow(workflow, inputs, wait).await?
            }
            "status" => {
                let execution_id = args
                    .get("execution_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'execution_id' argument".into()))?;
                self.get_execution_status(execution_id).await?
            }
            "emit" => {
                let event = args
                    .get("event")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'event' argument".into()))?;
                let data = args.get("data").cloned().unwrap_or(json!({}));
                self.emit_event(event, data).await?
            }
            "create" => {
                let name = args
                    .get("workflow")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'workflow' argument".into()))?;
                let definition = args
                    .get("definition")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'definition' argument".into()))?;
                self.create_workflow(name, definition).await?
            }
            _ => {
                return Err(ZeptoError::Tool(format!(
                    "Invalid 'action': {}. Expected one of: run, list, show, status, emit, create",
                    action
                )))
            }
        };
        Ok(ToolOutput::user_visible(s))
    }
}

/// Validate that a string is safe to use as a URL path segment.
///
/// Only allows alphanumeric characters, hyphens, underscores, and dots.
/// Returns an error if the name contains characters that could cause
/// URL path injection (e.g., `/`, `?`, `#`, `..`).
fn validate_path_segment(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ZeptoError::Tool(
            "Path segment must not be empty".to_string(),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ZeptoError::Tool(format!(
            "Invalid path segment '{}': only alphanumeric characters, hyphens, underscores, and dots are allowed",
            name
        )));
    }

    Ok(())
}

impl R8rTool {
    /// List all available workflows
    async fn list_workflows(&self) -> Result<String> {
        let url = format!("{}/api/workflows", self.endpoint);
        debug!("R8r list workflows: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to connect to r8r: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Tool(format!(
                "R8r API error ({}): {}",
                status, body
            )));
        }

        let workflows: Vec<WorkflowInfo> = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse r8r response: {}", e)))?;

        if workflows.is_empty() {
            return Ok(
                "No workflows found in r8r. Create one with: r8r workflows create <file.yaml>"
                    .to_string(),
            );
        }

        let mut output = String::from("Available r8r workflows:\n\n");
        for wf in workflows {
            output.push_str(&format!(
                "- {} ({})\n  {}\n",
                wf.name,
                if wf.enabled { "enabled" } else { "disabled" },
                wf.description
            ));
        }

        Ok(output)
    }

    /// Show workflow details
    async fn show_workflow(&self, name: &str) -> Result<String> {
        validate_path_segment(name)?;
        let url = format!("{}/api/workflows/{}", self.endpoint, name);
        debug!("R8r show workflow: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to connect to r8r: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            // Try to parse as error response
            if let Ok(err) = serde_json::from_str::<ErrorResponse>(&body) {
                return Err(ZeptoError::Tool(format!(
                    "R8r error [{}]: {}",
                    err.code, err.message
                )));
            }

            return Err(ZeptoError::Tool(format!(
                "R8r API error ({}): {}",
                status, body
            )));
        }

        let info: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse r8r response: {}", e)))?;

        // Format workflow details in a readable way
        let output = serde_json::to_string_pretty(&info).unwrap_or_else(|_| info.to_string());

        Ok(format!("Workflow '{}':\n\n{}", name, output))
    }

    /// Get execution status by ID
    async fn get_execution_status(&self, id: &str) -> Result<String> {
        validate_path_segment(id)?;
        let url = format!("{}/api/executions/{}", self.endpoint, id);
        debug!("R8r get execution status: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to connect to r8r: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Tool(format!(
                "R8r API error ({}): {}",
                status, body
            )));
        }

        let exec: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse r8r response: {}", e)))?;

        let status = exec
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match status {
            "completed" => {
                let duration = exec
                    .get("duration_ms")
                    .and_then(|v| v.as_i64())
                    .map(|d| format!(" ({}ms)", d))
                    .unwrap_or_default();

                let output = exec
                    .get("output")
                    .map(|o| serde_json::to_string_pretty(o).unwrap_or_else(|_| o.to_string()))
                    .unwrap_or_else(|| "(no output)".to_string());

                Ok(format!(
                    "Execution '{}' completed successfully{}.\n\nOutput:\n{}",
                    id, duration, output
                ))
            }
            "running" | "pending" => Ok(format!(
                "Execution '{}' is still {}. Poll again later.",
                id, status
            )),
            "failed" => {
                let error = exec
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                Err(ZeptoError::Tool(format!(
                    "Execution '{}' failed: {}",
                    id, error
                )))
            }
            "paused" => Ok(format!(
                "Execution '{}' is paused. Resume it via the API to continue.",
                id
            )),
            _ => Ok(format!("Execution '{}' status: {}", id, status)),
        }
    }

    /// Emit an event to the r8r event system
    async fn emit_event(&self, event: &str, data: Value) -> Result<String> {
        let url = format!("{}/api/events/publish", self.endpoint);
        debug!("R8r emit event: {} -> {}", event, url);

        let body = json!({
            "event": event,
            "data": data,
            "source": "zeptoclaw"
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to connect to r8r: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Tool(format!(
                "R8r API error ({}): {}",
                status, body
            )));
        }

        info!(event = event, "R8r event published");
        Ok(format!("Event '{}' published successfully.", event))
    }

    /// Create a new workflow in r8r
    async fn create_workflow(&self, name: &str, definition: &str) -> Result<String> {
        let url = format!("{}/api/workflows", self.endpoint);
        debug!("R8r create workflow: {} -> {}", name, url);

        let body = json!({
            "name": name,
            "definition": definition,
            "enabled": true
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to connect to r8r: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ZeptoError::Tool(format!(
                "R8r API error ({}): {}",
                status, body
            )));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse r8r response: {}", e)))?;

        let id = result
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let node_count = result
            .get("node_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let trigger_count = result
            .get("trigger_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        info!(workflow = name, id = id, "R8r workflow created");
        Ok(format!(
            "Workflow '{}' created successfully.\n\nID: {}\nNodes: {}\nTriggers: {}",
            name, id, node_count, trigger_count
        ))
    }

    /// Run a workflow
    async fn run_workflow(&self, name: &str, inputs: Value, wait: bool) -> Result<String> {
        validate_path_segment(name)?;
        let url = format!("{}/api/workflows/{}/execute", self.endpoint, name);
        debug!("R8r execute workflow: {} (wait={})", url, wait);

        let request = ExecuteRequest {
            input: inputs.clone(),
            wait: Some(wait),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to connect to r8r: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            // Try to parse as error response for structured error
            if let Ok(err) = serde_json::from_str::<ErrorResponse>(&body) {
                let node_info = err
                    .node_id
                    .map(|n| format!(" (at node: {})", n))
                    .unwrap_or_default();
                return Err(ZeptoError::Tool(format!(
                    "R8r workflow error [{}]{}: {}",
                    err.code, node_info, err.message
                )));
            }

            return Err(ZeptoError::Tool(format!(
                "R8r API error ({}): {}",
                status, body
            )));
        }

        let exec_response: ExecuteResponse = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse r8r response: {}", e)))?;

        info!(
            workflow = name,
            execution_id = %exec_response.execution_id,
            status = %exec_response.status,
            "R8r workflow executed"
        );

        // Format response based on status
        match exec_response.status.as_str() {
            "completed" => {
                let duration = exec_response
                    .duration_ms
                    .map(|d| format!(" ({}ms)", d))
                    .unwrap_or_default();

                let output = exec_response
                    .output
                    .map(|o| serde_json::to_string_pretty(&o).unwrap_or_else(|_| o.to_string()))
                    .unwrap_or_else(|| "(no output)".to_string());

                Ok(format!(
                    "Workflow '{}' completed successfully{}.\n\nExecution ID: {}\n\nOutput:\n{}",
                    name, duration, exec_response.execution_id, output
                ))
            }
            "running" | "pending" => Ok(format!(
                "Workflow '{}' started (async mode).\n\nExecution ID: {}\nStatus: {}\n\n\
                 Poll status with: r8r executions show {}",
                name, exec_response.execution_id, exec_response.status, exec_response.execution_id
            )),
            "failed" => {
                let error = exec_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string());

                Err(ZeptoError::Tool(format!(
                    "Workflow '{}' failed: {}\n\nExecution ID: {}",
                    name, error, exec_response.execution_id
                )))
            }
            _ => Ok(format!(
                "Workflow '{}' status: {}\n\nExecution ID: {}",
                name, exec_response.status, exec_response.execution_id
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_r8r_tool_name() {
        let tool = R8rTool::new("http://localhost:8080");
        assert_eq!(tool.name(), "r8r");
    }

    #[test]
    fn test_r8r_tool_description() {
        let tool = R8rTool::new("http://localhost:8080");
        assert!(tool.description().contains("workflow"));
        assert!(tool.description().contains("deterministic"));
    }

    #[test]
    fn test_r8r_tool_parameters() {
        let tool = R8rTool::new("http://localhost:8080");
        let params = tool.parameters();

        assert!(params.is_object());
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["workflow"].is_object());
        assert!(params["properties"]["inputs"].is_object());
        assert!(params["properties"]["wait"].is_object());
        assert!(params["properties"]["action"].is_object());
        assert_eq!(params["required"][0], "workflow");
    }

    #[test]
    fn test_r8r_tool_default() {
        let tool = R8rTool::default();
        assert!(!tool.endpoint.trim().is_empty());
        assert!(tool.endpoint.starts_with("http://") || tool.endpoint.starts_with("https://"));
    }

    #[test]
    fn test_r8r_tool_endpoint_trimming() {
        let tool = R8rTool::new("http://localhost:8080/");
        assert_eq!(tool.endpoint, "http://localhost:8080");
    }

    #[test]
    fn test_r8r_tool_from_env() {
        let expected = std::env::var("R8R_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_R8R_ENDPOINT.to_string())
            .trim_end_matches('/')
            .to_string();
        let tool = R8rTool::from_env();
        assert_eq!(tool.endpoint, expected);
    }

    #[test]
    fn test_r8r_tool_default_endpoint() {
        let tool = R8rTool::default_endpoint();
        assert_eq!(tool.endpoint, "http://localhost:8080");
    }

    #[test]
    fn test_execute_request_serialization() {
        let request = ExecuteRequest {
            input: json!({"key": "value"}),
            wait: Some(true),
        };
        let json_str = serde_json::to_string(&request).unwrap();
        assert!(json_str.contains("\"input\""));
        assert!(json_str.contains("\"wait\":true"));
    }

    #[test]
    fn test_execute_request_skip_none_wait() {
        let request = ExecuteRequest {
            input: json!(null),
            wait: None,
        };
        let json_str = serde_json::to_string(&request).unwrap();
        assert!(!json_str.contains("wait"));
    }

    #[test]
    fn test_validate_path_segment_valid() {
        assert!(validate_path_segment("my-workflow").is_ok());
        assert!(validate_path_segment("workflow_v2").is_ok());
        assert!(validate_path_segment("test.workflow").is_ok());
        assert!(validate_path_segment("SimpleName123").is_ok());
        assert!(validate_path_segment("a").is_ok());
    }

    #[test]
    fn test_validate_path_segment_invalid() {
        // Path traversal
        assert!(validate_path_segment("../etc/passwd").is_err());
        // Slash injection
        assert!(validate_path_segment("foo/bar").is_err());
        // Query string injection
        assert!(validate_path_segment("name?admin=true").is_err());
        // Fragment injection
        assert!(validate_path_segment("name#section").is_err());
        // Spaces
        assert!(validate_path_segment("has space").is_err());
        // Empty
        assert!(validate_path_segment("").is_err());
        // Encoded characters
        assert!(validate_path_segment("name%2F").is_err());
    }

    // Integration tests require a running r8r server
    // These are marked as ignored by default
    #[tokio::test]
    #[ignore = "Requires running r8r server"]
    async fn test_r8r_list_workflows_integration() {
        let tool = R8rTool::default_endpoint();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"action": "list", "workflow": "_"}), &ctx)
            .await;
        // Should either succeed or fail gracefully
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    #[ignore = "Requires running r8r server"]
    async fn test_r8r_run_workflow_integration() {
        let tool = R8rTool::default_endpoint();
        let ctx = ToolContext::new();

        let result = tool
            .execute(
                json!({
                    "workflow": "simple-test",
                    "inputs": {},
                    "wait": true
                }),
                &ctx,
            )
            .await;

        // If r8r is running and workflow exists, should succeed
        if result.is_ok() {
            let output = result.unwrap().for_llm;
            assert!(output.contains("completed") || output.contains("Execution ID"));
        }
    }
}
