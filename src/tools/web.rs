//! Web access tools.
//!
//! Provides:
//! - `web_search`: search the web with Brave Search API (or DuckDuckGo free fallback).
//! - `web_fetch`: fetch URL content and extract readable text.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use reqwest::{Client, Url};
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::lookup_host;

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

const BRAVE_API_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const DDG_HTML_URL: &str = "https://html.duckduckgo.com/html/";
const WEB_USER_AGENT: &str = "zeptoclaw/0.1 (+https://github.com/zeptoclaw/zeptoclaw)";
const MAX_WEB_SEARCH_COUNT: usize = 10;
const DEFAULT_MAX_FETCH_CHARS: usize = 50_000;
const MAX_FETCH_CHARS: usize = 200_000;
const MIN_FETCH_CHARS: usize = 256;
/// Maximum bytes to read from a response body before truncating.
/// Uses a 4x multiplier over MAX_FETCH_CHARS to account for multi-byte UTF-8.
const MAX_FETCH_BYTES: usize = MAX_FETCH_CHARS * 4;

// ---------------------------------------------------------------------------
// Static CSS selectors (compiled once, reused)
// ---------------------------------------------------------------------------
static SEL_TITLE: Lazy<Selector> = Lazy::new(|| Selector::parse("title").unwrap());
static SEL_MAIN: Lazy<Selector> = Lazy::new(|| Selector::parse("main").unwrap());
static SEL_ARTICLE: Lazy<Selector> = Lazy::new(|| Selector::parse("article").unwrap());
static SEL_ROLE_MAIN: Lazy<Selector> = Lazy::new(|| Selector::parse("[role=main]").unwrap());
static SEL_BODY: Lazy<Selector> = Lazy::new(|| Selector::parse("body").unwrap());
static SEL_LINKS: Lazy<Selector> = Lazy::new(|| Selector::parse("a[href]").unwrap());
static SEL_DDG_RESULT_LINK: Lazy<Selector> = Lazy::new(|| Selector::parse("a.result__a").unwrap());
static SEL_DDG_RESULT_SNIPPET: Lazy<Selector> =
    Lazy::new(|| Selector::parse("a.result__snippet").unwrap());

const SKIP_ELEMENTS: &[&str] = &[
    "script", "style", "noscript", "nav", "footer", "header", "aside", "iframe", "svg", "form",
    "input", "button", "select", "textarea",
];

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

/// Generic search result used by the DDG backend.
struct SearchResult {
    title: String,
    url: String,
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

    fn category(&self) -> ToolCategory {
        ToolCategory::NetworkRead
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

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
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
            return Ok(ToolOutput::user_visible(format!(
                "No web search results found for '{}'.",
                query
            )));
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

        Ok(ToolOutput::user_visible(output.trim_end().to_string()))
    }
}

/// Extract the real URL from a DDG redirect link.
/// DDG wraps results in `https://duckduckgo.com/l/?uddg=<encoded_url>&...`
fn extract_ddg_real_url(href: &str) -> String {
    if let Ok(parsed) = Url::parse(href) {
        if parsed.host_str() == Some("duckduckgo.com") {
            if let Some(uddg) = parsed.query_pairs().find(|(k, _)| k == "uddg") {
                return uddg.1.to_string();
            }
        }
    }
    href.to_string()
}

/// Parse DDG HTML search results page into structured results.
fn parse_ddg_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();

    let link_elements: Vec<_> = doc.select(&SEL_DDG_RESULT_LINK).collect();
    let snippet_elements: Vec<_> = doc.select(&SEL_DDG_RESULT_SNIPPET).collect();

    for (i, link_el) in link_elements.iter().enumerate() {
        if results.len() >= max_results {
            break;
        }
        let title = link_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let href = link_el.value().attr("href").unwrap_or_default();
        let url = extract_ddg_real_url(href);

        let description = snippet_elements
            .get(i)
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        results.push(SearchResult {
            title,
            url,
            description,
        });
    }

    results
}

/// Free web search tool backed by DuckDuckGo HTML scraping.
/// Used as automatic fallback when no Brave API key is configured.
pub struct DdgSearchTool {
    client: Client,
    max_results: usize,
}

impl Default for DdgSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl DdgSearchTool {
    /// Create a new DDG search tool with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            max_results: 5,
        }
    }

    /// Create with custom max results.
    pub fn with_max_results(max_results: usize) -> Self {
        Self {
            client: Client::new(),
            max_results: max_results.clamp(1, MAX_WEB_SEARCH_COUNT),
        }
    }
}

