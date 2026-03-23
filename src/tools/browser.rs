//! Browser tool — headless web browsing via agent-browser + Lightpanda.
//!
//! Wraps the `agent-browser` CLI to provide full browser automation:
//! navigation, content extraction, form filling, clicking, screenshots, etc.
//! Uses Lightpanda as the default engine (10x faster, 10x less memory than Chrome).

use async_trait::async_trait;
use reqwest::Url;
use serde_json::{json, Value};
use std::time::Duration;

use crate::config::BrowserConfig;
use crate::error::{Result, ZeptoError};

use super::web::{is_blocked_host, validate_redirect_target_basic};
use super::{Tool, ToolCategory, ToolContext, ToolOutput};

pub struct BrowserTool {
    engine: String,
    executable: String,
    timeout_secs: u64,
}

impl BrowserTool {
    pub fn new(config: &BrowserConfig) -> Self {
        Self {
            engine: config.engine.clone(),
            executable: config
                .executable_path
                .clone()
                .unwrap_or_else(|| "agent-browser".to_string()),
            timeout_secs: config.timeout_secs,
        }
    }

    /// Run an agent-browser command and return its stdout.
    async fn run_command(&self, command: &str, args: &[&str]) -> Result<String> {
        let mut cmd = tokio::process::Command::new(&self.executable);
        cmd.arg(command);
        cmd.args(args);
        cmd.env("AGENT_BROWSER_ENGINE", &self.engine);
        cmd.env("LIGHTPANDA_DISABLE_TELEMETRY", "true");

        let timeout = Duration::from_secs(self.timeout_secs);
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                ZeptoError::Tool(format!(
                    "Browser command '{}' timed out after {}s",
                    command, self.timeout_secs
                ))
            })?
            .map_err(|e| ZeptoError::Tool(format!("Failed to run agent-browser: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = if stderr.is_empty() {
                String::from_utf8_lossy(&output.stdout)
            } else {
                stderr
            };
            return Err(ZeptoError::Tool(format!(
                "agent-browser {} failed: {}",
                command,
                msg.trim()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Validate a URL against SSRF blocklist (scheme + host check).
    fn check_url(url_str: &str) -> Result<()> {
        let parsed = Url::parse(url_str)
            .map_err(|e| ZeptoError::Tool(format!("Invalid URL '{}': {}", url_str, e)))?;
        validate_redirect_target_basic(&parsed)
    }

    /// Post-navigation check: verify the final URL isn't a private/local address
    /// (catches redirect-based SSRF). Fails closed on unparseable URLs.
    async fn check_final_url(&self) -> Result<()> {
        let final_url = self.run_command("get", &["url"]).await?;
        let final_url = final_url.trim();

        if final_url.is_empty() {
            return Ok(());
        }

        let parsed = match Url::parse(final_url) {
            Ok(u) => u,
            Err(_) => {
                // Fail closed: unparseable final URL could be an exotic redirect
                if let Err(e) = self.run_command("close", &[]).await {
                    tracing::warn!("Failed to close browser after SSRF check: {}", e);
                }
                return Err(ZeptoError::SecurityViolation(format!(
                    "Navigation resulted in unparseable URL: {}",
                    final_url
                )));
            }
        };

        if is_blocked_host(&parsed) {
            if let Err(e) = self.run_command("close", &[]).await {
                tracing::warn!("Failed to close browser after SSRF block: {}", e);
            }
            return Err(ZeptoError::SecurityViolation(format!(
                "Navigation redirected to blocked host: {}",
                final_url
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Browse the web: fetch page content, read articles, interact with websites. \
         Use this tool whenever you need to visit a URL or retrieve web content. \
         Typical flow: open <url>, then snapshot to read the page. \
         Commands: open <url> (navigate to page), snapshot (read page content with element refs), \
         click <ref> (click element), fill <ref> <text> (type into input), \
         find role|text|label <query> (find elements), get text|html|url|title, \
         scroll up|down, back, forward, screenshot [path], wait <selector|ms>. \
         Element refs like @e1 are assigned by snapshot and reused for interaction."
    }

    fn compact_description(&self) -> &str {
        "Browse web"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The agent-browser command to run (e.g. open, snapshot, click, fill, find, get, scroll, back, screenshot, wait, close)"
                },
                "args": {
                    "type": "string",
                    "description": "Arguments for the command (e.g. a URL for open, a ref like @e1 for click, 'text hello' for find)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'command' argument".into()))?;

        let args_str = args.get("args").and_then(|v| v.as_str()).unwrap_or("");

        let is_navigation = command == "open";

        // Pre-navigation SSRF check
        if is_navigation {
            let url = args_str.split_whitespace().next().unwrap_or(args_str);
            if url.is_empty() {
                return Err(ZeptoError::Tool(format!(
                    "'{}' command requires a URL argument",
                    command
                )));
            }
            Self::check_url(url)?;
        }

        // Split args on whitespace (agent-browser args are simple tokens)
        let cmd_args: Vec<&str> = if args_str.is_empty() {
            vec![]
        } else {
            args_str.split_whitespace().collect()
        };

        let output = self.run_command(command, &cmd_args).await?;

        // Post-navigation SSRF check (catches redirects)
        if is_navigation {
            self.check_final_url().await?;
        }

        Ok(ToolOutput::user_visible(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_url_blocks_localhost() {
        assert!(BrowserTool::check_url("http://localhost").is_err());
        assert!(BrowserTool::check_url("http://localhost:8080").is_err());
        assert!(BrowserTool::check_url("http://127.0.0.1").is_err());
    }

    #[test]
    fn test_check_url_blocks_private_networks() {
        assert!(BrowserTool::check_url("http://192.168.1.1").is_err());
        assert!(BrowserTool::check_url("http://10.0.0.1").is_err());
        assert!(BrowserTool::check_url("http://172.16.0.1").is_err());
    }

    #[test]
    fn test_check_url_allows_public() {
        assert!(BrowserTool::check_url("https://example.com").is_ok());
        assert!(BrowserTool::check_url("https://google.com").is_ok());
    }

    #[test]
    fn test_check_url_rejects_non_http() {
        assert!(BrowserTool::check_url("ftp://example.com").is_err());
        assert!(BrowserTool::check_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_check_url_rejects_invalid() {
        assert!(BrowserTool::check_url("not a url").is_err());
    }
}
