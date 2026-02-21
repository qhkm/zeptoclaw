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

    /// Return cached results for `query` if present and not expired.
    pub fn get(&self, query: &str) -> Option<Vec<SkillSearchResult>> {
        let entries = self.entries.read().unwrap();
        entries.get(query).and_then(|e| {
            if e.inserted_at.elapsed() < self.ttl {
                Some(e.results.clone())
            } else {
                None
            }
        })
    }

    /// Store results for `query`.  Evicts the oldest entry when full.
    pub fn set(&self, query: &str, results: Vec<SkillSearchResult>) {
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
            query.to_string(),
            CacheEntry {
                results,
                inserted_at: Instant::now(),
            },
        );
    }
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
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> crate::error::Result<Vec<SkillSearchResult>> {
        if let Some(cached) = self.cache.get(query) {
            return Ok(cached);
        }

        let url = format!(
            "{}/api/v1/search?q={}&limit={}",
            self.base_url, query, limit
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

        self.cache.set(query, results.clone());
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
        cache.set("test query", results.clone());
        let hit = cache.get("test query").unwrap();
        assert_eq!(hit[0].slug, "test");
    }

    #[test]
    fn test_search_cache_ttl_expire() {
        let cache = SearchCache::new(10, Duration::from_millis(1));
        cache.set("q", vec![]);
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get("q").is_none());
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
        cache.set("query1", r1);
        cache.set("query2", r2);
        assert_eq!(cache.get("query1").unwrap()[0].slug, "a");
        assert_eq!(cache.get("query2").unwrap()[0].slug, "b");
    }

    #[test]
    fn test_search_cache_overwrite_same_key() {
        let cache = SearchCache::new(10, Duration::from_secs(60));
        cache.set("q", vec![]);
        let results = vec![SkillSearchResult {
            slug: "new".into(),
            display_name: "New".into(),
            summary: "updated".into(),
            version: "2.0".into(),
            is_suspicious: false,
        }];
        cache.set("q", results);
        assert_eq!(cache.get("q").unwrap()[0].slug, "new");
    }
}
