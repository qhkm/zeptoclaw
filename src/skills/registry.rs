//! ClawHub skill registry client with in-memory search cache.
//!
//! All outbound HTTP requests are guarded against SSRF attacks by reusing the
//! same `is_blocked_host` / `resolve_and_check_host` checks that protect
//! `web_fetch`. Self-hosted registries on private networks can opt in via
//! `ClawHubConfig::allowed_hosts`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::tools::web::{is_blocked_host, resolve_and_check_host};

/// A single skill entry returned from a ClawHub search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    /// Unique identifier for this skill (used when installing).
    pub slug: String,
    /// Human-readable skill name.
    pub display_name: String,
    /// Short description of what the skill does.
    pub summary: String,
    /// Published version string (e.g. "1.0.0").
    pub version: String,
    /// Set to `true` when the registry flags this skill as suspicious.
    #[serde(default)]
    pub is_suspicious: bool,
}

struct CacheEntry {
    results: Vec<SkillSearchResult>,
    inserted_at: Instant,
}

/// In-memory TTL search cache.
///
/// Evicts the oldest entry when `max_size` is reached.  Entries older than
/// `ttl` are treated as misses even if they are still present in the map.
pub struct SearchCache {
    entries: Arc<RwLock<HashMap<String, CacheEntry>>>,
    max_size: usize,
    ttl: Duration,
}

impl SearchCache {
    /// Create a new cache with the given capacity and entry TTL.
    pub fn new(max_size: usize, ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            max_size,
            ttl,
        }
    }

    /// Return cached results for `key` if present and not expired.
    pub fn get(&self, key: &str) -> Option<Vec<SkillSearchResult>> {
        let entries = self.entries.read().unwrap();
        entries.get(key).and_then(|e| {
            if e.inserted_at.elapsed() < self.ttl {
                Some(e.results.clone())
            } else {
                None
            }
        })
    }

    /// Store results for `key`.  Evicts the oldest entry when full.
    ///
    /// When `max_size` is 0 the cache is disabled and this is a no-op.
    pub fn set(&self, key: &str, results: Vec<SkillSearchResult>) {
        if self.max_size == 0 {
            return; // cache disabled
        }
        let mut entries = self.entries.write().unwrap();
        if entries.len() >= self.max_size {
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }
        entries.insert(
            key.to_string(),
            CacheEntry {
                results,
                inserted_at: Instant::now(),
            },
        );
    }
}

/// Percent-encode a string using RFC 3986 unreserved characters.
///
/// Characters in `[A-Za-z0-9\-_.~]` are passed through unchanged; every other
/// byte is encoded as `%XX` (uppercase hex).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// Validate that `slug` contains only safe characters for use in URLs and
/// filesystem paths.
///
/// Allowed: ASCII alphanumeric characters, hyphens (`-`), and underscores (`_`).
fn validate_slug(slug: &str) -> crate::error::Result<()> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(crate::error::ZeptoError::Tool(format!(
            "Invalid skill slug '{}': only alphanumeric characters, hyphens, and underscores are allowed",
            slug
        )));
    }
    Ok(())
}

/// Check whether `host` is in the allowed-hosts bypass list (case-insensitive).
fn is_allowed_host(host: &str, allowed_hosts: &[String]) -> bool {
    let host_lower = host.to_ascii_lowercase();
    allowed_hosts
        .iter()
        .any(|ah| ah.to_ascii_lowercase() == host_lower)
}

