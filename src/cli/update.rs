//! Self-update command.
//!
//! Downloads the latest ZeptoClaw binary from GitHub Releases,
//! verifies its SHA256 checksum, and atomically replaces the
//! running executable.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

// ============================================================================
// GitHub API types
// ============================================================================

#[derive(serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(serde::Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

// ============================================================================
// Platform detection
// ============================================================================

/// Asset name that matches CI output for this platform.
///
/// CI produces: `zeptoclaw-{os}-{arch}` where os = macos|linux,
/// arch = aarch64|x86_64.
fn platform_asset_name() -> Result<&'static str> {
    // OS
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        bail!("unsupported OS for self-update; only macOS and Linux are supported");
    };

    // Arch
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        bail!("unsupported architecture for self-update; only aarch64 and x86_64 are supported");
    };

    // Leak a formatted string into a &'static str. This only runs once per
    // invocation so the tiny allocation is fine.
    let name: &'static str = Box::leak(format!("zeptoclaw-{os}-{arch}").into_boxed_str());
    Ok(name)
}

// ============================================================================
// GitHub Release API
// ============================================================================

async fn fetch_release(version: Option<&str>) -> Result<GitHubRelease> {
    let url = match version {
        Some(v) => {
            let tag = if v.starts_with('v') {
                v.to_string()
            } else {
                format!("v{v}")
            };
            format!("https://api.github.com/repos/qhkm/zeptoclaw/releases/tags/{tag}")
        }
        None => "https://api.github.com/repos/qhkm/zeptoclaw/releases/latest".to_string(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "zeptoclaw-self-update")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("failed to reach GitHub API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("GitHub API returned {status}: {body}");
    }

    resp.json::<GitHubRelease>()
        .await
        .context("failed to parse GitHub release response")
}

// ============================================================================
// Version comparison
// ============================================================================

/// Parse a semver-ish string into (major, minor, patch).
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

/// Returns `true` if `latest` is strictly newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_semver(current), parse_semver(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

// ============================================================================
// Download + SHA256 verification
// ============================================================================

async fn download_and_verify(asset_url: &str, checksum_url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::new();

    // Download binary
    println!("  Downloading binary...");
    let binary_bytes = client
        .get(asset_url)
        .header("User-Agent", "zeptoclaw-self-update")
        .send()
        .await
        .context("failed to download binary")?
        .bytes()
        .await
        .context("failed to read binary response")?;

    // Download checksum sidecar
    println!("  Downloading checksum...");
    let checksum_text = client
        .get(checksum_url)
        .header("User-Agent", "zeptoclaw-self-update")
        .send()
        .await
        .context("failed to download checksum")?
        .text()
        .await
        .context("failed to read checksum response")?;

    // Parse expected hash (format: "<hex>  <filename>" or just "<hex>")
    let expected_hex = checksum_text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();

    if expected_hex.len() != 64 {
        bail!(
            "invalid checksum format (expected 64 hex chars, got {}): {checksum_text}",
            expected_hex.len()
        );
    }

    // Compute SHA256
    let mut hasher = Sha256::new();
    hasher.update(&binary_bytes);
    let actual_hex = format!("{:x}", hasher.finalize());

    if actual_hex != expected_hex {
        bail!(
            "SHA256 mismatch!\n  expected: {expected_hex}\n  actual:   {actual_hex}\n\n\
             The downloaded binary may be corrupted. Aborting."
        );
    }
    println!("  Checksum verified.");

    // Write to destination
    std::fs::write(dest, &binary_bytes).context("failed to write downloaded binary")?;

    Ok(())
}

// ============================================================================
// Atomic binary replacement
// ============================================================================

fn replace_binary(new_binary: &Path) -> Result<PathBuf> {
    let current_exe =
        std::env::current_exe().context("failed to determine current executable path")?;

    // Resolve symlinks so we replace the actual file
    let current_exe = current_exe
        .canonicalize()
        .unwrap_or_else(|_| current_exe.clone());

    let backup = current_exe.with_extension("old");

    // Rename current → backup
    std::fs::rename(&current_exe, &backup)
        .with_context(|| format!("failed to backup current binary to {}", backup.display()))?;

    // Rename new → current
    if let Err(e) = std::fs::rename(new_binary, &current_exe) {
        // Rollback: restore backup
        let _ = std::fs::rename(&backup, &current_exe);
        return Err(e).context("failed to install new binary (rolled back)");
    }

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)
            .context("failed to set executable permissions")?;
    }

    Ok(backup)
}

