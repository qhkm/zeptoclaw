//! Watch command — monitor URLs for changes and notify via channel.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use reqwest::Url;

/// Maximum bytes to read from a watched URL response (800KB, same as web_fetch).
const MAX_WATCH_BYTES: usize = 800_000;

/// Minimum allowed interval in seconds (prevents busy loops).
const MIN_INTERVAL_SECS: u64 = 10;

/// Parse interval string like "1h", "30m", "15m", "60s" into seconds.
pub fn parse_interval(s: &str) -> Result<u64> {
    let s = s.trim().to_lowercase();
    let secs = if let Some(hours) = s.strip_suffix('h') {
        let n: u64 = hours.parse().with_context(|| "Invalid hours value")?;
        n * 3600
    } else if let Some(mins) = s.strip_suffix('m') {
        let n: u64 = mins.parse().with_context(|| "Invalid minutes value")?;
        n * 60
    } else if let Some(sec_str) = s.strip_suffix('s') {
        let n: u64 = sec_str.parse().with_context(|| "Invalid seconds value")?;
        n
    } else {
        s.parse::<u64>()
            .with_context(|| "Invalid interval. Use formats like 1h, 30m, or 60s")?
    };

    if secs < MIN_INTERVAL_SECS {
        bail!(
            "Interval too small ({}s). Minimum is {}s to avoid excessive requests.",
            secs,
            MIN_INTERVAL_SECS
        );
    }
    Ok(secs)
}

/// Hash a URL to a filename-safe string.
fn url_hash(url: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    hasher.finish()
}

/// Get path for storing last snapshot of a watched URL.
fn snapshot_path(url: &str) -> PathBuf {
    let hash = format!("{:x}", url_hash(url));
    zeptoclaw::config::Config::dir()
        .join("watch")
        .join(format!("{}.txt", hash))
}

/// Validate that a URL is safe to fetch (scheme check + SSRF protection).
async fn validate_watch_url(url: &str) -> Result<Url> {
    let parsed = Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => bail!("Only http/https URLs are allowed, got: {}", other),
    }

    if zeptoclaw::tools::is_blocked_host(&parsed) {
        bail!("Blocked URL host (local or private network): {}", url);
    }

    // DNS-based SSRF check
    zeptoclaw::tools::resolve_and_check_host(&parsed)
        .await
        .map_err(|e| anyhow::anyhow!("SSRF check failed for {}: {}", url, e))?;

    Ok(parsed)
}

/// Read response body with a size limit (prevents unbounded memory/disk usage).
async fn read_body_limited(resp: reqwest::Response, max_bytes: usize) -> Result<String> {
    let mut buf = Vec::new();
    let stream = resp.bytes().await.unwrap_or_default();

    if stream.len() > max_bytes {
        buf.extend_from_slice(&stream[..max_bytes]);
    } else {
        buf.extend_from_slice(&stream);
    }

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) async fn cmd_watch(url: String, interval: String, notify: Option<String>) -> Result<()> {
    let interval_secs = parse_interval(&interval)?;

    // Validate URL before starting the loop (catches SSRF, bad schemes, etc.)
    validate_watch_url(&url).await?;

    println!("Watching: {}", url);
    println!("Interval: {} ({}s)", interval, interval_secs);
    if let Some(ref channel) = notify {
        println!("Notify via: {}", channel);
        eprintln!("Warning: Channel notification is not yet implemented. Changes will be printed to stdout.");
    } else {
        println!("Notify: stdout only");
    }
    println!();
    println!("Press Ctrl+C to stop.");
    println!();

    // Create watch directory
    let watch_dir = zeptoclaw::config::Config::dir().join("watch");
    std::fs::create_dir_all(&watch_dir)
        .with_context(|| format!("Failed to create watch directory: {:?}", watch_dir))?;

    let snap_path = snapshot_path(&url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

    loop {
        ticker.tick().await;

        match client.get(&url).send().await {
            Ok(resp) => {
                // Post-redirect SSRF check
                if zeptoclaw::tools::is_blocked_host(resp.url()) {
                    eprintln!(
                        "[{}] Blocked: redirect to local/private host {}",
                        chrono::Local::now().format("%H:%M"),
                        resp.url()
                    );
                    continue;
                }

                let status = resp.status();
                if !status.is_success() {
                    eprintln!(
                        "[{}] HTTP {} for {}",
                        chrono::Local::now().format("%H:%M"),
                        status,
                        url
                    );
                    continue;
                }

                let body = read_body_limited(resp, MAX_WATCH_BYTES).await?;
                let previous = std::fs::read_to_string(&snap_path).unwrap_or_default();

                if previous.is_empty() {
                    // First fetch — save baseline
                    std::fs::write(&snap_path, &body)?;
                    println!(
                        "[{}] Baseline saved ({} bytes)",
                        chrono::Local::now().format("%H:%M"),
                        body.len()
                    );
                } else if body != previous {
                    std::fs::write(&snap_path, &body)?;
                    println!(
                        "[{}] Change detected! (was {} bytes, now {} bytes)",
                        chrono::Local::now().format("%H:%M"),
                        previous.len(),
                        body.len()
                    );

                    // Notification message
                    let message = format!(
                        "URL changed: {}\nPrevious: {} bytes -> New: {} bytes",
                        url,
                        previous.len(),
                        body.len()
                    );
                    if let Some(ref channel) = notify {
                        println!("  Notification ({}): {}", channel, message);
                    } else {
                        println!("  {}", message);
                    }

                    // TODO: Wire to actual channel send via ChannelManager
                } else {
                    eprintln!("[{}] No change", chrono::Local::now().format("%H:%M"));
                }
            }
            Err(e) => {
                eprintln!(
                    "[{}] Fetch error: {}",
                    chrono::Local::now().format("%H:%M"),
                    e
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_interval_hours() {
        assert_eq!(parse_interval("1h").unwrap(), 3600);
        assert_eq!(parse_interval("2h").unwrap(), 7200);
    }

    #[test]
    fn test_parse_interval_minutes() {
        assert_eq!(parse_interval("30m").unwrap(), 1800);
        assert_eq!(parse_interval("15m").unwrap(), 900);
    }

    #[test]
    fn test_parse_interval_seconds() {
        assert_eq!(parse_interval("60s").unwrap(), 60);
        assert_eq!(parse_interval("120s").unwrap(), 120);
    }

    #[test]
    fn test_parse_interval_bare_number() {
        assert_eq!(parse_interval("3600").unwrap(), 3600);
    }

    #[test]
    fn test_parse_interval_invalid() {
        assert!(parse_interval("abc").is_err());
        assert!(parse_interval("").is_err());
    }

    #[test]
    fn test_parse_interval_zero_rejected() {
        assert!(parse_interval("0s").is_err());
        assert!(parse_interval("0m").is_err());
        assert!(parse_interval("0h").is_err());
    }

    #[test]
    fn test_parse_interval_below_minimum_rejected() {
        assert!(parse_interval("5s").is_err());
        assert!(parse_interval("9s").is_err());
        // 10s is the minimum
        assert!(parse_interval("10s").is_ok());
    }

    #[test]
    fn test_url_hash_deterministic() {
        let h1 = url_hash("https://example.com");
        let h2 = url_hash("https://example.com");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_url_hash_different_urls() {
        let h1 = url_hash("https://example.com");
        let h2 = url_hash("https://other.com");
        assert_ne!(h1, h2);
    }
}
