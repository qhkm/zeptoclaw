//! Web screenshot tool (feature-gated behind `screenshot`).
//!
//! Captures screenshots of web pages using a headless Chromium browser
//! via the Chrome DevTools Protocol. Includes full SSRF protection by
//! reusing the validation from [`super::web`].

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use reqwest::Url;
use serde_json::{json, Value};
use tokio::time::timeout;

use crate::error::{Result, ZeptoError};

use super::web::{is_blocked_host, resolve_and_check_host};
use super::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Default page-load timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum allowed timeout to prevent unbounded waits.
const MAX_TIMEOUT_SECS: u64 = 120;

/// Default viewport width in pixels.
const DEFAULT_WIDTH: u32 = 1280;

/// Default viewport height in pixels.
const DEFAULT_HEIGHT: u32 = 720;

/// Minimum viewport dimension.
const MIN_DIMENSION: u32 = 100;

/// Maximum viewport dimension.
const MAX_DIMENSION: u32 = 3840;

/// Web screenshot tool that captures full-page screenshots of URLs.
///
/// Uses a headless Chromium browser via the Chrome DevTools Protocol.
/// Applies the same SSRF protections as the web fetch tool to prevent
/// screenshots of internal/private network resources.
pub struct WebScreenshotTool;

impl WebScreenshotTool {
    /// Create a new web screenshot tool.
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebScreenshotTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebScreenshotTool {
    fn name(&self) -> &str {
        "web_screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot of a web page. Returns base64-encoded PNG or saves to a file path."
    }

    fn compact_description(&self) -> &str {
        "Screenshot URL"
    }

    fn category(&self) -> ToolCategory {
        // Fetches URL (NetworkRead) AND writes file to disk — use more restrictive category.
        ToolCategory::FilesystemWrite
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to capture a screenshot of (http/https only)"
                },
                "output_path": {
                    "type": "string",
                    "description": "File path to save the screenshot PNG. If omitted, returns base64-encoded data."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Page load timeout in seconds (default: 30, max: 120)",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_SECS
                },
                "width": {
                    "type": "integer",
                    "description": "Viewport width in pixels (default: 1280)",
                    "minimum": MIN_DIMENSION,
                    "maximum": MAX_DIMENSION
                },
                "height": {
                    "type": "integer",
                    "description": "Viewport height in pixels (default: 720)",
                    "minimum": MIN_DIMENSION,
                    "maximum": MAX_DIMENSION
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        // ---- Parse and validate URL ----
        let url_str = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing or empty 'url' parameter".to_string()))?;

        let parsed = Url::parse(url_str)
            .map_err(|e| ZeptoError::Tool(format!("Invalid URL '{}': {}", url_str, e)))?;

        match parsed.scheme() {
            "http" | "https" => {}
            other => {
                return Err(ZeptoError::Tool(format!(
                    "Only http/https URLs are allowed, got '{}'",
                    other
                )));
            }
        }

        // ---- SSRF protection ----
        if is_blocked_host(&parsed) {
            return Err(ZeptoError::SecurityViolation(
                "Blocked URL host (local or private network)".to_string(),
            ));
        }
        resolve_and_check_host(&parsed).await?;

        // ---- Parse optional parameters ----
        let output_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        let width = args
            .get("width")
            .and_then(|v| v.as_u64())
            .map(|v| (v as u32).clamp(MIN_DIMENSION, MAX_DIMENSION))
            .unwrap_or(DEFAULT_WIDTH);

        let height = args
            .get("height")
            .and_then(|v| v.as_u64())
            .map(|v| (v as u32).clamp(MIN_DIMENSION, MAX_DIMENSION))
            .unwrap_or(DEFAULT_HEIGHT);

        // ---- Launch headless browser ----
        let browser_config = BrowserConfig::builder()
            .no_sandbox()
            .viewport(Some(Viewport {
                width,
                height,
                device_scale_factor: None,
                emulating_mobile: false,
                is_landscape: false,
                has_touch: false,
            }))
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .build()
            .map_err(|e| ZeptoError::Tool(format!("Failed to configure browser: {}", e)))?;

        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to launch browser: {}", e)))?;

        // Spawn the CDP handler loop so the browser stays alive.
        let handler_handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                let _ = event;
            }
        });

        // ---- Navigate and screenshot (with timeout) ----
        let screenshot_result = timeout(Duration::from_secs(timeout_secs), async {
            let page = browser
                .new_page(url_str)
                .await
                .map_err(|e| ZeptoError::Tool(format!("Failed to open page: {}", e)))?;

            let screenshot_bytes = page
                .screenshot(ScreenshotParams::builder().full_page(false).build())
                .await
                .map_err(|e| ZeptoError::Tool(format!("Failed to capture screenshot: {}", e)))?;

            Ok::<Vec<u8>, ZeptoError>(screenshot_bytes)
        })
        .await
        .map_err(|_| {
            ZeptoError::Tool(format!(
                "Screenshot timed out after {}s for '{}'",
                timeout_secs, url_str
            ))
        })??;

        // Clean up browser resources.
        drop(browser);
        handler_handle.abort();

        // ---- Output: save or encode ----
        let result = if let Some(path) = output_path {
            tokio::fs::write(&path, &screenshot_result)
                .await
                .map_err(|e| {
                    ZeptoError::Tool(format!("Failed to write screenshot to '{}': {}", path, e))
                })?;

            json!({
                "url": url_str,
                "output_path": path,
                "size_bytes": screenshot_result.len(),
                "width": width,
                "height": height,
            })
            .to_string()
        } else {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&screenshot_result);
            json!({
                "url": url_str,
                "format": "png",
                "encoding": "base64",
                "size_bytes": screenshot_result.len(),
                "width": width,
                "height": height,
                "data": encoded,
            })
            .to_string()
        };

        Ok(ToolOutput::llm_only(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Tool metadata tests ----

    #[test]
    fn test_tool_name() {
        let tool = WebScreenshotTool::new();
        assert_eq!(tool.name(), "web_screenshot");
    }

    #[test]
    fn test_tool_description() {
        let tool = WebScreenshotTool::new();
        assert!(tool.description().contains("screenshot"));
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_compact_description() {
        let tool = WebScreenshotTool::new();
        assert_eq!(tool.compact_description(), "Screenshot URL");
        assert!(tool.compact_description().len() < tool.description().len());
    }

    #[test]
    fn test_parameters_schema() {
        let tool = WebScreenshotTool::new();
        let params = tool.parameters();

        assert_eq!(params["type"], "object");
        assert!(params["properties"]["url"].is_object());
        assert!(params["properties"]["output_path"].is_object());
        assert!(params["properties"]["timeout_secs"].is_object());
        assert!(params["properties"]["width"].is_object());
        assert!(params["properties"]["height"].is_object());

        // "url" is required
        let required = params["required"]
            .as_array()
            .expect("required should be array");
        assert!(required.iter().any(|v| v.as_str() == Some("url")));
    }

    #[test]
    fn test_parameters_url_field_type() {
        let tool = WebScreenshotTool::new();
        let params = tool.parameters();
        assert_eq!(params["properties"]["url"]["type"], "string");
    }

    #[test]
    fn test_default_constructor() {
        let tool = WebScreenshotTool::default();
        assert_eq!(tool.name(), "web_screenshot");
    }

    // ---- URL validation tests ----

    #[tokio::test]
    async fn test_missing_url_parameter() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Missing") || err.contains("url"),
            "Expected missing URL error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_empty_url_parameter() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"url": ""}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Missing") || err.contains("empty"),
            "Expected empty URL error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_whitespace_only_url() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"url": "   "}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_url_format() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool.execute(json!({"url": "not-a-valid-url"}), &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid URL"),
            "Expected URL parse error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_non_http_scheme_rejected() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "ftp://example.com/file.txt"}), &ctx)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Only http/https"),
            "Expected scheme error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_file_scheme_rejected() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "file:///etc/passwd"}), &ctx)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Only http/https"),
            "Expected scheme error, got: {}",
            err
        );
    }

    // ---- SSRF protection tests ----

    #[tokio::test]
    async fn test_ssrf_localhost_blocked() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "http://localhost:8080/admin"}), &ctx)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Blocked") || err.contains("local") || err.contains("private"),
            "Expected SSRF block error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_ssrf_private_ip_blocked() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "http://192.168.1.1/router"}), &ctx)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Blocked") || err.contains("local") || err.contains("private"),
            "Expected SSRF block error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_ssrf_loopback_blocked() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "http://127.0.0.1:9090/"}), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssrf_metadata_endpoint_blocked() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(
                json!({"url": "http://169.254.169.254/latest/meta-data/"}),
                &ctx,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssrf_internal_ten_network_blocked() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "http://10.0.0.1/internal"}), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssrf_dot_local_blocked() {
        let tool = WebScreenshotTool::new();
        let ctx = ToolContext::new();

        let result = tool
            .execute(json!({"url": "http://internal.local/data"}), &ctx)
            .await;
        assert!(result.is_err());
    }

    // ---- Parameter parsing / defaults tests ----

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_TIMEOUT_SECS, 30);
        assert_eq!(MAX_TIMEOUT_SECS, 120);
        assert_eq!(DEFAULT_WIDTH, 1280);
        assert_eq!(DEFAULT_HEIGHT, 720);
        assert_eq!(MIN_DIMENSION, 100);
        assert_eq!(MAX_DIMENSION, 3840);
    }

    #[test]
    fn test_parameter_clamping_logic() {
        // Simulate the clamping logic used in execute()
        let clamp = |v: u64| -> u32 { (v as u32).clamp(MIN_DIMENSION, MAX_DIMENSION) };

        assert_eq!(clamp(50), MIN_DIMENSION);
        assert_eq!(clamp(5000), MAX_DIMENSION);
        assert_eq!(clamp(1920), 1920);
    }

    #[test]
    fn test_timeout_clamping_logic() {
        let clamp_timeout = |v: u64| -> u64 { v.clamp(1, MAX_TIMEOUT_SECS) };

        assert_eq!(clamp_timeout(0), 1);
        assert_eq!(clamp_timeout(200), MAX_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(60), 60);
    }

    // Note: We intentionally do NOT test actual browser launching here.
    // That requires Chrome/Chromium to be installed and is covered by
    // integration tests, not unit tests.
}
