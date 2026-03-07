//! Web screenshot tool (feature-gated behind `screenshot`).
//!
//! Captures screenshots of web pages using a headless Chromium browser
//! via the Chrome DevTools Protocol. Includes full SSRF protection by
//! reusing the validation from [`super::web`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::{ErrorReason, ResourceType};
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use reqwest::Url;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::error::{Result, ZeptoError};

use super::web::{is_blocked_host, resolve_and_check_host, validate_redirect_target};
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

fn should_validate_navigation_request(
    resource_type: ResourceType,
    response_status_code: Option<i64>,
) -> bool {
    response_status_code.is_none() && resource_type == ResourceType::Document
}

fn wrap_blocked_navigation_error(url_str: &str, err: ZeptoError) -> ZeptoError {
    match err {
        ZeptoError::SecurityViolation(msg) => ZeptoError::SecurityViolation(format!(
            "Screenshot blocked: browser navigation target '{}' failed SSRF validation: {}",
            url_str, msg
        )),
        other => ZeptoError::Tool(format!(
            "Failed to validate browser navigation target '{}': {}",
            url_str, other
        )),
    }
}

async fn validate_navigation_target(url_str: &str) -> Result<()> {
    let parsed = Url::parse(url_str).map_err(|e| {
        ZeptoError::Tool(format!(
            "Failed to parse browser navigation target '{}': {}",
            url_str, e
        ))
    })?;
    validate_redirect_target(&parsed)
        .await
        .map_err(|e| wrap_blocked_navigation_error(url_str, e))
}

async fn continue_paused_request(
    page: &chromiumoxide::Page,
    request_id: &chromiumoxide::cdp::browser_protocol::fetch::RequestId,
) {
    let _ = page
        .execute(ContinueRequestParams::new(request_id.clone()))
        .await;
}

async fn block_paused_request(
    page: &chromiumoxide::Page,
    request_id: &chromiumoxide::cdp::browser_protocol::fetch::RequestId,
) {
    let _ = page
        .execute(FailRequestParams::new(
            request_id.clone(),
            ErrorReason::BlockedByClient,
        ))
        .await;
}