// ============================================================================
// Command handler
// ============================================================================

pub(crate) async fn cmd_update(check: bool, version: Option<String>, force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: v{current}");

    // Fetch release
    let release = fetch_release(version.as_deref()).await?;
    let latest = &release.tag_name;
    let latest_bare = latest.strip_prefix('v').unwrap_or(latest);

    println!("Latest release:  {latest}");

    // Compare
    if !force && !is_newer(current, latest_bare) {
        println!("\nAlready up to date.");
        return Ok(());
    }

    if check {
        if is_newer(current, latest_bare) {
            println!("\nUpdate available: v{current} -> {latest}");
        }
        return Ok(());
    }

    // Resolve platform asset
    let asset_name = platform_asset_name()?;
    let checksum_name = format!("{asset_name}.sha256");

    let binary_asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .with_context(|| format!("release {latest} has no asset named '{asset_name}'"))?;

    let checksum_asset = release
        .assets
        .iter()
        .find(|a| a.name == checksum_name)
        .with_context(|| format!("release {latest} has no checksum asset '{checksum_name}'"))?;

    println!("\nDownloading {asset_name} from {latest}...");

    // Download to temp file
    let tmp_dir = tempfile::tempdir().context("failed to create temp directory")?;
    let tmp_binary = tmp_dir.path().join(asset_name);

    download_and_verify(
        &binary_asset.browser_download_url,
        &checksum_asset.browser_download_url,
        &tmp_binary,
    )
    .await?;

    // Replace
    println!("  Replacing binary...");
    let backup = replace_binary(&tmp_binary)?;

    println!("\nUpdated to {latest}!");
    println!("  Backup: {}", backup.display());
    println!("  Restart to use the new version.");

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_semver ---------------------------------------------------------

    #[test]
    fn test_parse_semver_basic() {
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
    }

    #[test]
    fn test_parse_semver_with_v_prefix() {
        assert_eq!(parse_semver("v0.5.0"), Some((0, 5, 0)));
    }

    #[test]
    fn test_parse_semver_invalid() {
        assert_eq!(parse_semver("not-a-version"), None);
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver(""), None);
    }

    // -- is_newer -------------------------------------------------------------

    #[test]
    fn test_is_newer_true() {
        assert!(is_newer("0.5.0", "0.5.1"));
        assert!(is_newer("0.5.9", "0.6.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn test_is_newer_false_same() {
        assert!(!is_newer("0.5.0", "0.5.0"));
    }

    #[test]
    fn test_is_newer_false_older() {
        assert!(!is_newer("0.6.0", "0.5.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_is_newer_handles_v_prefix() {
        assert!(is_newer("v0.5.0", "v0.5.1"));
        assert!(is_newer("0.5.0", "v0.5.1"));
        assert!(is_newer("v0.5.0", "0.5.1"));
    }

    #[test]
    fn test_is_newer_invalid_returns_false() {
        assert!(!is_newer("bad", "0.5.0"));
        assert!(!is_newer("0.5.0", "bad"));
    }

    // -- platform_asset_name --------------------------------------------------

    #[test]
    fn test_platform_asset_name_valid() {
        let name = platform_asset_name().unwrap();
        assert!(name.starts_with("zeptoclaw-"));

        // Should contain a known OS
        assert!(name.contains("macos") || name.contains("linux"));
        // Should contain a known arch
        assert!(name.contains("aarch64") || name.contains("x86_64"));
    }
}