/// Validate that a URL does not target a private/local/link-local address.
///
/// Returns the pinned `(hostname, SocketAddr)` from DNS resolution so the
/// caller can build a reqwest client with `.resolve()`, eliminating the DNS
/// rebinding window between this check and the actual HTTP request.
///
/// If the URL's host is in `allowed_hosts`, the check is skipped (opt-in
/// override for self-hosted registries on private networks).
async fn check_ssrf(
    url_str: &str,
    allowed_hosts: &[String],
) -> crate::error::Result<Option<(String, std::net::SocketAddr)>> {
    let parsed = Url::parse(url_str).map_err(|e| {
        crate::error::ZeptoError::SecurityViolation(format!("Invalid URL '{}': {}", url_str, e))
    })?;

    // Only allow http/https schemes.
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(crate::error::ZeptoError::SecurityViolation(
                "Only http/https URLs are allowed for skill downloads".to_string(),
            ));
        }
    }

    // If the host is explicitly allowed, skip SSRF checks.
    if let Some(host) = parsed.host_str() {
        if is_allowed_host(host, allowed_hosts) {
            return Ok(None);
        }
    }

    // Hostname / IP blocklist (localhost, private ranges, link-local, etc.)
    if is_blocked_host(&parsed) {
        return Err(crate::error::ZeptoError::SecurityViolation(format!(
            "Skill URL targets a blocked host (local or private network): {}",
            url_str
        )));
    }

    // DNS resolution check — catches hostnames that resolve to private IPs
    // (e.g., `metadata.attacker.com` → 169.254.169.254).
    // Returns the first safe resolved address for connection pinning.
    let pinned = resolve_and_check_host(&parsed).await?;

    Ok(pinned)
}

/// Validate that a response's final URL (after redirects) is not a blocked host.
///
/// Reuses the same `is_blocked_host` check and `allowed_hosts` bypass as
/// `check_ssrf`, ensuring consistent SSRF protection across pre-request
/// and post-redirect validation.
fn check_redirect_ssrf(final_url: &Url, allowed_hosts: &[String]) -> crate::error::Result<()> {
    if is_blocked_host(final_url) {
        if let Some(host) = final_url.host_str() {
            if is_allowed_host(host, allowed_hosts) {
                return Ok(());
            }
        }
        return Err(crate::error::ZeptoError::SecurityViolation(format!(
            "Request redirected to blocked host: {}",
            final_url
        )));
    }
    Ok(())
}

/// Build a reqwest client that pins DNS resolution to a pre-validated IP,
/// preventing DNS rebinding attacks between the SSRF check and the actual
/// HTTP connection.
fn build_pinned_client(
    pinned: Option<(String, std::net::SocketAddr)>,
) -> crate::error::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5));
    if let Some((host, addr)) = pinned {
        builder = builder.resolve(&host, addr);
    }
    builder
        .build()
        .map_err(|e| crate::error::ZeptoError::Tool(format!("HTTP client error: {}", e)))
}

/// HTTP client for the ClawHub REST API.
pub struct ClawHubRegistry {
    base_url: String,
    auth_token: Option<String>,
    cache: Arc<SearchCache>,
    /// Hostnames allowed to bypass SSRF checks (for self-hosted registries).
    allowed_hosts: Vec<String>,
}