async fn take_blocked_navigation_error(error: &Mutex<Option<ZeptoError>>) -> Option<ZeptoError> {
    error.lock().await.take()
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
            .enable_request_intercept()
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

        let page = Arc::new(
            browser
                .new_page("about:blank")
                .await
                .map_err(|e| ZeptoError::Tool(format!("Failed to open page: {}", e)))?,
        );
        let blocked_navigation_error = Arc::new(Mutex::new(None::<ZeptoError>));
        let mut request_paused =
            page.event_listener::<EventRequestPaused>()
                .await
                .map_err(|e| {
                    ZeptoError::Tool(format!("Failed to attach request interceptor: {}", e))
                })?;
        let intercept_page = Arc::clone(&page);
        let intercept_error = Arc::clone(&blocked_navigation_error);
        let intercept_handle = tokio::spawn(async move {
            while let Some(event) = request_paused.next().await {
                if !should_validate_navigation_request(
                    event.resource_type.clone(),
                    event.response_status_code,
                ) {
                    continue_paused_request(intercept_page.as_ref(), &event.request_id).await;
                    continue;
                }

                match validate_navigation_target(&event.request.url).await {
                    Ok(()) => {
                        continue_paused_request(intercept_page.as_ref(), &event.request_id).await;
                    }
                    Err(err) => {
                        let mut slot = intercept_error.lock().await;
                        if slot.is_none() {
                            *slot = Some(err);
                        }
                        drop(slot);
                        block_paused_request(intercept_page.as_ref(), &event.request_id).await;
                    }
                }
            }
        });

        // ---- Navigate and screenshot (with timeout) ----
        let screenshot_result = timeout(Duration::from_secs(timeout_secs), async {
            match page.goto(url_str).await {
                Ok(_) => {}
                Err(e) => {
                    if let Some(err) =
                        take_blocked_navigation_error(&blocked_navigation_error).await
                    {
                        return Err(err);
                    }
                    return Err(ZeptoError::Tool(format!("Failed to navigate page: {}", e)));
                }
            }

            if let Some(err) = take_blocked_navigation_error(&blocked_navigation_error).await {
                return Err(err);
            }

            // Defense in depth: validate the final landed URL as well.
            if let Ok(Some(final_url_str)) = page.url().await {
                if let Ok(final_url) = Url::parse(&final_url_str) {
                    validate_redirect_target(&final_url).await?;
                }
            }

            if let Some(err) = take_blocked_navigation_error(&blocked_navigation_error).await {
                return Err(err);
            }

            let screenshot_bytes = match page
                .screenshot(ScreenshotParams::builder().full_page(false).build())
                .await
            {
                Ok(bytes) => bytes,
                Err(e) => {
                    if let Some(err) =
                        take_blocked_navigation_error(&blocked_navigation_error).await
                    {
                        return Err(err);
                    }
                    return Err(ZeptoError::Tool(format!(
                        "Failed to capture screenshot: {}",
                        e
                    )));
                }
            };

            Ok::<Vec<u8>, ZeptoError>(screenshot_bytes)
        })
        .await;

        // Clean up browser resources.
        drop(page);
        drop(browser);
        intercept_handle.abort();
        handler_handle.abort();

        let screenshot_result = screenshot_result.map_err(|_| {
            ZeptoError::Tool(format!(
                "Screenshot timed out after {}s for '{}'",
                timeout_secs, url_str
            ))
        })??;

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
        let tool = WebScreenshotTool;
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

    // ---- Redirect SSRF validation tests ----
    // These test the redirect-target validation functions from web.rs
    // that the screenshot tool uses for intercepted navigation validation.

    #[test]
    fn test_validate_document_request_stage() {
        assert!(should_validate_navigation_request(
            ResourceType::Document,
            None
        ));
    }

    #[test]
    fn test_skip_subresource_request_validation() {
        assert!(!should_validate_navigation_request(
            ResourceType::Image,
            None
        ));
    }

    #[test]
    fn test_skip_response_stage_validation() {
        assert!(!should_validate_navigation_request(
            ResourceType::Document,
            Some(302)
        ));
    }

    #[tokio::test]
    async fn test_validate_navigation_target_blocks_private_url() {
        let result = validate_navigation_target("http://127.0.0.1:8080/private").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_localhost_blocked() {
        let url = Url::parse("http://localhost/admin").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("blocked") || err.contains("Blocked"),
            "Expected redirect block error, got: {}",
            err
        );
    }

    #[test]
    fn test_redirect_to_private_ip_blocked() {
        let url = Url::parse("http://192.168.1.1/admin").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_loopback_blocked() {
        let url = Url::parse("http://127.0.0.1:9090/").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_metadata_endpoint_blocked() {
        let url = Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_ten_network_blocked() {
        let url = Url::parse("http://10.0.0.1/internal").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_172_private_blocked() {
        let url = Url::parse("http://172.16.0.1/").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_dot_local_blocked() {
        let url = Url::parse("http://internal.local/data").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_ftp_scheme_blocked() {
        let url = Url::parse("ftp://evil.com/file").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("scheme"),
            "Expected scheme error, got: {}",
            err
        );
    }

    #[test]
    fn test_redirect_file_scheme_blocked() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_gopher_scheme_blocked() {
        let url = Url::parse("gopher://evil.com/").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_public_https_allowed() {
        let url = Url::parse("https://example.com/page").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_ok());
    }

    #[test]
    fn test_redirect_to_public_http_allowed() {
        let url = Url::parse("http://example.com/page").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_ok());
    }

    #[test]
    fn test_redirect_to_ipv6_loopback_blocked() {
        let url = Url::parse("http://[::1]/admin").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_ipv6_link_local_blocked() {
        let url = Url::parse("http://[fe80::1]/").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_to_zero_ip_blocked() {
        let url = Url::parse("http://0.0.0.0/").unwrap();
        let result = super::super::web::validate_redirect_target_basic(&url);
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
