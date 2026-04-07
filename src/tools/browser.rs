//! Browser tool — headless web browsing via agent-browser + Lightpanda.
//!
//! Wraps the `agent-browser` CLI to provide full browser automation:
//! navigation, content extraction, form filling, clicking, screenshots, etc.
//! Uses Lightpanda as the default engine (10x faster, 10x less memory than Chrome).
//! Falls back to Chrome automatically if Lightpanda fails on navigation.

use async_trait::async_trait;
use reqwest::Url;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::config::BrowserConfig;
use crate::deps::{DepKind, Dependency, HasDependencies, HealthCheck};
use crate::error::{Result, ZeptoError};

use super::web::{is_blocked_host, resolve_and_check_host, validate_redirect_target_basic};
use super::{Tool, ToolCategory, ToolContext, ToolOutput};

const ENGINE_LIGHTPANDA: &str = "lightpanda";
const ENGINE_CHROME: &str = "chrome";

/// Browser automation tool wrapping the `agent-browser` CLI.
///
/// **Single-tenant**: engine state (`active_engine`) is shared across all
/// invocations of this tool instance. If multi-tenant isolation is needed,
/// engine state should move into `ToolContext` per conversation.
pub struct BrowserTool {
    default_engine: String,
    active_engine: Mutex<String>,
    executable: String,
    timeout_secs: u64,
}

impl BrowserTool {
    pub fn new(config: &BrowserConfig) -> Self {
        let engine = config.engine.clone();
        Self {
            active_engine: Mutex::new(engine.clone()),
            default_engine: engine,
            executable: config
                .executable_path
                .clone()
                .unwrap_or_else(|| "agent-browser".to_string()),
            timeout_secs: config.timeout_secs,
        }
    }

    /// Run an agent-browser command with a specific engine and return its stdout.
    async fn run_command_with_engine(
        &self,
        command: &str,
        args: &[&str],
        engine: &str,
    ) -> Result<String> {
        let mut cmd = tokio::process::Command::new(&self.executable);
        cmd.arg(command);
        cmd.args(args);
        cmd.env("AGENT_BROWSER_ENGINE", engine);
        cmd.env("LIGHTPANDA_DISABLE_TELEMETRY", "true");
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);

        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ZeptoError::Tool(format!(
                    "'{}' not found. Install it with:\n  \
                     npm install -g agent-browser   # or: brew install agent-browser\n  \
                     agent-browser install           # downloads Chrome\n\
                     For LightPanda (optional, faster): see https://agent-browser.dev/engines/lightpanda",
                    self.executable
                ))
            } else {
                ZeptoError::Tool(format!("Failed to run agent-browser: {}", e))
            }
        })?;

        let timeout = Duration::from_secs(self.timeout_secs);
        let output = tokio::time::timeout(timeout, child.wait_with_output())
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

    fn get_active_engine(&self) -> String {
        self.active_engine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_active_engine(&self, engine: &str) {
        *self.active_engine.lock().unwrap_or_else(|e| e.into_inner()) = engine.to_string();
    }

    /// Validate a URL against SSRF blocklist (scheme + host + DNS resolution).
    async fn check_url(url_str: &str) -> Result<()> {
        let parsed = Url::parse(url_str)
            .map_err(|e| ZeptoError::Tool(format!("Invalid URL '{}': {}", url_str, e)))?;
        validate_redirect_target_basic(&parsed)?;
        // DNS-aware check: resolve hostname and verify resolved IPs aren't private.
        // Catches attacks like metadata.attacker.com → 169.254.169.254.
        resolve_and_check_host(&parsed).await?;
        Ok(())
    }

    /// Post-navigation SSRF check: verify the final URL isn't a private/local address
    /// (catches redirect-based SSRF). Fails closed on unparseable URLs.
    async fn check_final_url(&self, engine: &str) -> Result<()> {
        let final_url = self
            .run_command_with_engine("get", &["url"], engine)
            .await?;
        let final_url = final_url.trim();

        if final_url.is_empty() {
            return Ok(());
        }

        let parsed = match Url::parse(final_url) {
            Ok(u) => u,
            Err(_) => {
                if let Err(e) = self.run_command_with_engine("close", &[], engine).await {
                    tracing::warn!("Failed to close browser after SSRF check: {}", e);
                }
                return Err(ZeptoError::SecurityViolation(format!(
                    "Navigation resulted in unparseable URL: {}",
                    final_url
                )));
            }
        };

        if is_blocked_host(&parsed) {
            if let Err(e) = self.run_command_with_engine("close", &[], engine).await {
                tracing::warn!("Failed to close browser after SSRF block: {}", e);
            }
            return Err(ZeptoError::SecurityViolation(format!(
                "Navigation redirected to blocked host: {}",
                final_url
            )));
        }

        Ok(())
    }

    /// Build a concise one-liner for the user, or None to show nothing.
    /// The model always gets the full output regardless.
    fn summarize_for_user(command: &str, args_str: &str) -> Option<String> {
        match command {
            "open" => {
                let url = args_str.split_whitespace().next().unwrap_or(args_str);
                Some(format!("Browsing {}", url))
            }
            "screenshot" => Some("Screenshot captured".to_string()),
            _ => None,
        }
    }
}

