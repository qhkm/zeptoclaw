//! Web screenshot tool (feature-gated behind `screenshot`).
//!
//! Captures screenshots of web pages using a headless Chromium browser
//! via the Chrome DevTools Protocol. Includes full SSRF protection by
//! reusing the validation from [`super::web`].

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, DisableParams as FetchDisableParams, EnableParams as FetchEnableParams,
    EventRequestPaused, FailRequestParams, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
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

/// Maximum allowed redirect hops for the main document navigation.
const MAX_SCREENSHOT_REDIRECT_HOPS: usize = 5;

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
    /// Create a default `WebScreenshotTool` instance.
    fn default() -> Self {
        Self::new()
    }
}

/// Validate a browser-request URL target without performing DNS resolution.
///
/// Ensures the scheme is `http`/`https` and the host is not a blocked local
/// or private address.
fn validate_browser_request_target_basic(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ZeptoError::SecurityViolation(format!(
                "Redirect/browser request scheme is blocked: {}",
                other
            )));
        }
    }

    if is_blocked_host(url) {
        return Err(ZeptoError::SecurityViolation(format!(
            "Redirect/browser request destination is blocked (local or private network): {}",
            url
        )));
    }

    Ok(())
}

/// Validate a browser-request URL including DNS safety checks.
///
/// DNS is re-checked for every intercepted request to narrow the DNS-rebinding
/// window across redirect chains.
async fn validate_browser_request_url(raw_url: &str) -> Result<()> {
    let parsed = Url::parse(raw_url).map_err(|e| {
        ZeptoError::SecurityViolation(format!(
            "Blocked browser request with invalid URL '{}': {}",
            raw_url, e
        ))
    })?;

    validate_browser_request_target_basic(&parsed)?;

    resolve_and_check_host(&parsed).await?;

    Ok(())
}

/// Handle an intercepted browser request during screenshot navigation.
///
/// Safe requests are continued; unsafe requests are failed with
/// `AccessDenied`, and the validation error is propagated.
async fn handle_paused_browser_request(
    page: &chromiumoxide::Page,
    event: &EventRequestPaused,
) -> Result<()> {
    let request_id = event.request_id.clone();
    let request_url = event.request.url.clone();

    match validate_browser_request_url(&request_url).await {
        Ok(()) => {
            page.execute(ContinueRequestParams::new(request_id))
                .await
                .map_err(|e| {
                    ZeptoError::Tool(format!(
                        "Failed to continue intercepted browser request '{}': {}",
                        request_url, e
                    ))
                })?;
            Ok(())
        }
        Err(err) => {
            let _ = page
                .execute(FailRequestParams::new(
                    request_id,
                    ErrorReason::AccessDenied,
                ))
                .await;
            Err(err)
        }
    }
}

/// Normalize a redirect URL string for stable loop tracking comparisons.
fn normalize_redirect_tracking_url(raw_url: &str) -> String {
    Url::parse(raw_url)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| raw_url.to_string())
}

/// Update redirect hop and loop-tracking state for a document redirect.
fn track_document_redirect(
    document_redirect_hops: &mut usize,
    seen_document_redirect_urls: &mut HashSet<String>,
    raw_redirect_url: &str,
) -> Result<()> {
    *document_redirect_hops += 1;
    if *document_redirect_hops > MAX_SCREENSHOT_REDIRECT_HOPS {
        return Err(ZeptoError::SecurityViolation(format!(
            "Exceeded maximum redirect hops ({})",
            MAX_SCREENSHOT_REDIRECT_HOPS
        )));
    }

    let redirected_url = normalize_redirect_tracking_url(raw_redirect_url);
    if !seen_document_redirect_urls.insert(redirected_url.clone()) {
        return Err(ZeptoError::SecurityViolation(format!(
            "Detected redirect loop while capturing screenshot: {}",
            redirected_url
        )));
    }

    Ok(())
}

#[async_trait]
impl Tool for WebScreenshotTool {
    /// Return the tool name.
    fn name(&self) -> &str {
        "web_screenshot"
    }

    /// Describe what this tool does.
    fn description(&self) -> &str {
        "Take a screenshot of a web page. Returns base64-encoded PNG or saves to a file path."
    }

    /// Provide a compact description for constrained UIs.
    fn compact_description(&self) -> &str {
        "Screenshot URL"
    }

    /// Classify this tool for policy enforcement.
    fn category(&self) -> ToolCategory {
        // Fetches URL (NetworkRead) AND writes file to disk — use more restrictive category.
        ToolCategory::FilesystemWrite
    }

    /// Define JSON schema for tool arguments.
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

    /// Execute screenshot capture with browser-native SSRF redirect checks.
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

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

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

        // Initial DNS SSRF check for the entry URL.
        resolve_and_check_host(&parsed).await?;

        let normalized_entry_url = parsed.to_string();

        // ---- Parse optional parameters ----
        let output_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

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

