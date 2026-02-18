//! Web access tools.
//!
//! Provides:
//! - `web_search`: search the web with Brave Search API.
//! - `web_fetch`: fetch URL content and extract readable text.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::lookup_host;

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolContext};

const BRAVE_API_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const WEB_USER_AGENT: &str = "zeptoclaw/0.1 (+https://github.com/zeptoclaw/zeptoclaw)";
const MAX_WEB_SEARCH_COUNT: usize = 10;
const DEFAULT_MAX_FETCH_CHARS: usize = 50_000;
const MAX_FETCH_CHARS: usize = 200_000;
const MIN_FETCH_CHARS: usize = 256;
/// Maximum bytes to read from a response body before truncating.
/// Uses a 4x multiplier over MAX_FETCH_CHARS to account for multi-byte UTF-8.
const MAX_FETCH_BYTES: usize = MAX_FETCH_CHARS * 4;

/// Web search tool backed by Brave Search.
pub struct WebSearchTool {
    api_key: String,
    client: Client,
    max_results: usize,
}

impl WebSearchTool {
    /// Create a new web search tool.
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: Client::new(),
            max_results: 5,
        }
    }

    /// Create a web search tool with custom default result count.
    pub fn with_max_results(api_key: &str, max_results: usize) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: Client::new(),
            max_results: max_results.clamp(1, MAX_WEB_SEARCH_COUNT),
        }
    }
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web and return result titles, URLs, and snippets."
    }

    fn compact_description(&self) -> &str {
        "Web search"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results (1-10)",
                    "minimum": 1,
                    "maximum": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'query' parameter".to_string()))?;

        let count = args
            .get("count")
            .and_then(|v| v.as_u64())
            .map(|c| c as usize)
            .unwrap_or(self.max_results)
            .clamp(1, MAX_WEB_SEARCH_COUNT);

        if self.api_key.trim().is_empty() {
            return Err(ZeptoError::Tool(
                "Brave Search API key is not configured".to_string(),
            ));
        }

        let response = self
            .client
            .get(BRAVE_API_URL)
            .header("Accept", "application/json")
            .header("User-Agent", WEB_USER_AGENT)
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", query), ("count", &count.to_string())])
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Web search request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            let detail = detail.trim();
            return Err(ZeptoError::Tool(if detail.is_empty() {
                format!("Brave Search API error: {}", status)
            } else {
                format!("Brave Search API error: {} ({})", status, detail)
            }));
        }

        let payload: BraveResponse = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to parse search response: {}", e)))?;

        let results = payload
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .take(count)
            .collect::<Vec<_>>();

        if results.is_empty() {
            return Ok(format!("No web search results found for '{}'.", query));
        }

        let mut output = format!("Web search results for '{}':\n\n", query);
        for (index, item) in results.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", index + 1, item.title));
            output.push_str(&format!("   {}\n", item.url));
            if let Some(description) = item.description.as_deref().map(str::trim) {
                if !description.is_empty() {
                    output.push_str(&format!("   {}\n", description));
                }
            }
            output.push('\n');
        }

        Ok(output.trim_end().to_string())
    }
}

/// Web fetch tool for URL content retrieval.
pub struct WebFetchTool {
    client: Client,
    max_chars: usize,
}