impl ClawHubRegistry {
    /// Create a new registry client.
    pub fn new(
        base_url: impl Into<String>,
        auth_token: Option<String>,
        cache: Arc<SearchCache>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token,
            cache,
            allowed_hosts: Vec::new(),
        }
    }

    /// Create a new registry client with an explicit SSRF-bypass allowlist.
    ///
    /// Hosts in `allowed_hosts` are exempt from the private/local IP check,
    /// enabling self-hosted registries on internal networks.
    pub fn with_allowed_hosts(
        base_url: impl Into<String>,
        auth_token: Option<String>,
        cache: Arc<SearchCache>,
        allowed_hosts: Vec<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token,
            cache,
            allowed_hosts,
        }
    }

    /// Search for skills matching `query`, returning at most `limit` results.
    ///
    /// Results are returned from the in-memory cache when available.
    /// The cache key includes both the query and limit to prevent stale
    /// truncated results from being served for a different limit value.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> crate::error::Result<Vec<SkillSearchResult>> {
        let cache_key = format!("{}:{}", query, limit);
        if let Some(cached) = self.cache.get(&cache_key) {
            return Ok(cached);
        }

        let url = format!(
            "{}/api/v1/search?q={}&limit={}",
            self.base_url,
            percent_encode(query),
            limit
        );

        // SSRF guard: validate the URL and pin DNS resolution to the
        // validated IP, preventing rebinding between check and request.
        let pinned = check_ssrf(&url, &self.allowed_hosts).await?;
        let client = build_pinned_client(pinned)?;

        let mut req = client.get(&url);
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

        // Post-redirect SSRF check: block redirects to private hosts.
        check_redirect_ssrf(resp.url(), &self.allowed_hosts)?;

        if !resp.status().is_success() {
            return Err(crate::error::ZeptoError::Tool(format!(
                "ClawHub search failed: {}",
                resp.status()
            )));
        }

        let results: Vec<SkillSearchResult> = resp
            .json()
            .await
            .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

        self.cache.set(&cache_key, results.clone());
        Ok(results)
    }

    /// Download a skill archive from ClawHub and extract it into `skills_dir`.
    ///
    /// Returns the path to the installed skill directory on success.
    pub async fn download_and_install(
        &self,
        slug: &str,
        skills_dir: &str,
    ) -> crate::error::Result<String> {
        // Validate slug before using it in a URL or filesystem path.
        validate_slug(slug)?;

        let url = format!("{}/api/v1/download/{}", self.base_url, slug);

        // SSRF guard: validate the URL and pin DNS resolution to the
        // validated IP, preventing rebinding between check and request.
        let pinned = check_ssrf(&url, &self.allowed_hosts).await?;
        let client = build_pinned_client(pinned)?;

        let mut req = client.get(&url);
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

        // Post-redirect SSRF check: block redirects to private hosts.
        check_redirect_ssrf(resp.url(), &self.allowed_hosts)?;

        if !resp.status().is_success() {
            return Err(crate::error::ZeptoError::Tool(format!(
                "ClawHub download failed: {}",
                resp.status()
            )));
        }

        // Reject archives that are larger than 50 MB before buffering.
        if let Some(content_length) = resp.content_length() {
            if content_length > 50 * 1024 * 1024 {
                return Err(crate::error::ZeptoError::Tool(format!(
                    "Skill archive too large ({} bytes, max 50MB)",
                    content_length
                )));
            }
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

        let target_dir = format!("{}/{}", skills_dir, slug);
        tokio::fs::create_dir_all(&target_dir)
            .await
            .map_err(crate::error::ZeptoError::Io)?;

        // Extract the zip archive synchronously inside spawn_blocking to avoid
        // holding non-Send ZipFile across await points.
        let bytes_vec = bytes.to_vec();
        let target_dir_clone = target_dir.clone();
        tokio::task::spawn_blocking(move || {
            let cursor = std::io::Cursor::new(bytes_vec);
            let mut archive = zip::ZipArchive::new(cursor)
                .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

            for i in 0..archive.len() {
                let mut file = archive
                    .by_index(i)
                    .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

                // Sanitise the path: strip leading '/' and reject '..'
                let safe_name = file.name().to_string();
                let safe_name = safe_name.trim_start_matches('/');
                if safe_name.contains("..") {
                    return Err(crate::error::ZeptoError::Tool(format!(
                        "Skill zip contains path traversal: {}",
                        safe_name
                    )));
                }

                let out_path = format!("{}/{}", target_dir_clone, safe_name);

                if file.is_dir() {
                    std::fs::create_dir_all(&out_path).map_err(crate::error::ZeptoError::Io)?;
                } else {
                    // Ensure parent directory exists
                    if let Some(parent) = std::path::Path::new(&out_path).parent() {
                        std::fs::create_dir_all(parent).map_err(crate::error::ZeptoError::Io)?;
                    }
                    let mut out =
                        std::fs::File::create(&out_path).map_err(crate::error::ZeptoError::Io)?;
                    std::io::copy(&mut file, &mut out).map_err(crate::error::ZeptoError::Io)?;
                }
            }
            Ok(target_dir_clone)
        })
        .await
        .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_cache_miss() {
        let cache = SearchCache::new(10, Duration::from_secs(60));
        assert!(cache.get("anything").is_none());
    }

    #[test]
    fn test_search_cache_hit() {
        let cache = SearchCache::new(10, Duration::from_secs(60));
        let results = vec![SkillSearchResult {
            slug: "test".into(),
            display_name: "Test".into(),
            summary: "A test skill".into(),
            version: "1.0.0".into(),
            is_suspicious: false,
        }];
        cache.set("test query:10", results.clone());
        let hit = cache.get("test query:10").unwrap();
        assert_eq!(hit[0].slug, "test");
    }

    #[test]
    fn test_search_cache_ttl_expire() {
        let cache = SearchCache::new(10, Duration::from_millis(1));
        cache.set("q:10", vec![]);
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get("q:10").is_none());
    }

    #[test]
    fn test_search_cache_evicts_when_full() {
        let cache = SearchCache::new(2, Duration::from_secs(60));
        cache.set("a", vec![]);
        cache.set("b", vec![]);
        cache.set("c", vec![]);
        let count = [
            cache.get("a").is_some(),
            cache.get("b").is_some(),
            cache.get("c").is_some(),
        ]
        .iter()
        .filter(|&&v| v)
        .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_skill_search_result_is_suspicious_defaults_false() {
        let json = r#"{"slug":"x","display_name":"X","summary":"s","version":"1.0"}"#;
        let r: SkillSearchResult = serde_json::from_str(json).unwrap();
        assert!(!r.is_suspicious);
    }

    #[test]
    fn test_search_cache_different_queries_stored_independently() {
        let cache = SearchCache::new(10, Duration::from_secs(60));
        let r1 = vec![SkillSearchResult {
            slug: "a".into(),
            display_name: "A".into(),
            summary: "".into(),
            version: "1.0".into(),
            is_suspicious: false,
        }];
        let r2 = vec![SkillSearchResult {
            slug: "b".into(),
            display_name: "B".into(),
            summary: "".into(),
            version: "2.0".into(),
            is_suspicious: false,
        }];
        cache.set("query1:10", r1);
        cache.set("query2:10", r2);
        assert_eq!(cache.get("query1:10").unwrap()[0].slug, "a");
        assert_eq!(cache.get("query2:10").unwrap()[0].slug, "b");
    }

    #[test]
    fn test_search_cache_overwrite_same_key() {
        let cache = SearchCache::new(10, Duration::from_secs(60));
        cache.set("q:10", vec![]);
        let results = vec![SkillSearchResult {
            slug: "new".into(),
            display_name: "New".into(),
            summary: "updated".into(),
            version: "2.0".into(),
            is_suspicious: false,
        }];
        cache.set("q:10", results);
        assert_eq!(cache.get("q:10").unwrap()[0].slug, "new");
    }

    // -------------------------------------------------------------------------
    // Fix 4: max_size == 0 disables the cache
    // -------------------------------------------------------------------------

    #[test]
    fn test_search_cache_max_size_zero_is_noop() {
        let cache = SearchCache::new(0, Duration::from_secs(60));
        cache.set("key", vec![]);
        // Nothing should have been stored.
        assert!(cache.get("key").is_none());
    }

    // -------------------------------------------------------------------------
    // Fix 1: percent_encode
    // -------------------------------------------------------------------------

    #[test]
    fn test_percent_encode_unreserved_passthrough() {
        assert_eq!(percent_encode("hello"), "hello");
        assert_eq!(percent_encode("test-value_123.txt~"), "test-value_123.txt~");
    }

    #[test]
    fn test_percent_encode_spaces_and_specials() {
        assert_eq!(percent_encode("hello world"), "hello%20world");
        assert_eq!(percent_encode("a=b&c=d"), "a%3Db%26c%3Dd");
        assert_eq!(percent_encode("web scraper"), "web%20scraper");
    }

    #[test]
    fn test_percent_encode_empty() {
        assert_eq!(percent_encode(""), "");
    }

    // -------------------------------------------------------------------------
    // Fix 2: validate_slug
    // -------------------------------------------------------------------------

    #[test]
    fn test_validate_slug_valid() {
        assert!(validate_slug("web-scraper").is_ok());
        assert!(validate_slug("my_skill").is_ok());
        assert!(validate_slug("skill123").is_ok());
        assert!(validate_slug("ABC").is_ok());
    }

    #[test]
    fn test_validate_slug_empty_is_error() {
        assert!(validate_slug("").is_err());
    }

    #[test]
    fn test_validate_slug_path_traversal_is_error() {
        assert!(validate_slug("../etc/passwd").is_err());
        assert!(validate_slug("../../secret").is_err());
    }

    #[test]
    fn test_validate_slug_slash_is_error() {
        assert!(validate_slug("foo/bar").is_err());
    }

    #[test]
    fn test_validate_slug_space_is_error() {
        assert!(validate_slug("web scraper").is_err());
    }

    #[test]
    fn test_validate_slug_special_chars_are_error() {
        assert!(validate_slug("skill;rm -rf").is_err());
        assert!(validate_slug("skill<script>").is_err());
        assert!(validate_slug("skill%20encoded").is_err());
    }

    // -------------------------------------------------------------------------
    // is_allowed_host helper
    // -------------------------------------------------------------------------

    #[test]
    fn test_is_allowed_host_match() {
        let hosts = vec!["registry.internal.corp".to_string()];
        assert!(is_allowed_host("registry.internal.corp", &hosts));
    }

    #[test]
    fn test_is_allowed_host_case_insensitive() {
        let hosts = vec!["Registry.Internal.Corp".to_string()];
        assert!(is_allowed_host("registry.internal.corp", &hosts));
        assert!(is_allowed_host("REGISTRY.INTERNAL.CORP", &hosts));
    }

    #[test]
    fn test_is_allowed_host_no_match() {
        let hosts = vec!["other.corp".to_string()];
        assert!(!is_allowed_host("registry.internal.corp", &hosts));
    }

    #[test]
    fn test_is_allowed_host_empty_list() {
        assert!(!is_allowed_host("anything", &[]));
    }

    // -------------------------------------------------------------------------
    // check_redirect_ssrf helper
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_redirect_ssrf_blocks_private() {
        let url = Url::parse("http://192.168.1.1/secret").unwrap();
        assert!(check_redirect_ssrf(&url, &[]).is_err());
    }

    #[test]
    fn test_check_redirect_ssrf_allows_public() {
        let url = Url::parse("https://clawhub.ai/download").unwrap();
        assert!(check_redirect_ssrf(&url, &[]).is_ok());
    }

    #[test]
    fn test_check_redirect_ssrf_allows_listed_private() {
        let url = Url::parse("http://10.0.0.5/download").unwrap();
        let allowed = vec!["10.0.0.5".to_string()];
        assert!(check_redirect_ssrf(&url, &allowed).is_ok());
    }

    #[test]
    fn test_check_redirect_ssrf_blocks_unlisted_private() {
        let url = Url::parse("http://10.0.0.99/download").unwrap();
        let allowed = vec!["10.0.0.5".to_string()];
        assert!(check_redirect_ssrf(&url, &allowed).is_err());
    }

    // -------------------------------------------------------------------------
    // build_pinned_client
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_pinned_client_none() {
        let client = build_pinned_client(None);
        assert!(client.is_ok(), "Should build client without pinning");
    }

    #[test]
    fn test_build_pinned_client_with_addr() {
        let addr: std::net::SocketAddr = "93.184.216.34:443".parse().unwrap();
        let client = build_pinned_client(Some(("example.com".to_string(), addr)));
        assert!(client.is_ok(), "Should build client with pinned address");
    }

    // -------------------------------------------------------------------------
    // SSRF guardrails (check_ssrf)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_check_ssrf_blocks_localhost() {
        let result = check_ssrf("http://localhost:8080/api/v1/download/test", &[]).await;
        assert!(result.is_err(), "localhost should be blocked");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("blocked") || err.contains("private") || err.contains("local"),
            "Error should mention blocking: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_check_ssrf_blocks_private_ipv4() {
        let result = check_ssrf("http://192.168.1.1/api/v1/download/test", &[]).await;
        assert!(result.is_err(), "192.168.x.x should be blocked");

        let result = check_ssrf("http://10.0.0.5/api/v1/download/test", &[]).await;
        assert!(result.is_err(), "10.x.x.x should be blocked");

        let result = check_ssrf("http://172.16.0.1/api/v1/download/test", &[]).await;
        assert!(result.is_err(), "172.16.x.x should be blocked");
    }

    #[tokio::test]
    async fn test_check_ssrf_blocks_loopback() {
        let result = check_ssrf("http://127.0.0.1:9090/download", &[]).await;
        assert!(result.is_err(), "127.0.0.1 should be blocked");
    }

    #[tokio::test]
    async fn test_check_ssrf_blocks_link_local_metadata() {
        // AWS/GCP/Azure metadata endpoint
        let result = check_ssrf("http://169.254.169.254/latest/meta-data/", &[]).await;
        assert!(result.is_err(), "Cloud metadata endpoint should be blocked");
    }

    #[tokio::test]
    async fn test_check_ssrf_allows_public_url() {
        // Public URLs should pass SSRF checks and return a pinned address
        let result = check_ssrf("https://clawhub.ai/api/v1/download/web-scraper", &[]).await;
        assert!(result.is_ok(), "Public URL should be allowed");
        // The result should contain a pinned address for DNS rebinding prevention
        let pinned = result.unwrap();
        assert!(
            pinned.is_some(),
            "Public hostname should return a pinned address"
        );
    }

    #[tokio::test]
    async fn test_check_ssrf_allowed_hosts_bypass() {
        // A private IP that is explicitly allowed should pass
        let allowed = vec!["192.168.1.100".to_string()];
        let result = check_ssrf("http://192.168.1.100/api/v1/download/test", &allowed).await;
        assert!(
            result.is_ok(),
            "Explicitly allowed host should bypass SSRF checks"
        );
        // Allowed hosts skip DNS check, so pinned should be None
        let pinned = result.unwrap();
        assert!(
            pinned.is_none(),
            "Allowed host bypass should return None (no pinning needed)"
        );
    }

    #[tokio::test]
    async fn test_check_ssrf_allowed_hosts_case_insensitive() {
        let allowed = vec!["Registry.Internal.Corp".to_string()];
        let result = check_ssrf(
            "https://registry.internal.corp/api/v1/download/test",
            &allowed,
        )
        .await;
        assert!(
            result.is_ok(),
            "Allowed host matching should be case-insensitive"
        );
    }

    #[tokio::test]
    async fn test_check_ssrf_rejects_ftp_scheme() {
        let result = check_ssrf("ftp://example.com/skill.tar.gz", &[]).await;
        assert!(result.is_err(), "ftp:// should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("http/https"),
            "Error should mention scheme restriction: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_check_ssrf_rejects_file_scheme() {
        let result = check_ssrf("file:///etc/passwd", &[]).await;
        assert!(result.is_err(), "file:// should be rejected");
    }

    #[tokio::test]
    async fn test_check_ssrf_blocks_ipv6_loopback() {
        let result = check_ssrf("http://[::1]:8080/api/v1/download/test", &[]).await;
        assert!(result.is_err(), "IPv6 loopback should be blocked");
    }

    #[tokio::test]
    async fn test_check_ssrf_blocks_unspecified() {
        let result = check_ssrf("http://0.0.0.0/api/v1/download/test", &[]).await;
        assert!(result.is_err(), "0.0.0.0 should be blocked");
    }

    // -------------------------------------------------------------------------
    // ClawHubRegistry with allowed_hosts
    // -------------------------------------------------------------------------

    #[test]
    fn test_registry_new_has_empty_allowed_hosts() {
        let cache = Arc::new(SearchCache::new(10, Duration::from_secs(60)));
        let registry = ClawHubRegistry::new("https://clawhub.ai", None, cache);
        assert!(registry.allowed_hosts.is_empty());
    }

    #[test]
    fn test_registry_with_allowed_hosts_stores_list() {
        let cache = Arc::new(SearchCache::new(10, Duration::from_secs(60)));
        let hosts = vec!["internal.corp".to_string(), "10.0.0.5".to_string()];
        let registry =
            ClawHubRegistry::with_allowed_hosts("http://10.0.0.5", None, cache, hosts.clone());
        assert_eq!(registry.allowed_hosts, hosts);
    }
}
