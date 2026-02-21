//! ClawHub skill registry client with in-memory search cache.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

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

/// HTTP client for the ClawHub REST API.
pub struct ClawHubRegistry {
    base_url: String,
    auth_token: Option<String>,
    client: reqwest::Client,
    cache: Arc<SearchCache>,
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
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            cache,
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
        let mut req = self.client.get(&url);
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

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
        let mut req = self.client.get(&url);
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::ZeptoError::Tool(e.to_string()))?;

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
}
