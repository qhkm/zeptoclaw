//! Project management tool for ZeptoClaw.
//!
//! Provides a unified interface for managing issues across GitHub, Jira, and Linear.
//!
//! # Supported actions
//!
//! - `list_issues`   — list open issues in a project
//! - `get_issue`     — fetch a single issue by ID
//! - `create_issue`  — create a new issue
//! - `update_issue`  — update title, description, status, assignee, or labels
//! - `search`        — search issues by query / JQL
//! - `transitions`   — list available transitions for an issue (Jira only)
//!
//! # Backends
//!
//! | Backend  | Auth header           | API base                                          |
//! |----------|-----------------------|---------------------------------------------------|
//! | `github` | `Bearer {TOKEN}`      | `https://api.github.com/repos/{owner}/{repo}/...` |
//! | `jira`   | `Basic {TOKEN}`       | `{JIRA_URL}/rest/api/3/...`                       |
//! | `linear` | `{LINEAR_API_KEY}`    | `https://api.linear.app/graphql`                  |

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::config::{ProjectBackend, ProjectConfig};
use crate::error::{Result, ZeptoError};

use super::{Tool, ToolContext, ToolOutput};

const DEFAULT_LIMIT: u64 = 10;

/// Tool for project management (GitHub Issues, Jira, Linear).
#[derive(Debug)]
pub struct ProjectTool {
    client: Client,
    config: ProjectConfig,
}