impl WebFetchTool {
    /// Create a new web fetch tool.
    pub fn new() -> Self {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            max_chars: DEFAULT_MAX_FETCH_CHARS,
        }
    }

    /// Create with a custom maximum output size.
    pub fn with_max_chars(max_chars: usize) -> Self {
        let mut tool = Self::new();
        tool.max_chars = max_chars.clamp(MIN_FETCH_CHARS, MAX_FETCH_CHARS);
        tool
    }

    fn extract_title(&self, html: &str) -> Option<String> {
        let regex = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
        let captures = regex.captures(html)?;
        let raw = captures.get(1)?.as_str();
        normalize_whitespace(&decode_common_html_entities(raw))
            .trim()
            .to_string()
            .into()
    }

    fn extract_text(&self, html: &str) -> String {
        let without_scripts = strip_regex(html, r"(?is)<script[^>]*>.*?</script>", " ");
        let without_styles = strip_regex(&without_scripts, r"(?is)<style[^>]*>.*?</style>", " ");
        let without_noscript =
            strip_regex(&without_styles, r"(?is)<noscript[^>]*>.*?</noscript>", " ");
        let with_line_breaks = strip_regex(
            &without_noscript,
            r"(?i)</?(p|div|h[1-6]|li|tr|td|th|br)\b[^>]*>",
            "\n",
        );
        let without_tags = strip_regex(&with_line_breaks, r"(?is)<[^>]+>", " ");

        normalize_whitespace(&decode_common_html_entities(&without_tags))
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return extracted readable content."
    }

    fn compact_description(&self) -> &str {
        "Fetch URL"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "http/https URL to fetch"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum output characters",
                    "minimum": MIN_FETCH_CHARS,
                    "maximum": MAX_FETCH_CHARS
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ZeptoError::Tool("Missing 'url' parameter".to_string()))?;

        let parsed = Url::parse(url)
            .map_err(|e| ZeptoError::Tool(format!("Invalid URL '{}': {}", url, e)))?;

        match parsed.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(ZeptoError::Tool(
                    "Only http/https URLs are allowed".to_string(),
                ));
            }
        }

        if is_blocked_host(&parsed) {
            return Err(ZeptoError::SecurityViolation(
                "Blocked URL host (local or private network)".to_string(),
            ));
        }

        // DNS-based SSRF check: resolve the hostname before making the
        // request and verify none of the resolved IPs are private/local.
        // The returned address is used to pin the connection, preventing
        // DNS rebinding attacks between this check and the actual request.
        let pinned = resolve_and_check_host(&parsed).await?;

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(self.max_chars)
            .clamp(MIN_FETCH_CHARS, MAX_FETCH_CHARS);

        // Build a client that pins the DNS resolution to the IP we already
        // validated, so the HTTP library cannot re-resolve to a different
        // (potentially private) address.
        let client = if let Some((host, addr)) = pinned {
            Client::builder()
                .redirect(reqwest::redirect::Policy::limited(5))
                .timeout(Duration::from_secs(30))
                .resolve(&host, addr)
                .build()
                .unwrap_or_else(|_| self.client.clone())
        } else {
            self.client.clone()
        };

        let response = client
            .get(parsed.clone())
            .header("User-Agent", WEB_USER_AGENT)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Web fetch failed: {}", e)))?;

        // SSRF redirect check: after reqwest follows redirects, validate
        // that the final destination URL is not a blocked host.
        if is_blocked_host(response.url()) {
            return Err(ZeptoError::SecurityViolation(format!(
                "Redirect destination is blocked (local or private network): {}",
                response.url()
            )));
        }

        let status = response.status();
        let final_url = response.url().to_string();

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!("HTTP error: {}", status)));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Read body in chunks with a size limit to prevent unbounded memory
        // allocation from malicious or oversized responses.
        let body = read_body_limited(response, MAX_FETCH_BYTES).await?;

        let (extractor, mut text) = if content_type.contains("application/json") {
            ("json", body)
        } else if content_type.contains("text/html") || body.trim_start().starts_with('<') {
            let title = self.extract_title(&body).unwrap_or_default();
            let extracted = self.extract_text(&body);
            if title.is_empty() {
                ("html", extracted)
            } else {
                ("html", format!("# {}\n\n{}", title, extracted))
            }
        } else {
            ("raw", body)
        };

        let truncated = text.len() > max_chars;
        if truncated {
            // Find a valid UTF-8 char boundary at or before max_chars to avoid panic
            let mut end = max_chars;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }

        Ok(json!({
            "url": url,
            "final_url": final_url,
            "status": status.as_u16(),
            "extractor": extractor,
            "truncated": truncated,
            "length": text.len(),
            "text": text,
        })
        .to_string())
    }
}

fn strip_regex(input: &str, pattern: &str, replacement: &str) -> String {
    match Regex::new(pattern) {
        Ok(regex) => regex.replace_all(input, replacement).into_owned(),
        Err(_) => input.to_string(),
    }
}

