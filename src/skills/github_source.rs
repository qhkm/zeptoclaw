//! GitHub-based skill discovery.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::Result;

const GITHUB_API_BASE: &str = "https://api.github.com/search/repositories";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    ClawHub,
    GitHub,
}

#[derive(Debug, Clone)]
pub struct SkillSearchResult {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub source: SkillSource,
    pub score: f64,
    pub stars: u64,
    pub url: String,
}

impl SkillSearchResult {
    pub fn from_github(repo: GitHubRepo, score: f64) -> Self {
        Self {
            name: repo.name,
            slug: repo.full_name.clone(),
            description: repo.description.unwrap_or_default(),
            source: SkillSource::GitHub,
            score,
            stars: repo.stargazers_count,
            url: repo.html_url,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubSearchResponse {
    pub total_count: u64,
    pub items: Vec<GitHubRepo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRepo {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub html_url: String,
    pub stargazers_count: u64,
    pub license: Option<License>,
    pub updated_at: String,
    #[serde(default)]
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct License {
    pub spdx_id: Option<String>,
}

/// Build a GitHub repository search URL for the given query and topic filters.
pub fn build_search_url(query: &str, topics: &[&str]) -> String {
    let encoded_query = query.replace(' ', "+");
    let topic_filters: Vec<String> = topics.iter().map(|t| format!("+topic:{}", t)).collect();
    format!(
        "{}?q={}{}&sort=stars&order=desc&per_page=20",
        GITHUB_API_BASE,
        encoded_query,
        topic_filters.join("")
    )
}

/// Compute a quality score [0.0, 1.0] for a GitHub repository as a skill source.
pub fn compute_quality_score(repo: &GitHubRepo, has_skill_md: bool) -> f64 {
    let mut score = 0.0;

    // Stars: log scale, max ~0.3
    if repo.stargazers_count > 0 {
        score += (repo.stargazers_count as f64).log10() * 0.15;
        score = score.min(0.3);
    }

    // License: +0.2
    if repo
        .license
        .as_ref()
        .and_then(|l| l.spdx_id.as_ref())
        .is_some()
    {
        score += 0.2;
    }

    // SKILL.md present: +0.3
    if has_skill_md {
        score += 0.3;
    }

    // Description quality: +0.1 if > 20 chars
    if repo.description.as_ref().is_some_and(|d| d.len() > 20) {
        score += 0.1;
    }

    // Recency: +0.1 if updated within 90 days
    if let Ok(updated) = chrono::DateTime::parse_from_rfc3339(&repo.updated_at) {
        let age_days = (chrono::Utc::now() - updated.with_timezone(&chrono::Utc)).num_days();
        if age_days < 90 {
            score += 0.1;
        }
    }

    score.min(1.0)
}

/// Simple in-memory cache with TTL.
pub struct SearchCache {
    entries: Mutex<HashMap<String, (Vec<SkillSearchResult>, Instant)>>,
    ttl: Duration,
}

impl SearchCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn get(&self, query: &str) -> Option<Vec<SkillSearchResult>> {
        let entries = self.entries.lock().unwrap();
        entries.get(query).and_then(|(results, cached_at)| {
            if cached_at.elapsed() < self.ttl {
                Some(results.clone())
            } else {
                None
            }
        })
    }

    pub fn set(&self, query: &str, results: Vec<SkillSearchResult>) {
        let mut entries = self.entries.lock().unwrap();
        entries.insert(query.to_string(), (results, Instant::now()));
    }
}

/// Search GitHub for skill repositories matching the query and topic filters.
pub async fn search_github(
    client: &reqwest::Client,
    query: &str,
    topics: &[&str],
) -> Result<Vec<SkillSearchResult>> {
    let url = build_search_url(query, topics);

    let response = client
        .get(&url)
        .header("User-Agent", "zeptoclaw")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?;

    if !response.status().is_success() {
        return Ok(vec![]); // Rate limited or error — return empty
    }

    let search_response: GitHubSearchResponse = response.json().await?;

    let results: Vec<SkillSearchResult> = search_response
        .items
        .into_iter()
        .map(|repo| {
            let score = compute_quality_score(&repo, false); // TODO: check SKILL.md via API
            SkillSearchResult::from_github(repo, score)
        })
        .collect();

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quality_score_high_stars() {
        let result = GitHubRepo {
            name: "test-skill".into(),
            full_name: "user/test-skill".into(),
            description: Some("A useful skill for data processing".into()),
            html_url: "https://github.com/user/test-skill".into(),
            stargazers_count: 100,
            license: Some(License {
                spdx_id: Some("MIT".into()),
            }),
            updated_at: "2026-02-20T00:00:00Z".into(),
            topics: vec!["zeptoclaw-skill".into()],
        };
        let score = compute_quality_score(&result, true);
        assert!(
            score > 0.5,
            "High-star repo should score > 0.5, got {}",
            score
        );
    }

    #[test]
    fn test_quality_score_no_license() {
        let with_license = GitHubRepo {
            name: "a".into(),
            full_name: "u/a".into(),
            description: Some("desc".into()),
            html_url: "https://github.com/u/a".into(),
            stargazers_count: 10,
            license: Some(License {
                spdx_id: Some("MIT".into()),
            }),
            updated_at: "2026-01-01T00:00:00Z".into(),
            topics: vec![],
        };
        let without_license = GitHubRepo {
            license: None,
            ..with_license.clone()
        };
        let score_with = compute_quality_score(&with_license, false);
        let score_without = compute_quality_score(&without_license, false);
        assert!(score_with > score_without);
    }

    #[test]
    fn test_quality_score_has_skill_md_bonus() {
        let repo = GitHubRepo {
            name: "a".into(),
            full_name: "u/a".into(),
            description: Some("desc".into()),
            html_url: "https://github.com/u/a".into(),
            stargazers_count: 5,
            license: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
            topics: vec![],
        };
        let without = compute_quality_score(&repo, false);
        let with_skill = compute_quality_score(&repo, true);
        assert!(with_skill > without, "SKILL.md bonus should increase score");
    }

    #[test]
    fn test_build_search_url() {
        let url = build_search_url("web scraping", &["zeptoclaw-skill"]);
        assert!(url.contains("web+scraping"));
        assert!(url.contains("topic:zeptoclaw-skill"));
    }

    #[test]
    fn test_build_search_url_multiple_topics() {
        let url = build_search_url("test", &["zeptoclaw-skill", "openclaw-skill"]);
        assert!(url.contains("topic:zeptoclaw-skill"));
        assert!(url.contains("topic:openclaw-skill"));
    }

    #[test]
    fn test_parse_search_response() {
        let json = serde_json::json!({
            "total_count": 1,
            "items": [{
                "name": "my-skill",
                "full_name": "user/my-skill",
                "description": "A cool skill",
                "html_url": "https://github.com/user/my-skill",
                "stargazers_count": 42,
                "license": {"spdx_id": "MIT"},
                "updated_at": "2026-02-20T00:00:00Z",
                "topics": ["zeptoclaw-skill"]
            }]
        });
        let response: GitHubSearchResponse = serde_json::from_value(json).unwrap();
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].stargazers_count, 42);
    }

    #[test]
    fn test_skill_result_from_github_repo() {
        let repo = GitHubRepo {
            name: "my-skill".into(),
            full_name: "user/my-skill".into(),
            description: Some("A skill".into()),
            html_url: "https://github.com/user/my-skill".into(),
            stargazers_count: 10,
            license: Some(License {
                spdx_id: Some("MIT".into()),
            }),
            updated_at: "2026-02-20T00:00:00Z".into(),
            topics: vec!["zeptoclaw-skill".into()],
        };
        let result = SkillSearchResult::from_github(repo, 0.8);
        assert_eq!(result.source, SkillSource::GitHub);
        assert_eq!(result.name, "my-skill");
        assert_eq!(result.stars, 10);
    }

    #[test]
    fn test_cache_miss_returns_none() {
        let cache = SearchCache::new(Duration::from_secs(60));
        assert!(cache.get("query").is_none());
    }

    #[test]
    fn test_cache_hit() {
        let cache = SearchCache::new(Duration::from_secs(60));
        let results = vec![SkillSearchResult {
            name: "test".into(),
            slug: "user/test".into(),
            description: "desc".into(),
            source: SkillSource::GitHub,
            score: 0.5,
            stars: 10,
            url: "https://github.com/user/test".into(),
        }];
        cache.set("query", results.clone());
        let cached = cache.get("query").unwrap();
        assert_eq!(cached.len(), 1);
    }

    #[test]
    fn test_cache_expired() {
        let cache = SearchCache::new(Duration::from_millis(1));
        cache.set("query", vec![]);
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get("query").is_none());
    }
}