impl ProjectTool {
    /// Create a new ProjectTool from a `ProjectConfig`.
    pub fn new(config: ProjectConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Resolve the project key: prefer explicit `project` arg, fall back to default.
    fn resolve_project<'a>(&'a self, args: &'a Value) -> Result<&'a str> {
        if let Some(p) = args.get("project").and_then(Value::as_str) {
            let p = p.trim();
            if !p.is_empty() {
                return Ok(p);
            }
        }
        let default = self.config.default_project.trim();
        if default.is_empty() {
            return Err(ZeptoError::Tool(
                "No project specified and no default_project configured".to_string(),
            ));
        }
        Ok(default)
    }

    /// Build the Authorization header value for the configured backend.
    fn auth_header(&self) -> Result<String> {
        match self.config.backend {
            ProjectBackend::Github => {
                let token = self
                    .config
                    .github_token
                    .as_deref()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "GitHub token is not configured (project.github_token)".to_string(),
                        )
                    })?;
                Ok(format!("Bearer {}", token.trim()))
            }
            ProjectBackend::Jira => {
                let token = self
                    .config
                    .jira_token
                    .as_deref()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Jira token is not configured (project.jira_token)".to_string(),
                        )
                    })?;
                Ok(format!("Basic {}", token.trim()))
            }
            ProjectBackend::Linear => {
                let key = self
                    .config
                    .linear_api_key
                    .as_deref()
                    .filter(|k| !k.trim().is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Linear API key is not configured (project.linear_api_key)".to_string(),
                        )
                    })?;
                Ok(key.trim().to_string())
            }
        }
    }

    // -------------------------------------------------------------------------
    // GitHub backend
    // -------------------------------------------------------------------------

    async fn github_list_issues(&self, repo: &str, limit: u64) -> Result<String> {
        let url = format!("https://api.github.com/repos/{}/issues", repo);
        let auth = self.auth_header()?;
        let response = self
            .client
            .get(&url)
            .header("Authorization", auth)
            .header("User-Agent", "zeptoclaw")
            .header("Accept", "application/vnd.github+json")
            .query(&[("per_page", limit.to_string().as_str()), ("state", "open")])
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("GitHub request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid GitHub response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "GitHub API error {}: {}",
                status, body
            )));
        }

        let issues = body.as_array().cloned().unwrap_or_default();
        if issues.is_empty() {
            return Ok("No open issues found.".to_string());
        }
        let lines: Vec<String> = issues
            .iter()
            .map(|issue| {
                let number = issue["number"].as_u64().unwrap_or(0);
                let title = issue["title"].as_str().unwrap_or("(no title)");
                let state = issue["state"].as_str().unwrap_or("?");
                format!("#{} [{}] {}", number, state, title)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    async fn github_get_issue(&self, repo: &str, issue_id: &str) -> Result<String> {
        let url = format!("https://api.github.com/repos/{}/issues/{}", repo, issue_id);
        let auth = self.auth_header()?;
        let response = self
            .client
            .get(&url)
            .header("Authorization", auth)
            .header("User-Agent", "zeptoclaw")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("GitHub request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid GitHub response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "GitHub API error {}: {}",
                status, body
            )));
        }

        let number = body["number"].as_u64().unwrap_or(0);
        let title = body["title"].as_str().unwrap_or("(no title)");
        let state = body["state"].as_str().unwrap_or("?");
        let body_text = body["body"].as_str().unwrap_or("(no description)");
        let assignee = body["assignee"]["login"].as_str().unwrap_or("unassigned");

        Ok(format!(
            "Issue #{}: {}\nState: {}\nAssignee: {}\n\n{}",
            number, title, state, assignee, body_text
        ))
    }

    async fn github_create_issue(
        &self,
        repo: &str,
        title: &str,
        description: Option<&str>,
        labels: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<String> {
        let url = format!("https://api.github.com/repos/{}/issues", repo);
        let auth = self.auth_header()?;

        let mut payload = json!({ "title": title });
        if let Some(body) = description {
            payload["body"] = json!(body);
        }
        if let Some(lbls) = labels {
            let label_vec: Vec<&str> = lbls.split(',').map(str::trim).collect();
            payload["labels"] = json!(label_vec);
        }
        if let Some(a) = assignee {
            payload["assignees"] = json!([a]);
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", auth)
            .header("User-Agent", "zeptoclaw")
            .header("Accept", "application/vnd.github+json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("GitHub request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid GitHub response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "GitHub API error {}: {}",
                status, body
            )));
        }

        let number = body["number"].as_u64().unwrap_or(0);
        let html_url = body["html_url"].as_str().unwrap_or("(no url)");
        Ok(format!("Created issue #{}: {}", number, html_url))
    }

    #[allow(clippy::too_many_arguments)]
    async fn github_update_issue(
        &self,
        repo: &str,
        issue_id: &str,
        title: Option<&str>,
        description: Option<&str>,
        state: Option<&str>,
        labels: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<String> {
        let url = format!("https://api.github.com/repos/{}/issues/{}", repo, issue_id);
        let auth = self.auth_header()?;

        let mut payload = json!({});
        if let Some(t) = title {
            payload["title"] = json!(t);
        }
        if let Some(b) = description {
            payload["body"] = json!(b);
        }
        if let Some(s) = state {
            payload["state"] = json!(s);
        }
        if let Some(lbls) = labels {
            let label_vec: Vec<&str> = lbls.split(',').map(str::trim).collect();
            payload["labels"] = json!(label_vec);
        }
        if let Some(a) = assignee {
            payload["assignees"] = json!([a]);
        }

        let response = self
            .client
            .patch(&url)
            .header("Authorization", auth)
            .header("User-Agent", "zeptoclaw")
            .header("Accept", "application/vnd.github+json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("GitHub request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid GitHub response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "GitHub API error {}: {}",
                status, body
            )));
        }

        let number = body["number"].as_u64().unwrap_or(0);
        let new_state = body["state"].as_str().unwrap_or("?");
        Ok(format!("Updated issue #{} (state: {})", number, new_state))
    }

    async fn github_search(&self, repo: &str, query: &str, limit: u64) -> Result<String> {
        let auth = self.auth_header()?;
        let full_query = format!("repo:{} {}", repo, query);
        let response = self
            .client
            .get("https://api.github.com/search/issues")
            .header("Authorization", auth)
            .header("User-Agent", "zeptoclaw")
            .header("Accept", "application/vnd.github+json")
            .query(&[("q", full_query.as_str()), ("per_page", &limit.to_string())])
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("GitHub request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid GitHub response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "GitHub API error {}: {}",
                status, body
            )));
        }

        let items = body["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            return Ok(format!("No issues found matching '{}'.", query));
        }
        let lines: Vec<String> = items
            .iter()
            .map(|issue| {
                let number = issue["number"].as_u64().unwrap_or(0);
                let title = issue["title"].as_str().unwrap_or("(no title)");
                let state = issue["state"].as_str().unwrap_or("?");
                format!("#{} [{}] {}", number, state, title)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    // -------------------------------------------------------------------------
    // Jira backend
    // -------------------------------------------------------------------------

    fn jira_base(&self) -> Result<String> {
        let url = self.config.jira_url.trim();
        if url.is_empty() {
            return Err(ZeptoError::Tool(
                "Jira URL is not configured (project.jira_url)".to_string(),
            ));
        }
        Ok(url.trim_end_matches('/').to_string())
    }

    async fn jira_list_issues(&self, project: &str, limit: u64) -> Result<String> {
        let base = self.jira_base()?;
        let auth = self.auth_header()?;
        let jql = format!("project = {} ORDER BY created DESC", project);
        let url = format!("{}/rest/api/3/search", base);

        let payload = json!({
            "jql": jql,
            "maxResults": limit,
            "fields": ["summary", "status", "assignee"]
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Jira request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Jira response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Jira API error {}: {}",
                status, body
            )));
        }

        let issues = body["issues"].as_array().cloned().unwrap_or_default();
        if issues.is_empty() {
            return Ok("No issues found.".to_string());
        }
        let lines: Vec<String> = issues
            .iter()
            .map(|issue| {
                let key = issue["key"].as_str().unwrap_or("?");
                let summary = issue["fields"]["summary"].as_str().unwrap_or("(no title)");
                let status_name = issue["fields"]["status"]["name"]
                    .as_str()
                    .unwrap_or("unknown");
                format!("{} [{}] {}", key, status_name, summary)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    async fn jira_get_issue(&self, issue_id: &str) -> Result<String> {
        let base = self.jira_base()?;
        let auth = self.auth_header()?;
        let url = format!("{}/rest/api/3/issue/{}", base, issue_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", auth)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Jira request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Jira response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Jira API error {}: {}",
                status, body
            )));
        }

        let key = body["key"].as_str().unwrap_or("?");
        let summary = body["fields"]["summary"].as_str().unwrap_or("(no title)");
        let status_name = body["fields"]["status"]["name"]
            .as_str()
            .unwrap_or("unknown");
        let description = body["fields"]["description"]
            .as_str()
            .unwrap_or("(no description)");
        let assignee = body["fields"]["assignee"]["displayName"]
            .as_str()
            .unwrap_or("unassigned");

        Ok(format!(
            "Issue {}: {}\nStatus: {}\nAssignee: {}\n\n{}",
            key, summary, status_name, assignee, description
        ))
    }

    async fn jira_create_issue(
        &self,
        project: &str,
        title: &str,
        description: Option<&str>,
        labels: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<String> {
        let base = self.jira_base()?;
        let auth = self.auth_header()?;
        let url = format!("{}/rest/api/3/issue", base);

        let mut fields = json!({
            "project": { "key": project },
            "summary": title,
            "issuetype": { "name": "Task" }
        });

        if let Some(desc) = description {
            // Jira API v3 uses Atlassian Document Format for description.
            // We store plain text in a paragraph node.
            fields["description"] = json!({
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": desc }]
                }]
            });
        }
        if let Some(lbls) = labels {
            let label_vec: Vec<&str> = lbls.split(',').map(str::trim).collect();
            fields["labels"] = json!(label_vec);
        }
        if let Some(a) = assignee {
            fields["assignee"] = json!({ "name": a });
        }

        let payload = json!({ "fields": fields });
        let response = self
            .client
            .post(&url)
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Jira request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Jira response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Jira API error {}: {}",
                status, body
            )));
        }

        let key = body["key"].as_str().unwrap_or("?");
        Ok(format!("Created Jira issue: {}", key))
    }

    async fn jira_update_issue(
        &self,
        issue_id: &str,
        title: Option<&str>,
        description: Option<&str>,
        _status: Option<&str>,
        labels: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<String> {
        let base = self.jira_base()?;
        let auth = self.auth_header()?;
        let url = format!("{}/rest/api/3/issue/{}", base, issue_id);

        let mut fields = json!({});
        if let Some(t) = title {
            fields["summary"] = json!(t);
        }
        if let Some(desc) = description {
            fields["description"] = json!({
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": desc }]
                }]
            });
        }
        if let Some(lbls) = labels {
            let label_vec: Vec<&str> = lbls.split(',').map(str::trim).collect();
            fields["labels"] = json!(label_vec);
        }
        if let Some(a) = assignee {
            fields["assignee"] = json!({ "name": a });
        }

        let payload = json!({ "fields": fields });
        let response = self
            .client
            .put(&url)
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Jira request failed: {}", e)))?;

        let status = response.status();
        // Jira PUT /issue returns 204 No Content on success.
        if status.as_u16() == 204 {
            return Ok(format!("Updated Jira issue: {}", issue_id));
        }

        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Jira response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Jira API error {}: {}",
                status, body
            )));
        }
        Ok(format!("Updated Jira issue: {}", issue_id))
    }

    async fn jira_search(&self, query: &str, limit: u64) -> Result<String> {
        let base = self.jira_base()?;
        let auth = self.auth_header()?;
        let url = format!("{}/rest/api/3/search", base);

        let payload = json!({
            "jql": query,
            "maxResults": limit,
            "fields": ["summary", "status", "assignee"]
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Jira request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Jira response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Jira API error {}: {}",
                status, body
            )));
        }

        let issues = body["issues"].as_array().cloned().unwrap_or_default();
        if issues.is_empty() {
            return Ok(format!("No issues found for JQL: '{}'.", query));
        }
        let lines: Vec<String> = issues
            .iter()
            .map(|issue| {
                let key = issue["key"].as_str().unwrap_or("?");
                let summary = issue["fields"]["summary"].as_str().unwrap_or("(no title)");
                let status_name = issue["fields"]["status"]["name"]
                    .as_str()
                    .unwrap_or("unknown");
                format!("{} [{}] {}", key, status_name, summary)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    async fn jira_transitions(&self, issue_id: &str) -> Result<String> {
        let base = self.jira_base()?;
        let auth = self.auth_header()?;
        let url = format!("{}/rest/api/3/issue/{}/transitions", base, issue_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", auth)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Jira request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Jira response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Jira API error {}: {}",
                status, body
            )));
        }

        let transitions = body["transitions"].as_array().cloned().unwrap_or_default();
        if transitions.is_empty() {
            return Ok(format!("No transitions available for issue {}.", issue_id));
        }
        let lines: Vec<String> = transitions
            .iter()
            .map(|t| {
                let id = t["id"].as_str().unwrap_or("?");
                let name = t["name"].as_str().unwrap_or("?");
                format!("[{}] {}", id, name)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    // -------------------------------------------------------------------------
    // Linear backend
    // -------------------------------------------------------------------------

    async fn linear_list_issues(&self, limit: u64) -> Result<String> {
        let auth = self.auth_header()?;
        let query = format!(
            r#"{{ issues(first: {}) {{ nodes {{ id identifier title state {{ name }} assignee {{ name }} }} }} }}"#,
            limit
        );
        let payload = json!({ "query": query });

        let response = self
            .client
            .post("https://api.linear.app/graphql")
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Linear request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Linear response: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Linear API error {}: {}",
                status, body
            )));
        }

        if let Some(errors) = body["errors"].as_array() {
            if !errors.is_empty() {
                return Err(ZeptoError::Tool(format!(
                    "Linear GraphQL error: {}",
                    errors[0]["message"].as_str().unwrap_or("unknown error")
                )));
            }
        }

        let nodes = body["data"]["issues"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if nodes.is_empty() {
            return Ok("No issues found.".to_string());
        }
        let lines: Vec<String> = nodes
            .iter()
            .map(|node| {
                let identifier = node["identifier"].as_str().unwrap_or("?");
                let title = node["title"].as_str().unwrap_or("(no title)");
                let state = node["state"]["name"].as_str().unwrap_or("?");
                format!("{} [{}] {}", identifier, state, title)
            })
            .collect();
        Ok(lines.join("\n"))
    }
}