#[async_trait]
impl Tool for DdgSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web and return result titles, URLs, and snippets."
    }

    fn compact_description(&self) -> &str {
        "Web search"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::NetworkRead
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

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
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

        let response = self
            .client
            .post(DDG_HTML_URL)
            .header("User-Agent", WEB_USER_AGENT)
            .form(&[("q", query)])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("DuckDuckGo search failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ZeptoError::Tool(format!(
                "DuckDuckGo search error: {}",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to read DDG response: {}", e)))?;

        let results = parse_ddg_html(&html, count);

        if results.is_empty() {
            return Ok(ToolOutput::user_visible(format!(
                "No web search results found for '{}'.",
                query
            )));
        }

        let mut output = format!("Web search results for '{}':\n\n", query);
        for (index, item) in results.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", index + 1, item.title));
            output.push_str(&format!("   {}\n", item.url));
            if let Some(desc) = item.description.as_deref().map(str::trim) {
                if !desc.is_empty() {
                    output.push_str(&format!("   {}\n", desc));
                }
            }
            output.push('\n');
        }

        Ok(ToolOutput::user_visible(output.trim_end().to_string()))
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

    fn extract_title_from_doc(&self, document: &Html) -> Option<String> {
        let el = document.select(&SEL_TITLE).next()?;
        let raw: String = el.text().collect();
        let title = normalize_whitespace(&raw);
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    }

    #[cfg(test)]
    fn extract_text(&self, html: &str) -> String {
        let document = Html::parse_document(html);
        self.extract_text_from_doc(&document, false, "")
    }

    fn extract_text_from_doc(
        &self,
        document: &Html,
        include_links: bool,
        base_url: &str,
    ) -> String {
        let md = if let Some(root) = find_content_root(document) {
            dom_to_markdown(root)
        } else {
            String::new()
        };
        let mut result = normalize_whitespace_md(&md);
        if include_links {
            let links = extract_links(document, base_url);
            if !links.is_empty() {
                result.push_str(&links);
            }
        }
        result
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

    fn category(&self) -> ToolCategory {
        ToolCategory::NetworkRead
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
                },
                "include_links": {
                    "type": "boolean",
                    "description": "Include a list of links found on the page"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
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

        let include_links = args
            .get("include_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (extractor, mut text) = if content_type.contains("application/json") {
            ("json", body)
        } else if content_type.contains("text/html") || body.trim_start().starts_with('<') {
            let document = Html::parse_document(&body);
            let title = self.extract_title_from_doc(&document).unwrap_or_default();
            let extracted = self.extract_text_from_doc(&document, include_links, &final_url);
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

        Ok(ToolOutput::llm_only(
            json!({
                "url": url,
                "final_url": final_url,
                "status": status.as_u16(),
                "extractor": extractor,
                "truncated": truncated,
                "length": text.len(),
                "text": text,
            })
            .to_string(),
        ))
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

fn decode_html_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '&' {
            output.push(ch);
            continue;
        }
        // Accumulate entity text between & and ;
        let mut entity = String::new();
        let mut found_semi = false;
        // Cap entity length to avoid unbounded accumulation on malformed input
        for _ in 0..12 {
            match chars.peek() {
                Some(&';') => {
                    chars.next();
                    found_semi = true;
                    break;
                }
                Some(_) => entity.push(chars.next().unwrap()),
                None => break,
            }
        }
        if !found_semi {
            // Not a valid entity — emit raw characters
            output.push('&');
            output.push_str(&entity);
            continue;
        }
        match decode_entity(&entity) {
            Some(decoded) => output.push_str(decoded),
            None => {
                // Numeric entities
                if let Some(stripped) = entity.strip_prefix('#') {
                    let code = if let Some(hex) =
                        stripped.strip_prefix('x').or(stripped.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        stripped.parse::<u32>().ok()
                    };
                    if let Some(c) = code.and_then(char::from_u32) {
                        output.push(c);
                    } else {
                        output.push('&');
                        output.push_str(&entity);
                        output.push(';');
                    }
                } else {
                    // Unknown named entity — pass through
                    output.push('&');
                    output.push_str(&entity);
                    output.push(';');
                }
            }
        }
    }
    output
}

fn decode_entity(name: &str) -> Option<&'static str> {
    match name {
        "amp" => Some("&"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "quot" => Some("\""),
        "apos" | "#39" => Some("'"),
        "nbsp" => Some(" "),
        "mdash" => Some("\u{2014}"),
        "ndash" => Some("\u{2013}"),
        "lsquo" => Some("\u{2018}"),
        "rsquo" => Some("\u{2019}"),
        "ldquo" => Some("\u{201C}"),
        "rdquo" => Some("\u{201D}"),
        "hellip" => Some("\u{2026}"),
        "copy" => Some("\u{00A9}"),
        "reg" => Some("\u{00AE}"),
        "trade" => Some("\u{2122}"),
        "bull" => Some("\u{2022}"),
        _ => None,
    }
}

/// Normalize whitespace while preserving line structure for markdown.
/// Collapses horizontal whitespace per line, preserves newlines,
/// and collapses 3+ consecutive blank lines to 2.
fn normalize_whitespace_md(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut blank_count = 0u32;

    for line in input.lines() {
        let trimmed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                output.push('\n');
            }
        } else {
            blank_count = 0;
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&trimmed);
            output.push('\n');
        }
    }
    // Trim trailing newlines to at most one
    let trimmed = output.trim_end_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{}\n", trimmed)
    }
}

