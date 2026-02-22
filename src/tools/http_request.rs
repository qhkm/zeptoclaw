//! HTTP request tool — lets the agent call external REST APIs.
//! Requires `tools.http_request.allowed_domains` in config.

use crate::error::{Result, ZeptoError};
use crate::tools::web::{is_blocked_host, resolve_and_check_host};
use crate::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use reqwest::{Client, Method, Url};
use serde_json::{json, Value};
use std::time::Duration;

/// Tool that allows the agent to make HTTP requests to external REST APIs.
///
/// Only domains listed in `allowed_domains` config are permitted.
/// Private/local IPs are always blocked via SSRF protection.
pub struct HttpRequestTool {
    allowed_domains: Vec<String>,
    timeout_secs: u64,
    max_response_bytes: usize,
}

impl HttpRequestTool {
    /// Create a new `HttpRequestTool`.
    pub fn new(allowed_domains: Vec<String>, timeout_secs: u64, max_response_bytes: usize) -> Self {
        Self {
            allowed_domains,
            timeout_secs,
            max_response_bytes,
        }
    }

    /// Validate the URL: must be http(s), non-empty, no whitespace, in allowed
    /// domains list, and not pointing to a private/local address.
    pub fn validate_url(&self, raw_url: &str) -> Result<Url> {
        let url = raw_url.trim();
        if url.is_empty() {
            return Err(ZeptoError::Tool("URL cannot be empty".into()));
        }
        if url.chars().any(char::is_whitespace) {
            return Err(ZeptoError::Tool("URL cannot contain whitespace".into()));
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ZeptoError::Tool(
                "Only http:// and https:// URLs are allowed".into(),
            ));
        }
        if self.allowed_domains.is_empty() {
            return Err(ZeptoError::Tool(
                "http_request tool: no allowed_domains configured".into(),
            ));
        }
        let parsed = Url::parse(url).map_err(|e| ZeptoError::Tool(format!("Invalid URL: {e}")))?;
        if is_blocked_host(&parsed) {
            return Err(ZeptoError::Tool(format!(
                "Blocked private/local host: {url}"
            )));
        }
        let host = parsed.host_str().unwrap_or("").to_lowercase();
        if !self.allowed_domains.iter().any(|d| host_matches(d, &host)) {
            return Err(ZeptoError::Tool(format!(
                "Host '{host}' not in allowed_domains"
            )));
        }
        Ok(parsed)
    }

    /// Strip dangerous headers that could be used for host spoofing or credential theft.
    pub fn strip_dangerous_headers(headers: Vec<(String, String)>) -> Vec<(String, String)> {
        let blocked = ["authorization", "host", "cookie", "set-cookie"];
        headers
            .into_iter()
            .filter(|(k, _)| !blocked.contains(&k.to_lowercase().as_str()))
            .collect()
    }
}