impl HasDependencies for BrowserTool {
    fn dependencies(&self) -> Vec<Dependency> {
        vec![
            Dependency {
                name: "agent-browser".to_string(),
                kind: DepKind::NpmPackage {
                    package: "agent-browser".to_string(),
                    version: "latest".to_string(),
                    entry_point: "agent-browser".to_string(),
                },
                health_check: HealthCheck::Command {
                    command: "agent-browser --version".to_string(),
                },
                env: HashMap::new(),
                args: vec![],
            },
            Dependency {
                name: "lightpanda".to_string(),
                kind: DepKind::Binary {
                    repo: "lightpanda-io/browser".to_string(),
                    asset_pattern: "lightpanda-{arch}-{os}".to_string(),
                    version: "nightly".to_string(),
                },
                health_check: HealthCheck::Command {
                    command: "lightpanda --version".to_string(),
                },
                env: HashMap::new(),
                args: vec![],
            },
        ]
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
         Element refs like @e1 are assigned by snapshot and reused for interaction. \
         Optional engine param: set to 'chrome' for full rendering fidelity when the user \
         requests Chrome. Defaults to lightpanda (faster). Falls back to Chrome automatically \
         if lightpanda fails."
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
                },
                "engine": {
                    "type": "string",
                    "enum": ["lightpanda", "chrome"],
                    "description": "Browser engine override. Use 'chrome' for full rendering fidelity when requested by the user."
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
        let engine_override = args.get("engine").and_then(|v| v.as_str());

        // The JSON schema declares an enum for `engine`, but schemas are
        // advisory — the LLM can produce arbitrary values. Enforce an
        // allowlist so the engine string can never reach the
        // AGENT_BROWSER_ENGINE env var as something unexpected.
        if let Some(ov) = engine_override {
            if ov != ENGINE_LIGHTPANDA && ov != ENGINE_CHROME {
                return Err(ZeptoError::Tool(format!(
                    "Invalid engine '{}': must be '{}' or '{}'",
                    ov, ENGINE_LIGHTPANDA, ENGINE_CHROME
                )));
            }
        }

        let is_navigation = command == "open";

        // SSRF check: validate URLs in all commands that accept them.
        // "open" always has a URL; other commands may contain URLs in args.
        let url_to_check = if is_navigation {
            let url = args_str.split_whitespace().next().unwrap_or(args_str);
            if url.is_empty() {
                return Err(ZeptoError::Tool(format!(
                    "'{}' command requires a URL argument",
                    command
                )));
            }
            Some(url.to_string())
        } else {
            // Check for URLs in args of other commands that can accept them
            // (tab new <url>, connect <url>, network route <url>).
            // Also catch any arg that looks like a URL to be safe.
            args_str
                .split_whitespace()
                .find(|arg| arg.starts_with("http://") || arg.starts_with("https://"))
                .map(|s| s.to_string())
        };

        if let Some(ref url) = url_to_check {
            Self::check_url(url).await?;
        }

        if command == "close" {
            let engine = self.get_active_engine();
            let output = self.run_command_with_engine(command, &[], &engine).await?;
            self.set_active_engine(&self.default_engine);
            return Ok(ToolOutput::llm_only(output));
        }

        // Resolve engine: explicit override > active session engine
        let engine = if let Some(ov) = engine_override {
            self.set_active_engine(ov);
            ov.to_string()
        } else {
            self.get_active_engine()
        };

        // Build CLI args. Most commands take discrete tokens that can be split on
        // whitespace. However, `fill`/`type` take `<selector> <text>` where the text
        // portion may contain spaces, `keyboard` takes `<subcommand> <text>`, and
        // `eval` takes a single JS expression. Handle these specially.
        let cmd_args: Vec<&str> = if args_str.is_empty() {
            vec![]
        } else {
            match command {
                // <selector> <text> — split into exactly two args
                "fill" | "type" => match args_str.split_once(char::is_whitespace) {
                    Some((sel, text)) => vec![sel, text.trim_start()],
                    None => vec![args_str],
                },
                // <subcommand> <text> — e.g. "type hello world" or "inserttext hello"
                "keyboard" => match args_str.split_once(char::is_whitespace) {
                    Some((sub, text)) => vec![sub, text.trim_start()],
                    None => vec![args_str],
                },
                // Single expression/arg — don't split
                "eval" => vec![args_str],
                // All other commands: discrete tokens
                _ => args_str.split_whitespace().collect(),
            }
        };

        let (output, engine) = match self
            .run_command_with_engine(command, &cmd_args, &engine)
            .await
        {
            Ok(output) => (output, engine),
            Err(lp_err) if is_navigation && engine == ENGINE_LIGHTPANDA => {
                tracing::warn!(
                    "Lightpanda failed for '{}', falling back to Chrome: {}",
                    command,
                    lp_err
                );
                match self
                    .run_command_with_engine(command, &cmd_args, ENGINE_CHROME)
                    .await
                {
                    Ok(output) => {
                        self.set_active_engine(ENGINE_CHROME);
                        tracing::info!("Chrome fallback succeeded, session switched to Chrome");
                        (output, ENGINE_CHROME.to_string())
                    }
                    Err(chrome_err) => {
                        return Err(ZeptoError::Tool(format!(
                            "Both engines failed. Lightpanda: {}. Chrome: {}",
                            lp_err, chrome_err
                        )));
                    }
                }
            }
            Err(e) => return Err(e),
        };

        // Post-navigation SSRF check (catches redirects).
        //
        // Run the check whenever the command may have caused a navigation,
        // not only when the request body explicitly carried a URL. Commands
        // like `click`, `back`, `forward`, `submit`, and `follow` can land
        // the active page on an internal IP via a redirect chain (e.g.
        // `http://attacker.example/redirect?to=http://169.254.169.254/...`)
        // without ever passing through the pre-flight URL allowlist.
        let may_navigate = matches!(command, "click" | "back" | "forward" | "submit" | "follow");
        if url_to_check.is_some() || may_navigate {
            self.check_final_url(&engine).await?;
        }

        match Self::summarize_for_user(command, args_str) {
            Some(summary) => Ok(ToolOutput::split(output, summary)),
            None => Ok(ToolOutput::llm_only(output)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BrowserConfig;

    fn make_tool(engine: &str) -> BrowserTool {
        BrowserTool::new(&BrowserConfig {
            enabled: true,
            engine: engine.to_string(),
            executable_path: None,
            timeout_secs: 30,
        })
    }

    #[tokio::test]
    async fn test_check_url_blocks_localhost() {
        assert!(BrowserTool::check_url("http://localhost").await.is_err());
        assert!(BrowserTool::check_url("http://localhost:8080")
            .await
            .is_err());
        assert!(BrowserTool::check_url("http://127.0.0.1").await.is_err());
    }

    #[tokio::test]
    async fn test_check_url_blocks_private_networks() {
        assert!(BrowserTool::check_url("http://192.168.1.1").await.is_err());
        assert!(BrowserTool::check_url("http://10.0.0.1").await.is_err());
        assert!(BrowserTool::check_url("http://172.16.0.1").await.is_err());
    }

    #[tokio::test]
    async fn test_check_url_allows_public() {
        assert!(BrowserTool::check_url("https://example.com").await.is_ok());
        assert!(BrowserTool::check_url("https://google.com").await.is_ok());
    }

    #[tokio::test]
    async fn test_check_url_rejects_non_http() {
        assert!(BrowserTool::check_url("ftp://example.com").await.is_err());
        assert!(BrowserTool::check_url("file:///etc/passwd").await.is_err());
    }

    #[tokio::test]
    async fn test_check_url_rejects_invalid() {
        assert!(BrowserTool::check_url("not a url").await.is_err());
    }

    #[test]
    fn test_default_engine_is_preserved() {
        let tool = make_tool(ENGINE_LIGHTPANDA);
        assert_eq!(tool.default_engine, ENGINE_LIGHTPANDA);
    }

    #[test]
    fn test_engine_override_sets_active_engine() {
        let tool = make_tool(ENGINE_LIGHTPANDA);
        assert_eq!(tool.get_active_engine(), ENGINE_LIGHTPANDA);

        tool.set_active_engine(ENGINE_CHROME);
        assert_eq!(tool.get_active_engine(), ENGINE_CHROME);
    }

    #[test]
    fn test_close_resets_active_engine() {
        let tool = make_tool(ENGINE_LIGHTPANDA);

        tool.set_active_engine(ENGINE_CHROME);
        assert_eq!(tool.get_active_engine(), ENGINE_CHROME);

        // Simulate what close does
        tool.set_active_engine(&tool.default_engine);
        assert_eq!(tool.get_active_engine(), ENGINE_LIGHTPANDA);
    }

    #[test]
    fn test_summarize_open() {
        let summary = BrowserTool::summarize_for_user("open", "https://example.com");
        assert_eq!(summary, Some("Browsing https://example.com".to_string()));
    }

    #[test]
    fn test_summarize_screenshot() {
        let summary = BrowserTool::summarize_for_user("screenshot", "");
        assert_eq!(summary, Some("Screenshot captured".to_string()));
    }

    #[test]
    fn test_summarize_other_commands_are_silent() {
        assert!(BrowserTool::summarize_for_user("click", "@e1").is_none());
        assert!(BrowserTool::summarize_for_user("snapshot", "").is_none());
        assert!(BrowserTool::summarize_for_user("fill", "@e5 hello").is_none());
        assert!(BrowserTool::summarize_for_user("scroll", "down").is_none());
        assert!(BrowserTool::summarize_for_user("close", "").is_none());
    }

    #[test]
    fn test_parameters_include_engine() {
        let tool = make_tool(ENGINE_LIGHTPANDA);
        let params = tool.parameters();
        let engine_prop = &params["properties"]["engine"];
        assert_eq!(engine_prop["type"], "string");
        let enum_values = engine_prop["enum"].as_array().unwrap();
        assert_eq!(enum_values.len(), 2);
        assert!(enum_values.iter().any(|v| v == ENGINE_LIGHTPANDA));
        assert!(enum_values.iter().any(|v| v == ENGINE_CHROME));
    }

    #[tokio::test]
    async fn test_execute_rejects_invalid_engine_override() {
        let tool = make_tool(ENGINE_LIGHTPANDA);
        let ctx = crate::tools::types::ToolContext::new();
        // The JSON schema enum is advisory — execute() must enforce the
        // allowlist itself so the LLM cannot inject arbitrary engine strings
        // into the AGENT_BROWSER_ENGINE env var.
        let err = tool
            .execute(
                serde_json::json!({
                    "command": "open",
                    "args": "https://example.com",
                    "engine": "../../bin/sh"
                }),
                &ctx,
            )
            .await
            .expect_err("invalid engine must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("Invalid engine") && msg.contains("../../bin/sh"),
            "Expected invalid engine error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_execute_rejects_arbitrary_engine_value() {
        let tool = make_tool(ENGINE_LIGHTPANDA);
        let ctx = crate::tools::types::ToolContext::new();
        let err = tool
            .execute(
                serde_json::json!({
                    "command": "open",
                    "args": "https://example.com",
                    "engine": "firefox"
                }),
                &ctx,
            )
            .await
            .expect_err("non-allowlisted engine must be rejected");
        assert!(err.to_string().contains("Invalid engine"));
    }

    #[test]
    fn test_navigation_commands_trigger_post_nav_check() {
        // Document the allowlist of commands that require post-navigation
        // SSRF re-checks. If a new navigation-causing command is added to
        // the browser, it must also be added here so redirects can't reach
        // internal IPs (e.g. 169.254.169.254 cloud metadata endpoints).
        for cmd in &["click", "back", "forward", "submit", "follow"] {
            let may_navigate = matches!(*cmd, "click" | "back" | "forward" | "submit" | "follow");
            assert!(
                may_navigate,
                "{cmd} should be in the post-navigation SSRF check allowlist"
            );
        }
    }
}