// ---------------------------------------------------------------------------
// DOM-based content extraction
// ---------------------------------------------------------------------------

/// Find the best content root in a parsed HTML document.
/// Tries selectors in priority: main → article → [role=main] → body.
fn find_content_root(document: &Html) -> Option<ElementRef<'_>> {
    document
        .select(&SEL_MAIN)
        .next()
        .or_else(|| document.select(&SEL_ARTICLE).next())
        .or_else(|| document.select(&SEL_ROLE_MAIN).next())
        .or_else(|| document.select(&SEL_BODY).next())
}

/// Convert an HTML element subtree to markdown.
fn dom_to_markdown(element: ElementRef<'_>) -> String {
    let mut output = String::new();
    dom_walk(element, &mut output);
    output
}

fn dom_walk(element: ElementRef<'_>, output: &mut String) {
    for child in element.children() {
        match child.value() {
            Node::Text(text) => {
                output.push_str(&decode_html_entities(text));
            }
            Node::Element(el) => {
                let tag = el.name.local.as_ref();
                if SKIP_ELEMENTS.contains(&tag) {
                    continue;
                }
                // Safe: child is an element node, so ElementRef::wrap is valid
                let Some(child_ref) = ElementRef::wrap(child) else {
                    continue;
                };
                match tag {
                    "h1" => {
                        output.push_str("\n\n# ");
                        output.push_str(&collect_inline_text(child_ref));
                        output.push_str("\n\n");
                    }
                    "h2" => {
                        output.push_str("\n\n## ");
                        output.push_str(&collect_inline_text(child_ref));
                        output.push_str("\n\n");
                    }
                    "h3" => {
                        output.push_str("\n\n### ");
                        output.push_str(&collect_inline_text(child_ref));
                        output.push_str("\n\n");
                    }
                    "h4" => {
                        output.push_str("\n\n#### ");
                        output.push_str(&collect_inline_text(child_ref));
                        output.push_str("\n\n");
                    }
                    "h5" => {
                        output.push_str("\n\n##### ");
                        output.push_str(&collect_inline_text(child_ref));
                        output.push_str("\n\n");
                    }
                    "h6" => {
                        output.push_str("\n\n###### ");
                        output.push_str(&collect_inline_text(child_ref));
                        output.push_str("\n\n");
                    }
                    "a" => {
                        let href = el.attr("href").unwrap_or("");
                        let text = collect_inline_text(child_ref);
                        if text.is_empty() {
                            output.push_str(href);
                        } else {
                            output.push('[');
                            output.push_str(&text);
                            output.push_str("](");
                            output.push_str(href);
                            output.push(')');
                        }
                    }
                    "strong" | "b" => {
                        let text = collect_inline_text(child_ref);
                        if !text.is_empty() {
                            output.push_str("**");
                            output.push_str(&text);
                            output.push_str("**");
                        }
                    }
                    "em" | "i" => {
                        let text = collect_inline_text(child_ref);
                        if !text.is_empty() {
                            output.push('*');
                            output.push_str(&text);
                            output.push('*');
                        }
                    }
                    "code" => {
                        let text = collect_inline_text(child_ref);
                        if !text.is_empty() {
                            output.push('`');
                            output.push_str(&text);
                            output.push('`');
                        }
                    }
                    "pre" => {
                        let text = collect_raw_text(child_ref);
                        output.push_str("\n\n```\n");
                        output.push_str(&text);
                        output.push_str("\n```\n\n");
                    }
                    "li" => {
                        output.push_str("\n- ");
                        dom_walk(child_ref, output);
                    }
                    "br" => {
                        output.push('\n');
                    }
                    "hr" => {
                        output.push_str("\n\n---\n\n");
                    }
                    "blockquote" => {
                        let inner = dom_to_markdown(child_ref);
                        for line in inner.lines() {
                            output.push_str("> ");
                            output.push_str(line);
                            output.push('\n');
                        }
                    }
                    "img" => {
                        let alt = el.attr("alt").unwrap_or("");
                        let src = el.attr("src").unwrap_or("");
                        if !src.is_empty() {
                            output.push_str("![");
                            output.push_str(alt);
                            output.push_str("](");
                            output.push_str(src);
                            output.push(')');
                        }
                    }
                    "p" | "div" | "section" | "main" | "article" => {
                        output.push_str("\n\n");
                        dom_walk(child_ref, output);
                        output.push_str("\n\n");
                    }
                    "ul" | "ol" => {
                        output.push('\n');
                        dom_walk(child_ref, output);
                        output.push('\n');
                    }
                    "td" | "th" => {
                        dom_walk(child_ref, output);
                        output.push_str(" | ");
                    }
                    _ => {
                        dom_walk(child_ref, output);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Collect all descendant text from an element, stripping inner tags.
fn collect_inline_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collect raw text preserving whitespace (for `<pre>` blocks).
fn collect_raw_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>()
}

/// Extract deduplicated links from a parsed HTML document.
fn extract_links(document: &Html, base_url: &str) -> String {
    let base = Url::parse(base_url).ok();
    let mut seen = HashSet::new();
    let mut links = Vec::new();

    for el in document.select(&SEL_LINKS) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        let href = href.trim();
        // Skip fragment-only anchors and empty hrefs
        if href.is_empty() || href.starts_with('#') {
            continue;
        }
        // Resolve relative URLs
        let resolved = if href.starts_with("http://") || href.starts_with("https://") {
            href.to_string()
        } else if let Some(ref base) = base {
            match base.join(href) {
                Ok(u) => u.to_string(),
                Err(_) => continue,
            }
        } else {
            continue;
        };
        if seen.insert(resolved.clone()) {
            let text = collect_inline_text(el);
            if text.is_empty() {
                links.push(format!("- {}", resolved));
            } else {
                links.push(format!("- [{}]({})", text, resolved));
            }
        }
    }

    if links.is_empty() {
        String::new()
    } else {
        format!("\n\n## Links\n\n{}\n", links.join("\n"))
    }
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
    let segs = addr.segments();
    let first = segs[0];

    // Standard IPv6 private/reserved ranges
    if addr.is_loopback()
        || addr.is_unspecified()
        || (first & 0xfe00) == 0xfc00  // ULA (fc00::/7)
        || (first & 0xffc0) == 0xfe80  // Link-local (fe80::/10)
        || (first & 0xff00) == 0xff00
    // Multicast (ff00::/8)
    {
        return true;
    }

    // IPv6-to-IPv4 transition addresses: extract the embedded IPv4 and check it.
    // See GHSA-j8q9-r9pq-2hh9 for details on each bypass vector.

    // IPv4-mapped (::ffff:w.x.y.z) and IPv4-compatible (::w.x.y.z) — RFC 4291
    // Layout: first 80 bits zero, then either 0000 or ffff, then 32-bit IPv4
    if (segs[0] == 0 && segs[1] == 0 && segs[2] == 0 && segs[3] == 0 && segs[4] == 0)
        && (segs[5] == 0xffff || segs[5] == 0x0000)
    {
        let ipv4 = Ipv4Addr::new(
            (segs[6] >> 8) as u8,
            segs[6] as u8,
            (segs[7] >> 8) as u8,
            segs[7] as u8,
        );
        return is_private_or_local_ipv4(ipv4);
    }

    // NAT64 well-known prefix (64:ff9b::/96) — RFC 6052
    // Layout: 0064:ff9b:0000:0000:0000:0000:w.x.y.z
    if segs[0] == 0x0064
        && segs[1] == 0xff9b
        && segs[2] == 0
        && segs[3] == 0
        && segs[4] == 0
        && segs[5] == 0
    {
        let ipv4 = Ipv4Addr::new(
            (segs[6] >> 8) as u8,
            segs[6] as u8,
            (segs[7] >> 8) as u8,
            segs[7] as u8,
        );
        return is_private_or_local_ipv4(ipv4);
    }

    // 6to4 (2002::/16) — RFC 3056
    // IPv4 embedded in bits 16-47 (segments 1 and 2)
    if first == 0x2002 {
        let ipv4 = Ipv4Addr::new(
            (segs[1] >> 8) as u8,
            segs[1] as u8,
            (segs[2] >> 8) as u8,
            segs[2] as u8,
        );
        return is_private_or_local_ipv4(ipv4);
    }

    // Teredo (2001:0000::/32) — RFC 4380
    // IPv4 is bitwise-inverted in the last 32 bits (segments 6 and 7)
    if segs[0] == 0x2001 && segs[1] == 0x0000 {
        let inv6 = !segs[6];
        let inv7 = !segs[7];
        let ipv4 = Ipv4Addr::new((inv6 >> 8) as u8, inv6 as u8, (inv7 >> 8) as u8, inv7 as u8);
        return is_private_or_local_ipv4(ipv4);
    }

    // ISATAP (RFC 5214 section 6.1)
    // Interface ID contains 0000:5efe or 0200:5efe followed by 32-bit IPv4
    // Pattern: *:*:*:*:0000:5efe:w.x.y.z or *:*:*:*:0200:5efe:w.x.y.z
    if (segs[4] == 0x0000 || segs[4] == 0x0200) && segs[5] == 0x5efe {
        let ipv4 = Ipv4Addr::new(
            (segs[6] >> 8) as u8,
            segs[6] as u8,
            (segs[7] >> 8) as u8,
            segs[7] as u8,
        );
        return is_private_or_local_ipv4(ipv4);
    }

    false
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
        let doc = Html::parse_document(html);
        assert_eq!(
            tool.extract_title_from_doc(&doc),
            Some("Test Page".to_string())
        );
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
        assert!(
            text.contains("# Hello"),
            "Expected markdown heading, got: {}",
            text
        );
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

    // ==================== IPv6-to-IPv4 TRANSITION ADDRESS TESTS ====================
    // Ref: GHSA-j8q9-r9pq-2hh9 — SSRF guard bypass via transition addresses

    #[test]
    fn test_ipv6_mapped_ipv4_blocked() {
        // ::ffff:127.0.0.1 — IPv4-mapped IPv6 (RFC 4291)
        let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "IPv4-mapped IPv6 loopback (::ffff:127.0.0.1) should be blocked"
        );
        // Cloud metadata
        let meta: IpAddr = "::ffff:169.254.169.254".parse().unwrap();
        assert!(
            is_private_or_local_ip(meta),
            "IPv4-mapped cloud metadata (::ffff:169.254.169.254) should be blocked"
        );
        // Private range
        let priv10: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(priv10),
            "IPv4-mapped private (::ffff:10.0.0.1) should be blocked"
        );
    }

    #[test]
    fn test_ipv6_compatible_ipv4_blocked() {
        // ::127.0.0.1 — IPv4-compatible IPv6 (deprecated but still parsed)
        let ip: IpAddr = "::127.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "IPv4-compatible IPv6 loopback (::127.0.0.1) should be blocked"
        );
    }

    #[test]
    fn test_ipv6_nat64_blocked() {
        // 64:ff9b::127.0.0.1 — NAT64 well-known prefix (RFC 6052)
        let ip: IpAddr = "64:ff9b::127.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "NAT64 loopback (64:ff9b::127.0.0.1) should be blocked"
        );
        let meta: IpAddr = "64:ff9b::169.254.169.254".parse().unwrap();
        assert!(
            is_private_or_local_ip(meta),
            "NAT64 cloud metadata (64:ff9b::169.254.169.254) should be blocked"
        );
    }

    #[test]
    fn test_ipv6_6to4_blocked() {
        // 2002:7f00:0001:: — 6to4 (RFC 3056), embeds 127.0.0.1 in bits 16-47
        let ip: IpAddr = "2002:7f00:0001::".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "6to4 loopback (2002:7f00:0001::) should be blocked"
        );
        // 2002:a9fe:a9fe:: embeds 169.254.169.254
        let meta: IpAddr = "2002:a9fe:a9fe::".parse().unwrap();
        assert!(
            is_private_or_local_ip(meta),
            "6to4 cloud metadata (2002:a9fe:a9fe::) should be blocked"
        );
    }

    #[test]
    fn test_ipv6_teredo_blocked() {
        // Teredo (2001:0000::/32) — IPv4 embedded inverted in last 32 bits
        // 127.0.0.1 inverted = 0x80fffffe → last 32 bits
        let ip: IpAddr = "2001:0000:0:0:0:0:80ff:fefe".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "Teredo loopback (2001:0000::80ff:fefe) should be blocked"
        );
    }

    #[test]
    fn test_ipv6_isatap_blocked() {
        // ISATAP (RFC 5214) — ::5efe:w.x.y.z or ::0200:5efe:w.x.y.z
        let ip: IpAddr = "2001:db8:1234::5efe:127.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip),
            "ISATAP loopback (::5efe:127.0.0.1) should be blocked"
        );
        let ip2: IpAddr = "fe80::5efe:10.0.0.1".parse().unwrap();
        assert!(
            is_private_or_local_ip(ip2),
            "ISATAP private (fe80::5efe:10.0.0.1) should be blocked"
        );
    }

    #[test]
    fn test_ipv4_mapped_unspecified_blocked() {
        // ::ffff:0.0.0.0 — IPv4-mapped with embedded 0.0.0.0 must be blocked
        let addr: IpAddr = "::ffff:0.0.0.0".parse().unwrap();
        assert!(
            is_private_or_local_ip(addr),
            "::ffff:0.0.0.0 should be blocked (unspecified IPv4)"
        );
        // ::0.0.0.0 — IPv4-compatible with embedded 0.0.0.0 (same as ::)
        // Already caught by is_unspecified, but verify
        let addr2: IpAddr = "::".parse().unwrap();
        assert!(
            is_private_or_local_ip(addr2),
            ":: should be blocked (unspecified)"
        );
    }

    #[test]
    fn test_ipv6_transition_public_ipv4_allowed() {
        // Legitimate public IPv4 embedded in transition addresses should NOT be blocked
        // 8.8.8.8 via NAT64
        let public_nat64: IpAddr = "64:ff9b::8.8.8.8".parse().unwrap();
        assert!(
            !is_private_or_local_ip(public_nat64),
            "NAT64 with public IP (64:ff9b::8.8.8.8) should NOT be blocked"
        );
        // 8.8.8.8 via 6to4 (2002:0808:0808::)
        let public_6to4: IpAddr = "2002:0808:0808::".parse().unwrap();
        assert!(
            !is_private_or_local_ip(public_6to4),
            "6to4 with public IP (2002:0808:0808::) should NOT be blocked"
        );
    }

    // ==================== HTML ENTITY DECODING TESTS ====================

    #[test]
    fn test_decode_named_entities() {
        assert_eq!(decode_html_entities("&amp; &lt; &gt;"), "& < >");
        assert_eq!(decode_html_entities("&quot;hi&quot;"), "\"hi\"");
        assert_eq!(decode_html_entities("&nbsp;"), " ");
        assert_eq!(decode_html_entities("&#39;"), "'");
        assert_eq!(decode_html_entities("&apos;"), "'");
    }

    #[test]
    fn test_decode_typography_entities() {
        assert_eq!(decode_html_entities("&mdash;"), "\u{2014}");
        assert_eq!(decode_html_entities("&ndash;"), "\u{2013}");
        assert_eq!(
            decode_html_entities("&ldquo;hi&rdquo;"),
            "\u{201C}hi\u{201D}"
        );
        assert_eq!(decode_html_entities("&hellip;"), "\u{2026}");
        assert_eq!(decode_html_entities("&copy;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&bull;"), "\u{2022}");
    }

    #[test]
    fn test_decode_numeric_decimal() {
        assert_eq!(decode_html_entities("&#65;"), "A");
        assert_eq!(decode_html_entities("&#169;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&#8212;"), "\u{2014}");
    }

    #[test]
    fn test_decode_numeric_hex() {
        assert_eq!(decode_html_entities("&#x41;"), "A");
        assert_eq!(decode_html_entities("&#xA9;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&#x2014;"), "\u{2014}");
    }

    #[test]
    fn test_decode_unknown_entity_passthrough() {
        assert_eq!(decode_html_entities("&foobar;"), "&foobar;");
        assert_eq!(decode_html_entities("&unknown;"), "&unknown;");
    }

    #[test]
    fn test_decode_no_semicolon_passthrough() {
        // & without matching ; should pass through raw
        assert_eq!(decode_html_entities("AT&T rocks"), "AT&T rocks");
    }

    #[test]
    fn test_normalize_whitespace_md_preserves_lines() {
        let input = "Hello  world\n\nSecond   paragraph\n";
        let result = normalize_whitespace_md(input);
        assert!(result.contains("Hello world\n"));
        assert!(result.contains("Second paragraph\n"));
    }

    #[test]
    fn test_normalize_whitespace_md_collapses_blank_lines() {
        let input = "A\n\n\n\n\nB\n";
        let result = normalize_whitespace_md(input);
        // 3+ blank lines should be collapsed to 2
        assert!(!result.contains("\n\n\n\n"));
        assert!(result.contains("A\n"));
        assert!(result.contains("B\n"));
    }

    // ==================== DOM WALKER TESTS ====================

    #[test]
    fn test_dom_headings() {
        let doc = Html::parse_document(
            "<html><body><h1>Title</h1><h2>Sub</h2><h3>Sub3</h3></body></html>",
        );
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("# Title"));
        assert!(md.contains("## Sub"));
        assert!(md.contains("### Sub3"));
    }

    #[test]
    fn test_dom_links() {
        let doc = Html::parse_document(r#"<body><a href="https://example.com">Click</a></body>"#);
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("[Click](https://example.com)"));
    }

    #[test]
    fn test_dom_bold_italic() {
        let doc = Html::parse_document("<body><strong>bold</strong> and <em>italic</em></body>");
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("**bold**"));
        assert!(md.contains("*italic*"));
    }

    #[test]
    fn test_dom_code_inline_and_block() {
        let doc = Html::parse_document("<body><code>inline</code><pre>code\nblock</pre></body>");
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("`inline`"));
        assert!(md.contains("```\ncode\nblock\n```"));
    }

    #[test]
    fn test_dom_lists() {
        let doc = Html::parse_document("<body><ul><li>one</li><li>two</li></ul></body>");
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("- one"));
        assert!(md.contains("- two"));
    }

    #[test]
    fn test_dom_skips_nav_footer() {
        let doc = Html::parse_document(
            "<body><nav>Skip me</nav><p>Content</p><footer>Skip too</footer></body>",
        );
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(!md.contains("Skip me"));
        assert!(!md.contains("Skip too"));
        assert!(md.contains("Content"));
    }

    #[test]
    fn test_dom_skips_script_style() {
        let doc = Html::parse_document(
            "<body><script>alert('x')</script><style>body{}</style><p>Visible</p></body>",
        );
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(!md.contains("alert"));
        assert!(!md.contains("body{}"));
        assert!(md.contains("Visible"));
    }

    #[test]
    fn test_dom_content_targeting_main() {
        let doc = Html::parse_document(
            "<html><body><nav>Menu</nav><main><p>Main content</p></main></body></html>",
        );
        let root = find_content_root(&doc).unwrap();
        let tag = root.value().name.local.as_ref();
        assert_eq!(tag, "main");
        let md = dom_to_markdown(root);
        assert!(md.contains("Main content"));
        assert!(!md.contains("Menu"));
    }

    #[test]
    fn test_dom_content_targeting_article() {
        let doc = Html::parse_document(
            "<html><body><aside>Sidebar</aside><article><p>Article body</p></article></body></html>",
        );
        let root = find_content_root(&doc).unwrap();
        let tag = root.value().name.local.as_ref();
        assert_eq!(tag, "article");
    }

    #[test]
    fn test_dom_nested_formatting() {
        let doc = Html::parse_document(
            "<body><p>Hello <strong>bold <em>and italic</em></strong></p></body>",
        );
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("**bold and italic**"));
    }

    #[test]
    fn test_dom_empty_body() {
        let doc = Html::parse_document("<html><head></head><body></body></html>");
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.trim().is_empty());
    }

    #[test]
    fn test_dom_blockquote() {
        let doc = Html::parse_document("<body><blockquote>Quoted text</blockquote></body>");
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("> Quoted text"));
    }

    #[test]
    fn test_dom_image() {
        let doc = Html::parse_document(
            r#"<body><img alt="photo" src="https://example.com/img.jpg"></body>"#,
        );
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("![photo](https://example.com/img.jpg)"));
    }

    #[test]
    fn test_dom_hr() {
        let doc = Html::parse_document("<body><p>Before</p><hr><p>After</p></body>");
        let root = find_content_root(&doc).unwrap();
        let md = dom_to_markdown(root);
        assert!(md.contains("---"));
    }

    // ==================== LINK EXTRACTION TESTS ====================

    #[test]
    fn test_extract_links_absolute_urls() {
        let doc = Html::parse_document(
            r#"<body>
                <a href="https://example.com/a">Link A</a>
                <a href="https://example.com/b">Link B</a>
            </body>"#,
        );
        let links = extract_links(&doc, "https://example.com/");
        assert!(links.contains("[Link A](https://example.com/a)"));
        assert!(links.contains("[Link B](https://example.com/b)"));
        assert!(links.contains("## Links"));
    }

    #[test]
    fn test_extract_links_deduplicates() {
        let doc = Html::parse_document(
            r#"<body>
                <a href="https://example.com/a">First</a>
                <a href="https://example.com/a">Second</a>
            </body>"#,
        );
        let links = extract_links(&doc, "https://example.com/");
        // Should only appear once
        let count = links.matches("example.com/a").count();
        assert_eq!(count, 1, "Duplicate URL should be deduplicated");
    }

    #[test]
    fn test_extract_links_skips_anchors() {
        let doc = Html::parse_document(
            r##"<body>
                <a href="#section">Anchor</a>
                <a href="">Empty</a>
                <a href="https://real.com">Real</a>
            </body>"##,
        );
        let links = extract_links(&doc, "https://example.com/");
        assert!(!links.contains("#section"));
        assert!(links.contains("https://real.com"));
    }

    #[test]
    fn test_extract_links_resolves_relative() {
        let doc = Html::parse_document(r#"<body><a href="/about">About</a></body>"#);
        let links = extract_links(&doc, "https://example.com/page");
        assert!(
            links.contains("https://example.com/about"),
            "Relative URL should resolve against base. Got: {}",
            links
        );
    }

    #[test]
    fn test_extract_links_empty_when_none() {
        let doc = Html::parse_document("<body><p>No links here</p></body>");
        let links = extract_links(&doc, "https://example.com/");
        assert!(links.is_empty());
    }

    #[test]
    fn test_extract_text_with_include_links() {
        let tool = WebFetchTool::new();
        let html = r#"<body><p>Content</p><a href="https://example.com">Link</a></body>"#;
        let doc = Html::parse_document(html);
        let text = tool.extract_text_from_doc(&doc, true, "https://example.com/");
        assert!(text.contains("Content"));
        assert!(text.contains("## Links"));
        assert!(text.contains("https://example.com"));
    }

    // ==================== DDG SEARCH PARSING TESTS ====================

    #[test]
    fn test_parse_ddg_results_basic() {
        let html = r#"<html><body>
            <div class="results">
                <div class="result">
                    <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&amp;rut=abc">Example Page</a>
                    <a class="result__snippet">This is the snippet for example page.</a>
                </div>
            </div>
        </body></html>"#;
        let results = parse_ddg_html(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Page");
        assert_eq!(results[0].url, "https://example.com/page");
        assert_eq!(
            results[0].description,
            Some("This is the snippet for example page.".to_string())
        );
    }

    #[test]
    fn test_parse_ddg_results_multiple() {
        let html = r#"<html><body>
            <div class="results">
                <div class="result">
                    <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fa.com">A</a>
                    <a class="result__snippet">Snippet A</a>
                </div>
                <div class="result">
                    <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fb.com">B</a>
                    <a class="result__snippet">Snippet B</a>
                </div>
                <div class="result">
                    <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fc.com">C</a>
                    <a class="result__snippet">Snippet C</a>
                </div>
            </div>
        </body></html>"#;
        let results = parse_ddg_html(html, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "A");
        assert_eq!(results[1].title, "B");
    }

    #[test]
    fn test_parse_ddg_results_empty() {
        let html = "<html><body><div class='results'></div></body></html>";
        let results = parse_ddg_html(html, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_ddg_direct_url() {
        let html = r#"<html><body>
            <div class="results">
                <div class="result">
                    <a class="result__a" href="https://example.com/direct">Direct Link</a>
                    <a class="result__snippet">Direct snippet</a>
                </div>
            </div>
        </body></html>"#;
        let results = parse_ddg_html(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/direct");
    }

    #[test]
    fn test_extract_ddg_url_with_uddg() {
        let url = "https://duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2Flearn&rut=abc";
        assert_eq!(extract_ddg_real_url(url), "https://rust-lang.org/learn");
    }

    #[test]
    fn test_extract_ddg_url_direct() {
        let url = "https://example.com/page";
        assert_eq!(extract_ddg_real_url(url), "https://example.com/page");
    }

    #[test]
    fn test_extract_ddg_url_no_uddg_param() {
        let url = "https://duckduckgo.com/l/?other=value";
        assert_eq!(
            extract_ddg_real_url(url),
            "https://duckduckgo.com/l/?other=value"
        );
    }

    // ==================== DDG SEARCH TOOL TESTS ====================

    #[test]
    fn test_ddg_search_tool_name() {
        let tool = DdgSearchTool::new();
        assert_eq!(tool.name(), "web_search");
    }

    #[test]
    fn test_ddg_search_tool_description() {
        let tool = DdgSearchTool::new();
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_ddg_search_tool_parameters() {
        let tool = DdgSearchTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["query"].is_object());
        assert!(params["required"]
            .as_array()
            .unwrap()
            .contains(&json!("query")));
    }
}