/// Check whether `host` matches `pattern`, supporting wildcard subdomains.
/// `*.myco.com` matches `staging.myco.com` and `myco.com` itself.
fn host_matches(pattern: &str, host: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == pattern
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &str {
        "http_request"
    }

    fn description(&self) -> &str {
        "Make an HTTP request to an external API. Supports GET, POST, PUT, PATCH, DELETE. \
         Only domains in tools.http_request.allowed_domains are permitted."
    }

    fn compact_description(&self) -> &str {
        "Make an HTTP request to an allowlisted external API."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["url", "method"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Full URL including scheme, e.g. https://api.example.com/v1/users"
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"],
                    "description": "HTTP method"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers (Authorization, Host, Cookie are stripped)",
                    "additionalProperties": { "type": "string" }
                },
                "body": {
                    "type": "string",
                    "description": "Optional request body (for POST/PUT/PATCH)"
                }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let url_str = args["url"].as_str().unwrap_or("").to_string();
        let method_str = args["method"]
            .as_str()
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter: method".into()))?
            .to_uppercase();

        let parsed = self.validate_url(&url_str)?;

        // DNS-level SSRF check: resolve the hostname and verify it is not
        // private/local.  We keep the returned pinned address so the HTTP
        // client can be told to connect to that exact IP, eliminating the
        // DNS rebinding window between this check and the actual connection.
        let pinned = resolve_and_check_host(&parsed).await?;

        let method = Method::from_bytes(method_str.as_bytes())
            .map_err(|_| ZeptoError::Tool(format!("Unknown HTTP method: {method_str}")))?;

        // Build a client that pins the DNS resolution to the IP we already
        // validated and caps redirects so intermediate hops cannot escape to
        // a private address undetected.
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(5));
        if let Some((host, addr)) = pinned {
            builder = builder.resolve(&host, addr);
        }
        let client = builder
            .build()
            .map_err(|e| ZeptoError::Tool(format!("HTTP client error: {e}")))?;

        let mut req = client.request(method, parsed.as_str());

        if let Some(headers) = args["headers"].as_object() {
            let pairs: Vec<(String, String)> = headers
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
            for (k, v) in Self::strip_dangerous_headers(pairs) {
                req = req.header(&k, &v);
            }
        }

        if let Some(body) = args["body"].as_str() {
            // Auto-set Content-Type to application/json when the body looks
            // like JSON and the caller has not already provided a content-type
            // header (prevents silent broken POSTs where the server rejects an
            // untyped JSON payload).
            let caller_set_ct = args["headers"]
                .as_object()
                .map(|h| h.keys().any(|k| k.to_lowercase() == "content-type"))
                .unwrap_or(false);
            let trimmed = body.trim_start();
            if !caller_set_ct && (trimmed.starts_with('{') || trimmed.starts_with('[')) {
                req = req.header("Content-Type", "application/json");
            }
            req = req.body(body.to_string());
        }

        let response = req
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Request failed: {e}")))?;

        // Post-redirect SSRF check: block redirects to private hosts.
        if is_blocked_host(response.url()) {
            return Err(ZeptoError::Tool(
                "Redirect to private/local host blocked".into(),
            ));
        }

        let status = response.status().as_u16();
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to read response body: {e}")))?;

        let body_str = if body_bytes.len() > self.max_response_bytes {
            let truncated = &body_bytes[..self.max_response_bytes];
            format!(
                "{}\n[TRUNCATED — {} bytes total]",
                String::from_utf8_lossy(truncated),
                body_bytes.len()
            )
        } else {
            String::from_utf8_lossy(&body_bytes).into_owned()
        };

        Ok(ToolOutput::llm_only(format!(
            "Status: {status}\n\n{body_str}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> HttpRequestTool {
        HttpRequestTool::new(
            vec!["api.example.com".to_string(), "*.myco.com".to_string()],
            30,
            512 * 1024,
        )
    }

    #[test]
    fn test_validate_url_rejects_empty() {
        assert!(tool().validate_url("").is_err());
    }

    #[test]
    fn test_validate_url_rejects_non_http() {
        assert!(tool().validate_url("ftp://api.example.com/data").is_err());
    }

    #[test]
    fn test_validate_url_rejects_disallowed_domain() {
        assert!(tool().validate_url("https://evil.com/steal").is_err());
    }

    #[test]
    fn test_validate_url_rejects_private_ip() {
        assert!(tool().validate_url("https://192.168.1.1/admin").is_err());
        assert!(tool().validate_url("https://10.0.0.1/data").is_err());
        assert!(tool().validate_url("https://localhost/api").is_err());
    }

    #[test]
    fn test_validate_url_rejects_whitespace() {
        assert!(tool().validate_url("https://api.example.com /v1").is_err());
    }

    #[test]
    fn test_validate_url_accepts_allowed_domain() {
        assert!(tool()
            .validate_url("https://api.example.com/v1/users")
            .is_ok());
    }

    #[test]
    fn test_validate_url_accepts_wildcard_subdomain() {
        assert!(tool().validate_url("https://staging.myco.com/v1").is_ok());
    }

    #[test]
    fn test_empty_allowed_domains_always_rejects() {
        let t = HttpRequestTool::new(vec![], 30, 512 * 1024);
        assert!(t.validate_url("https://api.example.com/v1").is_err());
    }

    #[test]
    fn test_validate_url_wildcard_does_not_match_same_suffix_non_subdomain() {
        // "evilmyco.com" ends with "myco.com" as a raw string but is NOT a
        // real subdomain — the pattern "*.myco.com" must not match it.
        let t = HttpRequestTool::new(vec!["*.myco.com".to_string()], 30, 512 * 1024);
        assert!(t.validate_url("https://evilmyco.com/steal").is_err());
    }

    #[test]
    fn test_validate_url_wildcard_matches_apex_domain() {
        // "*.myco.com" should also match the apex domain "myco.com" itself,
        // because host_matches() has a `host == suffix` branch.
        let t = HttpRequestTool::new(vec!["*.myco.com".to_string()], 30, 512 * 1024);
        assert!(t.validate_url("https://myco.com/v1").is_ok());
    }

    #[test]
    fn test_strip_dangerous_headers() {
        let headers = vec![
            ("Authorization".to_string(), "Bearer steal-me".to_string()),
            ("Host".to_string(), "evil.com".to_string()),
            ("X-Custom".to_string(), "ok".to_string()),
        ];
        let stripped = HttpRequestTool::strip_dangerous_headers(headers);
        assert_eq!(stripped.len(), 1);
        assert_eq!(stripped[0].0, "X-Custom");
    }
}