#[async_trait]
impl Tool for ProjectTool {
    fn name(&self) -> &str {
        "project"
    }

    fn description(&self) -> &str {
        "Manage issues on GitHub, Jira, or Linear (list_issues, get_issue, create_issue, update_issue, search, transitions)."
    }

    fn compact_description(&self) -> &str {
        "Project issue management (GitHub/Jira/Linear)"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_issues", "get_issue", "create_issue", "update_issue", "search", "transitions"],
                    "description": "Action to perform."
                },
                "project": {
                    "type": "string",
                    "description": "Project key or repo (e.g., 'owner/repo' for GitHub, 'PROJ' for Jira). Defaults to config default_project."
                },
                "issue_id": {
                    "type": "string",
                    "description": "Issue ID or number for get_issue, update_issue, and transitions."
                },
                "title": {
                    "type": "string",
                    "description": "Issue title for create_issue or update_issue."
                },
                "description": {
                    "type": "string",
                    "description": "Issue description/body for create_issue or update_issue."
                },
                "status": {
                    "type": "string",
                    "description": "Issue status to set (e.g., 'open', 'closed') for update_issue."
                },
                "query": {
                    "type": "string",
                    "description": "Search query or JQL string for the search action."
                },
                "labels": {
                    "type": "string",
                    "description": "Comma-separated label names for create_issue or update_issue."
                },
                "assignee": {
                    "type": "string",
                    "description": "Username or account ID to assign the issue to."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 10).",
                    "minimum": 1,
                    "maximum": 100
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".to_string()))?;

        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, 100);

        // `transitions` is Jira-only
        if action == "transitions" {
            if self.config.backend != ProjectBackend::Jira {
                return Err(ZeptoError::Tool(
                    "'transitions' action is only supported for the Jira backend".to_string(),
                ));
            }
            let issue_id = args
                .get("issue_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ZeptoError::Tool("'issue_id' is required for transitions".to_string())
                })?;
            return self
                .jira_transitions(issue_id)
                .await
                .map(ToolOutput::llm_only);
        }

        match self.config.backend {
            ProjectBackend::Github => self.execute_github(action, &args, limit).await,
            ProjectBackend::Jira => self.execute_jira(action, &args, limit).await,
            ProjectBackend::Linear => self.execute_linear(action, &args, limit).await,
        }
        .map(ToolOutput::llm_only)
    }
}