        // ---- Navigate and screenshot (with timeout + browser-side SSRF redirect checks) ----
        let screenshot_result = async {
            // Create an empty page first so we can attach interception before navigation.
            let page = browser
                .new_page("about:blank")
                .await
                .map_err(|e| ZeptoError::Tool(format!("Failed to open page: {}", e)))?;

            let main_frame_id = page
                .mainframe()
                .await
                .map_err(|e| ZeptoError::Tool(format!("Failed to resolve main frame: {}", e)))?;

            let mut paused_events = page
                .event_listener::<EventRequestPaused>()
                .await
                .map_err(|e| {
                    ZeptoError::Tool(format!(
                        "Failed to subscribe to browser request interception events: {}",
                        e
                    ))
                })?;

            // Intercept every request. Redirect hops are surfaced as additional
            // paused requests and validated individually before being continued.
            page.execute(
                FetchEnableParams::builder()
                    .pattern(RequestPattern::builder().url_pattern("*").build())
                    .build(),
            )
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to enable request interception: {}", e)))?;

            let nav_page = page.clone();
            let screenshot_future = async move {
                nav_page
                    .goto(url_str)
                    .await
                    .map_err(|e| ZeptoError::Tool(format!("Failed to open page: {}", e)))?;

                let screenshot_bytes = nav_page
                    .screenshot(ScreenshotParams::builder().full_page(false).build())
                    .await
                    .map_err(|e| ZeptoError::Tool(format!("Failed to capture screenshot: {}", e)))?;

                Ok::<Vec<u8>, ZeptoError>(screenshot_bytes)
            };
            tokio::pin!(screenshot_future);

            let mut document_redirect_hops: usize = 0;
            let mut seen_document_redirect_urls: HashSet<String> = HashSet::new();
            seen_document_redirect_urls.insert(normalized_entry_url.clone());

            let timed_capture = timeout(Duration::from_secs(timeout_secs), async {
                loop {
                    tokio::select! {
                        captured = &mut screenshot_future => {
                            break captured;
                        }
                        maybe_event = paused_events.next() => {
                            let Some(event) = maybe_event else {
                                break Err(ZeptoError::Tool(
                                    "Browser request interception stream ended unexpectedly".to_string()
                                ));
                            };

                            let event_ref = event.as_ref();
                            let is_main_frame = match &main_frame_id {
                                Some(frame_id) => event_ref.frame_id == *frame_id,
                                None => true,
                            };

                            if event_ref.redirected_request_id.is_some()
                                && is_main_frame
                                && matches!(
                                    event_ref.resource_type,
                                    chromiumoxide::cdp::browser_protocol::network::ResourceType::Document
                                )
                            {
                                if let Err(err) = track_document_redirect(
                                    &mut document_redirect_hops,
                                    &mut seen_document_redirect_urls,
                                    &event_ref.request.url,
                                ) {
                                    let _ = page
                                        .execute(FailRequestParams::new(
                                            event_ref.request_id.clone(),
                                            ErrorReason::AccessDenied,
                                        ))
                                        .await;
                                    break Err(err);
                                }
                            }

                            if let Err(err) = handle_paused_browser_request(&page, event_ref).await {
                                break Err(err);
                            }
                        }
                    }
                }
            })
            .await;

            // Best-effort cleanup of interception domain.
            let _ = page.execute(FetchDisableParams::default()).await;

            let capture_result = timed_capture.map_err(|_| {
                ZeptoError::Tool(format!(
                    "Screenshot timed out after {}s for '{}'",
                    timeout_secs, url_str
                ))
            })?;

            capture_result
        }
        .await;

        drop(browser);
        handler_handle.abort();

        let screenshot_result = screenshot_result?;

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

    #[test]
    fn test_validate_browser_request_target_basic_blocks_private_host() {
        let private_target = Url::parse("http://127.0.0.1:8080/admin").unwrap();
        let result = validate_browser_request_target_basic(&private_target);

        assert!(matches!(result, Err(ZeptoError::SecurityViolation(_))));
        match result {
            Err(ZeptoError::SecurityViolation(msg)) => {
                assert!(msg.contains("blocked (local or private network)"));
            }
            other => panic!("expected SecurityViolation, got {other:?}"),
        }
    }

    #[test]
    fn test_validate_browser_request_target_basic_blocks_non_http_scheme() {
        let ftp_target = Url::parse("ftp://example.com/file").unwrap();
        let result = validate_browser_request_target_basic(&ftp_target);

        assert!(matches!(result, Err(ZeptoError::SecurityViolation(_))));
        match result {
            Err(ZeptoError::SecurityViolation(msg)) => {
                assert!(msg.contains("scheme is blocked"));
            }
            other => panic!("expected SecurityViolation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_validate_browser_request_url_blocks_private_host() {
        let result = validate_browser_request_url("http://192.168.1.10/admin").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_track_document_redirect_exceeds_hop_limit() {
        let mut redirect_hops = MAX_SCREENSHOT_REDIRECT_HOPS;
        let mut seen_urls = HashSet::new();
        seen_urls.insert("https://example.com/".to_string());

        let result = track_document_redirect(
            &mut redirect_hops,
            &mut seen_urls,
            "https://example.com/next",
        );

        assert!(matches!(result, Err(ZeptoError::SecurityViolation(_))));
        match result {
            Err(ZeptoError::SecurityViolation(msg)) => {
                assert!(msg.contains("Exceeded maximum redirect hops"));
            }
            other => panic!("expected SecurityViolation, got {other:?}"),
        }
    }

    #[test]
    fn test_track_document_redirect_detects_normalized_loop() {
        let mut redirect_hops = 0;
        let mut seen_urls = HashSet::new();
        seen_urls.insert("https://example.com/".to_string());

        let result =
            track_document_redirect(&mut redirect_hops, &mut seen_urls, "HTTPS://EXAMPLE.COM");

        assert!(matches!(result, Err(ZeptoError::SecurityViolation(_))));
        match result {
            Err(ZeptoError::SecurityViolation(msg)) => {
                assert!(msg.contains("Detected redirect loop while capturing screenshot"));
                assert!(msg.contains("https://example.com/"));
            }
            other => panic!("expected SecurityViolation, got {other:?}"),
        }
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