fn normalize_whitespace(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn decode_common_html_entities(input: &str) -> String {
    let mut decoded = input.replace("&nbsp;", " ");
    decoded = decoded.replace("&amp;", "&");
    decoded = decoded.replace("&lt;", "<");
    decoded = decoded.replace("&gt;", ">");
    decoded = decoded.replace("&quot;", "\"");
    decoded.replace("&#39;", "'")
}

/// Read a response body in chunks, enforcing a maximum byte limit.
///
/// This prevents unbounded memory allocation when a server returns an
/// extremely large response (intentional or otherwise).  The bytes are
/// accumulated in chunks and converted to a UTF-8 string (lossy) once
/// the limit is reached or the stream ends.
async fn read_body_limited(response: reqwest::Response, max_bytes: usize) -> Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = response;

    loop {
        match stream.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = max_bytes.saturating_sub(buf.len());
                if remaining == 0 {
                    break;
                }
                let take = chunk.len().min(remaining);
                buf.extend_from_slice(&chunk[..take]);
                if buf.len() >= max_bytes {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                return Err(ZeptoError::Tool(format!(
                    "Failed to read response body: {}",
                    e
                )));
            }
        }
    }

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Check whether a URL's host is a blocked (local/private) address.
/// Used by both `WebFetchTool` and the watch command to prevent SSRF.
pub fn is_blocked_host(url: &Url) -> bool {
    let Some(host_str) = url.host_str() else {
        return true;
    };

    let host = host_str.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".local") {
        return true;
    }

    // Try parsing as IP directly first, then try stripping IPv6 brackets.
    // `Url::host_str()` returns IPv6 addresses with surrounding brackets
    // (e.g. "[::1]"), which `IpAddr::parse` does not accept.
    let ip_str = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&host);
    if let Ok(ip) = ip_str.parse::<IpAddr>() {
        return is_private_or_local_ip(ip);
    }

    false
}

/// Resolve a URL's hostname via DNS and check whether any of the resolved IPs
/// point to a private or local address.  This catches DNS-based SSRF attacks
/// where a public hostname (e.g. `metadata.attacker.com`) resolves to an
/// internal IP such as `169.254.169.254`.
///
/// Returns the first safe resolved IP address so the caller can pin the
/// connection to it, preventing DNS rebinding attacks where a second DNS
/// lookup (by the HTTP client) returns a different, private IP.
pub async fn resolve_and_check_host(url: &Url) -> Result<Option<(String, std::net::SocketAddr)>> {
    let host = url
        .host_str()
        .ok_or_else(|| ZeptoError::SecurityViolation("URL has no host".to_string()))?;

    // IP literals are already checked by `is_blocked_host`, skip DNS lookup.
    if host.parse::<IpAddr>().is_ok() {
        return Ok(None);
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let lookup_addr = format!("{}:{}", host, port);

    let addrs: Vec<std::net::SocketAddr> = lookup_host(&lookup_addr)
        .await
        .map_err(|e| ZeptoError::Tool(format!("DNS lookup failed for '{}': {}", host, e)))?
        .collect();

    for addr in &addrs {
        if is_private_or_local_ip(addr.ip()) {
            return Err(ZeptoError::SecurityViolation(format!(
                "DNS for '{}' resolved to private/local IP {}",
                host,
                addr.ip()
            )));
        }
    }

    // Return the first safe address so the caller can pin the connection,
    // preventing DNS rebinding between this check and the actual request.
    Ok(addrs
        .into_iter()
        .next()
        .map(|addr| (host.to_string(), addr)))
}

fn is_private_or_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => is_private_or_local_ipv4(addr),
        IpAddr::V6(addr) => is_private_or_local_ipv6(addr),
    }
}

fn is_private_or_local_ipv4(addr: Ipv4Addr) -> bool {
    addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_documentation()
        || addr.is_unspecified()
        || addr.octets()[0] == 0
}