impl ProjectTool {
    async fn execute_github(&self, action: &str, args: &Value, limit: u64) -> Result<String> {
        match action {
            "list_issues" => {
                let repo = self.resolve_project(args)?;
                self.github_list_issues(repo, limit).await
            }
            "get_issue" => {
                let repo = self.resolve_project(args)?;
                let issue_id = args
                    .get("issue_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'issue_id' is required for get_issue".to_string())
                    })?;
                self.github_get_issue(repo, issue_id).await
            }
            "create_issue" => {
                let repo = self.resolve_project(args)?;
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'title' is required for create_issue".to_string())
                    })?;
                let description = args.get("description").and_then(Value::as_str);
                let labels = args.get("labels").and_then(Value::as_str);
                let assignee = args.get("assignee").and_then(Value::as_str);
                self.github_create_issue(repo, title, description, labels, assignee)
                    .await
            }
            "update_issue" => {
                let repo = self.resolve_project(args)?;
                let issue_id = args
                    .get("issue_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'issue_id' is required for update_issue".to_string())
                    })?;
                let title = args.get("title").and_then(Value::as_str);
                let description = args.get("description").and_then(Value::as_str);
                let status = args.get("status").and_then(Value::as_str);
                let labels = args.get("labels").and_then(Value::as_str);
                let assignee = args.get("assignee").and_then(Value::as_str);
                self.github_update_issue(repo, issue_id, title, description, status, labels, assignee)
                    .await
            }
            "search" => {
                let repo = self.resolve_project(args)?;
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'query' is required for search".to_string())
                    })?;
                self.github_search(repo, query, limit).await
            }
            other => Err(ZeptoError::Tool(format!(
                "Unknown action '{}'. Supported: list_issues, get_issue, create_issue, update_issue, search",
                other
            ))),
        }
    }

    async fn execute_jira(&self, action: &str, args: &Value, limit: u64) -> Result<String> {
        match action {
            "list_issues" => {
                let project = self.resolve_project(args)?;
                self.jira_list_issues(project, limit).await
            }
            "get_issue" => {
                let issue_id = args
                    .get("issue_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'issue_id' is required for get_issue".to_string())
                    })?;
                self.jira_get_issue(issue_id).await
            }
            "create_issue" => {
                let project = self.resolve_project(args)?;
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'title' is required for create_issue".to_string())
                    })?;
                let description = args.get("description").and_then(Value::as_str);
                let labels = args.get("labels").and_then(Value::as_str);
                let assignee = args.get("assignee").and_then(Value::as_str);
                self.jira_create_issue(project, title, description, labels, assignee)
                    .await
            }
            "update_issue" => {
                let issue_id = args
                    .get("issue_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'issue_id' is required for update_issue".to_string())
                    })?;
                let title = args.get("title").and_then(Value::as_str);
                let description = args.get("description").and_then(Value::as_str);
                let status = args.get("status").and_then(Value::as_str);
                let labels = args.get("labels").and_then(Value::as_str);
                let assignee = args.get("assignee").and_then(Value::as_str);
                self.jira_update_issue(issue_id, title, description, status, labels, assignee)
                    .await
            }
            "search" => {
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        ZeptoError::Tool("'query' is required for search".to_string())
                    })?;
                self.jira_search(query, limit).await
            }
            other => Err(ZeptoError::Tool(format!(
                "Unknown action '{}'. Supported: list_issues, get_issue, create_issue, update_issue, search, transitions",
                other
            ))),
        }
    }

    async fn execute_linear(&self, action: &str, args: &Value, limit: u64) -> Result<String> {
        match action {
            "list_issues" => self.linear_list_issues(limit).await,
            "get_issue" | "create_issue" | "update_issue" | "search" => {
                let _ = (args, limit); // suppress unused warnings
                Err(ZeptoError::Tool(format!(
                    "Action '{}' is not yet supported for the Linear backend. Only 'list_issues' is currently available.",
                    action
                )))
            }
            other => Err(ZeptoError::Tool(format!(
                "Unknown action '{}'. Supported for Linear: list_issues",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectBackend, ProjectConfig};
    use serde_json::json;

    fn github_config(token: &str, default_project: &str) -> ProjectConfig {
        ProjectConfig {
            backend: ProjectBackend::Github,
            default_project: default_project.to_string(),
            jira_url: String::new(),
            jira_token: None,
            github_token: if token.is_empty() {
                None
            } else {
                Some(token.to_string())
            },
            linear_api_key: None,
        }
    }

    fn jira_config(token: &str, jira_url: &str, default_project: &str) -> ProjectConfig {
        ProjectConfig {
            backend: ProjectBackend::Jira,
            default_project: default_project.to_string(),
            jira_url: jira_url.to_string(),
            jira_token: if token.is_empty() {
                None
            } else {
                Some(token.to_string())
            },
            github_token: None,
            linear_api_key: None,
        }
    }

    fn linear_config(key: &str) -> ProjectConfig {
        ProjectConfig {
            backend: ProjectBackend::Linear,
            default_project: String::new(),
            jira_url: String::new(),
            jira_token: None,
            github_token: None,
            linear_api_key: if key.is_empty() {
                None
            } else {
                Some(key.to_string())
            },
        }
    }

    // ---- Config tests --------------------------------------------------------

    #[test]
    fn test_project_config_defaults() {
        let config = ProjectConfig::default();
        assert_eq!(config.backend, ProjectBackend::Github);
        assert!(config.default_project.is_empty());
        assert!(config.jira_url.is_empty());
        assert!(config.github_token.is_none());
        assert!(config.jira_token.is_none());
        assert!(config.linear_api_key.is_none());
    }

    #[test]
    fn test_project_config_serde_roundtrip() {
        let config = ProjectConfig {
            backend: ProjectBackend::Jira,
            default_project: "MY-PROJ".to_string(),
            jira_url: "https://example.atlassian.net".to_string(),
            jira_token: Some("dXNlcjp0b2tlbg==".to_string()),
            github_token: None,
            linear_api_key: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: ProjectConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.backend, ProjectBackend::Jira);
        assert_eq!(restored.default_project, "MY-PROJ");
        assert_eq!(restored.jira_token.as_deref(), Some("dXNlcjp0b2tlbg=="));
    }

    #[test]
    fn test_project_backend_serde_github() {
        let json = r#""github""#;
        let backend: ProjectBackend = serde_json::from_str(json).unwrap();
        assert_eq!(backend, ProjectBackend::Github);
    }

    #[test]
    fn test_project_backend_serde_jira() {
        let json = r#""jira""#;
        let backend: ProjectBackend = serde_json::from_str(json).unwrap();
        assert_eq!(backend, ProjectBackend::Jira);
    }

    #[test]
    fn test_project_backend_serde_linear() {
        let json = r#""linear""#;
        let backend: ProjectBackend = serde_json::from_str(json).unwrap();
        assert_eq!(backend, ProjectBackend::Linear);
    }

    // ---- Auth header tests ---------------------------------------------------

    #[test]
    fn test_auth_header_github() {
        let config = github_config("ghp_mytoken", "owner/repo");
        let tool = ProjectTool::new(config);
        let header = tool.auth_header().unwrap();
        assert_eq!(header, "Bearer ghp_mytoken");
    }

    #[test]
    fn test_auth_header_jira() {
        let config = jira_config("base64token", "https://example.atlassian.net", "PROJ");
        let tool = ProjectTool::new(config);
        let header = tool.auth_header().unwrap();
        assert_eq!(header, "Basic base64token");
    }

    #[test]
    fn test_auth_header_linear() {
        let config = linear_config("lin_api_key123");
        let tool = ProjectTool::new(config);
        let header = tool.auth_header().unwrap();
        assert_eq!(header, "lin_api_key123");
    }

    #[test]
    fn test_auth_header_missing_github_token() {
        let config = github_config("", "owner/repo");
        let tool = ProjectTool::new(config);
        let result = tool.auth_header();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("GitHub token"));
    }

    #[test]
    fn test_auth_header_missing_jira_token() {
        let config = jira_config("", "https://example.atlassian.net", "PROJ");
        let tool = ProjectTool::new(config);
        let result = tool.auth_header();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Jira token"));
    }

    #[test]
    fn test_auth_header_missing_linear_key() {
        let config = linear_config("");
        let tool = ProjectTool::new(config);
        let result = tool.auth_header();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Linear API key"));
    }

    // ---- resolve_project tests -----------------------------------------------

    #[test]
    fn test_project_or_default() {
        let config = github_config("tok", "owner/default-repo");
        let tool = ProjectTool::new(config);
        // No explicit project arg — uses default
        let args = json!({});
        let result = tool.resolve_project(&args).unwrap();
        assert_eq!(result, "owner/default-repo");
    }

    #[test]
    fn test_project_or_default_override() {
        let config = github_config("tok", "owner/default-repo");
        let tool = ProjectTool::new(config);
        // Explicit arg overrides default
        let args = json!({"project": "other/repo"});
        let result = tool.resolve_project(&args).unwrap();
        assert_eq!(result, "other/repo");
    }

    #[test]
    fn test_project_or_default_no_default() {
        let config = github_config("tok", "");
        let tool = ProjectTool::new(config);
        let args = json!({});
        let result = tool.resolve_project(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("default_project"), "got: {}", err);
    }

    // ---- Action tests --------------------------------------------------------

    #[tokio::test]
    async fn test_missing_action() {
        let config = github_config("tok", "owner/repo");
        let tool = ProjectTool::new(config);
        let result = tool.execute(json!({}), &ToolContext::new()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing 'action'"), "got: {}", err);
    }

    #[tokio::test]
    async fn test_transitions_github_unsupported() {
        let config = github_config("tok", "owner/repo");
        let tool = ProjectTool::new(config);
        let result = tool
            .execute(
                json!({"action": "transitions", "issue_id": "1"}),
                &ToolContext::new(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Jira backend"),
            "expected Jira backend message, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_transitions_linear_unsupported() {
        let config = linear_config("lin_key");
        let tool = ProjectTool::new(config);
        let result = tool
            .execute(
                json!({"action": "transitions", "issue_id": "LIN-1"}),
                &ToolContext::new(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Jira backend"),
            "expected Jira backend message, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_jira_base_url_missing() {
        let config = jira_config("tok", "", "PROJ");
        let tool = ProjectTool::new(config);
        let result = tool
            .execute(
                json!({"action": "list_issues", "project": "PROJ"}),
                &ToolContext::new(),
            )
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Jira URL"),
            "expected Jira URL error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_linear_unsupported_actions() {
        let config = linear_config("lin_key");
        let tool = ProjectTool::new(config);
        for action in &["create_issue", "get_issue", "update_issue", "search"] {
            let result = tool
                .execute(
                    json!({"action": action, "title": "Test"}),
                    &ToolContext::new(),
                )
                .await;
            assert!(
                result.is_err(),
                "Expected error for Linear action '{}', got Ok",
                action
            );
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("not yet supported"),
                "Expected 'not yet supported' for action '{}', got: {}",
                action,
                err
            );
        }
    }

    // ---- Tool metadata -------------------------------------------------------

    #[test]
    fn test_tool_name() {
        let config = github_config("tok", "owner/repo");
        let tool = ProjectTool::new(config);
        assert_eq!(tool.name(), "project");
    }

    #[test]
    fn test_parameters_schema() {
        let config = github_config("tok", "owner/repo");
        let tool = ProjectTool::new(config);
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert!(params["properties"]["project"].is_object());
        assert!(params["properties"]["issue_id"].is_object());
        assert!(params["properties"]["title"].is_object());
        assert!(params["properties"]["description"].is_object());
        assert!(params["properties"]["status"].is_object());
        assert!(params["properties"]["query"].is_object());
        assert!(params["properties"]["labels"].is_object());
        assert!(params["properties"]["assignee"].is_object());
        assert!(params["properties"]["limit"].is_object());
        let required = params["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "action");
    }
}