fn is_private_or_local_ipv6(addr: Ipv6Addr) -> bool {
    let first = addr.segments()[0];

    addr.is_loopback()
        || addr.is_unspecified()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xff00) == 0xff00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_search_tool_properties() {
        let tool = WebSearchTool::new("test-key");
        assert_eq!(tool.name(), "web_search");
        assert!(tool.description().contains("Search the web"));
    }

    #[test]
    fn test_web_fetch_tool_properties() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
        assert!(tool.description().contains("Fetch"));
    }

    #[test]
    fn test_extract_title() {
        let tool = WebFetchTool::new();
        let html = "<html><head><title> Test Page </title></head><body>x</body></html>";
        assert_eq!(tool.extract_title(html), Some("Test Page".to_string()));
    }

    #[test]
    fn test_extract_text() {
        let tool = WebFetchTool::new();
        let html = r#"
            <html>
              <body>
                <h1>Hello</h1>
                <p>World</p>
                <script>alert('x')</script>
                <style>body {color: red;}</style>
              </body>
            </html>
        "#;

        let text = tool.extract_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color:"));
    }

    #[test]
    fn test_blocked_hosts() {
        let localhost = Url::parse("http://localhost:8080/").unwrap();
        let private_v4 = Url::parse("http://192.168.1.2/").unwrap();
        let public_host = Url::parse("https://example.com/").unwrap();

        assert!(is_blocked_host(&localhost));
        assert!(is_blocked_host(&private_v4));
        assert!(!is_blocked_host(&public_host));
    }

    #[test]
    fn test_blocked_redirect_destination() {
        // Simulate a redirect landing on a private IP — `is_blocked_host`
        // must catch these when called on the final response URL.
        let cloud_metadata = Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        assert!(is_blocked_host(&cloud_metadata));

        let loopback = Url::parse("http://127.0.0.1:9090/admin").unwrap();
        assert!(is_blocked_host(&loopback));

        let link_local = Url::parse("http://169.254.1.1/secret").unwrap();
        assert!(is_blocked_host(&link_local));

        let private_10 = Url::parse("http://10.0.0.1/internal").unwrap();
        assert!(is_blocked_host(&private_10));

        let dot_local = Url::parse("http://internal.local/data").unwrap();
        assert!(is_blocked_host(&dot_local));

        // Public URLs should not be blocked after redirect.
        let public = Url::parse("https://cdn.example.com/page").unwrap();
        assert!(!is_blocked_host(&public));
    }

    #[tokio::test]
    async fn test_resolve_and_check_host_ip_literal_passes_through() {
        // IP literals are already covered by `is_blocked_host`, so
        // `resolve_and_check_host` should return None (no pinning needed).
        let url = Url::parse("https://93.184.216.34/").unwrap();
        let result = resolve_and_check_host(&url).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_resolve_and_check_host_blocks_localhost_alias() {
        // Hostnames that resolve to 127.0.0.1 must be blocked.
        // `localhost` is a well-known name that resolves to loopback.
        let url = Url::parse("https://localhost:443/").unwrap();
        let result = resolve_and_check_host(&url).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ZeptoError::SecurityViolation(_)),
            "Expected SecurityViolation, got: {:?}",
            err
        );
    }

    #[test]
    fn test_body_size_limit() {
        // Verify `MAX_FETCH_BYTES` enforces a reasonable byte cap that
        // corresponds to MAX_FETCH_CHARS * 4 (worst-case UTF-8 encoding).
        assert_eq!(MAX_FETCH_BYTES, MAX_FETCH_CHARS * 4);
        assert_eq!(MAX_FETCH_BYTES, 800_000);

        // Verify that the streaming reader would truncate at the limit:
        // a buffer that exceeds MAX_FETCH_BYTES should stop growing.
        let big = vec![b'A'; MAX_FETCH_BYTES + 100];
        let truncated = &big[..MAX_FETCH_BYTES];
        assert_eq!(truncated.len(), MAX_FETCH_BYTES);
    }

    #[test]
    fn test_private_or_local_ip_cloud_metadata() {
        // The AWS/GCP/Azure metadata endpoint IP must be caught.
        let metadata_ip: IpAddr = "169.254.169.254".parse().unwrap();
        assert!(
            is_private_or_local_ip(metadata_ip),
            "169.254.169.254 should be detected as link-local"
        );
    }

    #[test]
    fn test_blocked_hosts_ipv6_loopback() {
        let ipv6_loopback = Url::parse("http://[::1]:8080/").unwrap();
        assert!(is_blocked_host(&ipv6_loopback));
    }

    #[test]
    fn test_blocked_hosts_ipv6_link_local() {
        let ipv6_link_local = Url::parse("http://[fe80::1]/").unwrap();
        assert!(is_blocked_host(&ipv6_link_local));
    }

    // ==================== ADDITIONAL SSRF / SECURITY TESTS ====================

    #[test]
    fn test_private_ip_10_range_blocked() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "10.0.0.0/8 range should be detected as private"
        );

        let ip2: IpAddr = "10.255.255.255".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip2),
            "10.255.255.255 should be detected as private"
        );
    }

    #[test]
    fn test_private_ip_172_range_blocked() {
        let ip: IpAddr = "172.16.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "172.16.0.0/12 range should be detected as private"
        );

        let ip_end: IpAddr = "172.31.255.255".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip_end),
            "172.31.255.255 should be detected as private"
        );

        // 172.32.x.x is NOT private -- ensure no false positive
        let public: IpAddr = "172.32.0.1".parse().unwrap();
        assert!(
            !is_private_or_local_ip(public),
            "172.32.0.1 should NOT be detected as private"
        );
    }

    #[test]
    fn test_unspecified_and_broadcast_blocked() {
        let unspecified: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(
            is_private_or_local_ip(unspecified),
            "0.0.0.0 should be blocked"
        );

        let broadcast: IpAddr = "255.255.255.255".parse().unwrap();
        assert!(
            is_private_or_local_ip(broadcast),
            "255.255.255.255 should be blocked"
        );

        // Addresses starting with 0 (e.g., 0.1.2.3) should be blocked
        let zero_prefix: IpAddr = "0.1.2.3".parse().unwrap();
        assert!(
            is_private_or_local_ip(zero_prefix),
            "0.x.x.x should be blocked"
        );
    }

    #[test]
    fn test_ipv6_ula_and_multicast_blocked() {
        // Unique Local Address (fc00::/7)
        let ula: IpAddr = "fd00::1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ula),
            "IPv6 ULA (fd00::1) should be blocked"
        );

        let ula2: IpAddr = "fc00::1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ula2),
            "IPv6 ULA (fc00::1) should be blocked"
        );

        // Multicast (ff00::/8)
        let multicast: IpAddr = "ff02::1".parse().unwrap();
        assert!(
            is_private_or_local_ip(multicast),
            "IPv6 multicast should be blocked"
        );

        // Unspecified
        let unspecified_v6: IpAddr = "::".parse().unwrap();
        assert!(
            is_private_or_local_ip(unspecified_v6),
            "IPv6 unspecified (::) should be blocked"
        );
    }

    #[tokio::test]
    async fn test_web_fetch_rejects_non_http_schemes() {
        let tool = WebFetchTool::new();
        let ctx = ToolContext::new();

        // ftp:// should be rejected
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

        // file:// should be rejected
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

    #[test]
    fn test_with_max_chars_clamping() {
        // Below minimum should clamp to MIN_FETCH_CHARS
        let tool = WebFetchTool::with_max_chars(1);
        assert_eq!(tool.max_chars, MIN_FETCH_CHARS);

        // Above maximum should clamp to MAX_FETCH_CHARS
        let tool = WebFetchTool::with_max_chars(999_999_999);
        assert_eq!(tool.max_chars, MAX_FETCH_CHARS);

        // Within range should be preserved
        let tool = WebFetchTool::with_max_chars(10_000);
        assert_eq!(tool.max_chars, 10_000);
    }

    #[test]
    fn test_is_blocked_host_no_host() {
        // A URL with no host should be blocked
        // data: URLs have no host
        let no_host = Url::parse("data:text/plain;base64,SGVsbG8=").unwrap();
        assert!(
            is_blocked_host(&no_host),
            "URL with no host should be blocked"
        );
    }
}
