//! Dependency fetcher trait and implementations.
//!
//! `DepFetcher` abstracts network/system calls for testability.
//! `RealFetcher` makes actual system calls.
//! `MockFetcher` is used in tests.

use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::error::{Result, ZeptoError};

use super::types::DepKind;

/// Result of a fetch operation.
#[derive(Debug, Clone)]
pub struct FetchResult {
    /// Path where the artifact was installed.
    pub path: String,
    /// Resolved version that was installed.
    pub version: String,
}

/// Abstracts the actual download/install operations.
#[async_trait]
pub trait DepFetcher: Send + Sync {
    /// Install a dependency. Returns the installed path and version.
    async fn install(&self, kind: &DepKind, dest_dir: &Path) -> Result<FetchResult>;

    /// Check if a command/binary is available on the system.
    fn is_command_available(&self, command: &str) -> bool;
}

/// Real fetcher that makes actual system calls.
pub struct RealFetcher;

#[async_trait]
impl DepFetcher for RealFetcher {
    async fn install(&self, kind: &DepKind, dest_dir: &Path) -> Result<FetchResult> {
        match kind {
            DepKind::Binary {
                repo,
                asset_pattern,
                version,
            } => install_github_binary(repo, asset_pattern, version, dest_dir).await,
            DepKind::DockerImage { image, tag, .. } => {
                let output = tokio::process::Command::new("docker")
                    .args(["pull", &format!("{}:{}", image, tag)])
                    .output()
                    .await
                    .map_err(|e| ZeptoError::Tool(format!("Failed to run docker pull: {}", e)))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ZeptoError::Tool(format!("docker pull failed: {}", stderr)));
                }
                Ok(FetchResult {
                    path: format!("{}:{}", image, tag),
                    version: tag.clone(),
                })
            }
            DepKind::NpmPackage {
                package, version, ..
            } => {
                let node_dir = dest_dir.join("node_modules");
                std::fs::create_dir_all(&node_dir)?;
                let output = tokio::process::Command::new("npm")
                    .args([
                        "install",
                        "--prefix",
                        &dest_dir.to_string_lossy(),
                        &format!("{}@{}", package, version),
                    ])
                    .output()
                    .await
                    .map_err(|e| ZeptoError::Tool(format!("npm install failed: {}", e)))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ZeptoError::Tool(format!("npm install failed: {}", stderr)));
                }
                Ok(FetchResult {
                    path: node_dir.to_string_lossy().to_string(),
                    version: version.clone(),
                })
            }
            DepKind::PipPackage {
                package, version, ..
            } => {
                let venv_dir = dest_dir.join("venvs").join(package);
                std::fs::create_dir_all(&venv_dir)?;
                let venv_out = tokio::process::Command::new("python3")
                    .args(["-m", "venv", &venv_dir.to_string_lossy()])
                    .output()
                    .await
                    .map_err(|e| ZeptoError::Tool(format!("venv creation failed: {}", e)))?;
                if !venv_out.status.success() {
                    let stderr = String::from_utf8_lossy(&venv_out.stderr);
                    return Err(ZeptoError::Tool(format!(
                        "venv creation failed: {}",
                        stderr
                    )));
                }
                let pip_bin = venv_dir.join("bin").join("pip");
                let pip_out = tokio::process::Command::new(&pip_bin)
                    .args(["install", &format!("{}{}", package, version)])
                    .output()
                    .await
                    .map_err(|e| ZeptoError::Tool(format!("pip install failed: {}", e)))?;
                if !pip_out.status.success() {
                    let stderr = String::from_utf8_lossy(&pip_out.stderr);
                    return Err(ZeptoError::Tool(format!("pip install failed: {}", stderr)));
                }
                Ok(FetchResult {
                    path: venv_dir.to_string_lossy().to_string(),
                    version: version.clone(),
                })
            }
        }
    }

    fn is_command_available(&self, command: &str) -> bool {
        std::process::Command::new("which")
            .arg(command)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

async fn install_github_binary(
    repo: &str,
    asset_pattern: &str,
    version: &str,
    dest_dir: &Path,
) -> Result<FetchResult> {
    let resolved_pattern = super::types::resolve_asset_pattern(asset_pattern);
    let client = build_github_client()?;
    let requested_version = non_empty(version);
    let release = fetch_github_release(&client, repo, requested_version.as_deref()).await?;
    let asset = select_release_asset(&release.assets, &resolved_pattern).ok_or_else(|| {
        let names = release
            .assets
            .iter()
            .map(|asset| asset.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        ZeptoError::Tool(format!(
            "No release asset matched pattern '{}' in repo '{}' (assets: {})",
            resolved_pattern,
            repo,
            if names.is_empty() {
                "<none>".to_string()
            } else {
                names
            }
        ))
    })?;
    let bytes = download_asset_bytes(&client, &asset.browser_download_url).await?;

    let bin_dir = dest_dir.join("bin");
    fs::create_dir_all(&bin_dir)?;
    let install_name = infer_install_name(&resolved_pattern, &asset.name);
    let install_path = bin_dir.join(install_name);

    let lower_name = asset.name.to_ascii_lowercase();
    if is_archive_name(&lower_name) {
        let temp_root = tempfile::tempdir()?;
        let archive_path = temp_root.path().join(&asset.name);
        fs::write(&archive_path, &bytes)?;
        let extract_dir = temp_root.path().join("extract");
        fs::create_dir_all(&extract_dir)?;

        extract_archive(&archive_path, &lower_name, &extract_dir).await?;
        let extracted_bin = find_extracted_binary(&extract_dir, &resolved_pattern)?;
        fs::copy(&extracted_bin, &install_path).map_err(|e| {
            ZeptoError::Tool(format!(
                "Failed to copy extracted binary '{}' to '{}': {}",
                extracted_bin.display(),
                install_path.display(),
                e
            ))
        })?;
    } else {
        fs::write(&install_path, bytes).map_err(|e| {
            ZeptoError::Tool(format!(
                "Failed to write downloaded binary '{}': {}",
                install_path.display(),
                e
            ))
        })?;
    }

    set_executable_mode(&install_path)?;

    Ok(FetchResult {
        path: install_path.to_string_lossy().to_string(),
        version: release.tag_name,
    })
}

fn build_github_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .map_err(|e| ZeptoError::Tool(format!("Failed to build HTTP client: {}", e)))
}

async fn fetch_github_release(
    client: &reqwest::Client,
    repo: &str,
    version: Option<&str>,
) -> Result<GitHubRelease> {
    let url = match version {
        Some(tag) => format!(
            "https://api.github.com/repos/{}/releases/tags/{}",
            repo, tag
        ),
        None => format!("https://api.github.com/repos/{}/releases/latest", repo),
    };

    let mut req = client
        .get(&url)
        .header(USER_AGENT, "zeptoclaw-deps/0.1")
        .header(ACCEPT, "application/vnd.github+json");
    if let Some(token) = github_token() {
        req = req.header(AUTHORIZATION, format!("Bearer {}", token));
    }

    let response = req
        .send()
        .await
        .map_err(|e| ZeptoError::Tool(format!("GitHub request failed for '{}': {}", url, e)))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| {
        ZeptoError::Tool(format!(
            "Failed reading GitHub response for '{}': {}",
            url, e
        ))
    })?;
    if !status.is_success() {
        let snippet = body.chars().take(300).collect::<String>();
        return Err(ZeptoError::Tool(format!(
            "GitHub release lookup failed for '{}': {} {}",
            url,
            status.as_u16(),
            snippet
        )));
    }

    serde_json::from_str(&body).map_err(|e| {
        ZeptoError::Tool(format!(
            "Failed parsing GitHub release metadata for '{}': {}",
            url, e
        ))
    })
}

async fn download_asset_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let mut req = client.get(url).header(USER_AGENT, "zeptoclaw-deps/0.1");
    if let Some(token) = github_token() {
        req = req.header(AUTHORIZATION, format!("Bearer {}", token));
    }

    let response = req
        .send()
        .await
        .map_err(|e| ZeptoError::Tool(format!("Asset download request failed: {}", e)))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ZeptoError::Tool(format!("Failed reading downloaded asset body: {}", e)))?;
    if !status.is_success() {
        let snippet = String::from_utf8_lossy(&bytes)
            .chars()
            .take(300)
            .collect::<String>();
        return Err(ZeptoError::Tool(format!(
            "Asset download failed ({}): {}",
            status.as_u16(),
            snippet
        )));
    }
    Ok(bytes.to_vec())
}

fn select_release_asset<'a>(
    assets: &'a [GitHubAsset],
    resolved_pattern: &str,
) -> Option<&'a GitHubAsset> {
    assets
        .iter()
        .find(|asset| asset.name == resolved_pattern)
        .or_else(|| {
            assets.iter().find(|asset| {
                asset.name.starts_with(&format!("{}.", resolved_pattern))
                    || asset.name.starts_with(&format!("{}-", resolved_pattern))
                    || asset.name.starts_with(&format!("{}/", resolved_pattern))
            })
        })
        .or_else(|| {
            assets
                .iter()
                .find(|asset| asset.name.contains(resolved_pattern))
        })
}

fn infer_install_name(resolved_pattern: &str, asset_name: &str) -> String {
    if cfg!(windows)
        && !resolved_pattern.to_ascii_lowercase().ends_with(".exe")
        && asset_name.to_ascii_lowercase().ends_with(".exe")
    {
        return format!("{}.exe", resolved_pattern);
    }
    resolved_pattern.to_string()
}

fn is_archive_name(name_lower: &str) -> bool {
    name_lower.ends_with(".zip")
        || name_lower.ends_with(".tar.gz")
        || name_lower.ends_with(".tgz")
        || name_lower.ends_with(".tar")
}

async fn extract_archive(archive_path: &Path, name_lower: &str, extract_dir: &Path) -> Result<()> {
    let output = if name_lower.ends_with(".zip") {
        Command::new("unzip")
            .args(["-qq"])
            .arg(archive_path)
            .args(["-d", extract_dir.to_string_lossy().as_ref()])
            .output()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to run unzip: {}", e)))?
    } else if name_lower.ends_with(".tar.gz") || name_lower.ends_with(".tgz") {
        Command::new("tar")
            .args(["xzf"])
            .arg(archive_path)
            .args(["-C", extract_dir.to_string_lossy().as_ref()])
            .output()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to run tar xzf: {}", e)))?
    } else if name_lower.ends_with(".tar") {
        Command::new("tar")
            .args(["xf"])
            .arg(archive_path)
            .args(["-C", extract_dir.to_string_lossy().as_ref()])
            .output()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to run tar xf: {}", e)))?
    } else {
        return Err(ZeptoError::Tool(format!(
            "Unsupported archive format '{}'",
            archive_path.display()
        )));
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ZeptoError::Tool(format!(
            "Archive extraction failed for '{}': {}",
            archive_path.display(),
            stderr
        )));
    }
    Ok(())
}

fn find_extracted_binary(extract_dir: &Path, resolved_pattern: &str) -> Result<PathBuf> {
    let mut files = Vec::new();
    collect_regular_files(extract_dir, &mut files)?;

    if files.is_empty() {
        return Err(ZeptoError::Tool(format!(
            "Extracted archive '{}' has no regular files",
            extract_dir.display()
        )));
    }

    if let Some(found) = files.iter().find(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == resolved_pattern || n == format!("{}.exe", resolved_pattern))
            .unwrap_or(false)
    }) {
        return Ok(found.clone());
    }

    if let Some(found) = files.iter().find(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(resolved_pattern))
            .unwrap_or(false)
    }) {
        return Ok(found.clone());
    }

    if files.len() == 1 {
        return Ok(files[0].clone());
    }

    let names = files
        .iter()
        .map(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<invalid utf8>")
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(ZeptoError::Tool(format!(
        "Unable to identify binary from extracted archive for '{}'; candidates: {}",
        resolved_pattern, names
    )))
}

fn collect_regular_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_regular_files(&path, out)?;
        } else if ft.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn set_executable_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Mock fetcher for tests.
#[cfg(test)]
pub struct MockFetcher {
    pub install_result: std::sync::Mutex<Option<Result<FetchResult>>>,
    pub commands_available: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl MockFetcher {
    pub fn success(path: &str, version: &str) -> Self {
        Self {
            install_result: std::sync::Mutex::new(Some(Ok(FetchResult {
                path: path.to_string(),
                version: version.to_string(),
            }))),
            commands_available: std::sync::Mutex::new(vec![]),
        }
    }

    pub fn failure(msg: &str) -> Self {
        Self {
            install_result: std::sync::Mutex::new(Some(Err(ZeptoError::Tool(msg.to_string())))),
            commands_available: std::sync::Mutex::new(vec![]),
        }
    }

    pub fn with_commands(mut self, cmds: Vec<&str>) -> Self {
        self.commands_available =
            std::sync::Mutex::new(cmds.iter().map(|s| s.to_string()).collect());
        self
    }
}

#[cfg(test)]
#[async_trait]
impl DepFetcher for MockFetcher {
    async fn install(&self, _kind: &DepKind, _dest_dir: &Path) -> Result<FetchResult> {
        self.install_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Err(ZeptoError::Tool("No mock result configured".to_string())))
    }

    fn is_command_available(&self, command: &str) -> bool {
        self.commands_available
            .lock()
            .unwrap()
            .contains(&command.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_fetch_result_construction() {
        let result = FetchResult {
            path: "/usr/local/bin/test".to_string(),
            version: "v1.0.0".to_string(),
        };
        assert_eq!(result.path, "/usr/local/bin/test");
        assert_eq!(result.version, "v1.0.0");
    }

    #[test]
    fn test_mock_fetcher_success() {
        let fetcher = MockFetcher::success("/bin/test", "v1.0.0");
        assert!(!fetcher.is_command_available("docker"));
    }

    #[test]
    fn test_mock_fetcher_with_commands() {
        let fetcher =
            MockFetcher::success("/bin/test", "v1.0.0").with_commands(vec!["docker", "npm"]);
        assert!(fetcher.is_command_available("docker"));
        assert!(fetcher.is_command_available("npm"));
        assert!(!fetcher.is_command_available("pip"));
    }

    #[tokio::test]
    async fn test_mock_fetcher_install_success() {
        let fetcher = MockFetcher::success("/bin/test", "v1.0.0");
        let kind = DepKind::Binary {
            repo: "test/repo".to_string(),
            asset_pattern: "bin".to_string(),
            version: "v1.0.0".to_string(),
        };
        let result = fetcher.install(&kind, Path::new("/tmp")).await;
        assert!(result.is_ok());
        let fr = result.unwrap();
        assert_eq!(fr.path, "/bin/test");
        assert_eq!(fr.version, "v1.0.0");
    }

    #[tokio::test]
    async fn test_mock_fetcher_install_failure() {
        let fetcher = MockFetcher::failure("test error");
        let kind = DepKind::Binary {
            repo: "test/repo".to_string(),
            asset_pattern: "bin".to_string(),
            version: "v1.0.0".to_string(),
        };
        let result = fetcher.install(&kind, Path::new("/tmp")).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_real_fetcher_is_command_available() {
        let fetcher = RealFetcher;
        assert!(fetcher.is_command_available("ls"));
        assert!(!fetcher.is_command_available("nonexistent_command_xyz_123"));
    }

    #[test]
    fn test_select_release_asset_prefers_exact_match() {
        let assets = vec![
            GitHubAsset {
                name: "tool-linux-amd64.tar.gz".to_string(),
                browser_download_url: "https://example.com/a".to_string(),
            },
            GitHubAsset {
                name: "tool-linux-amd64".to_string(),
                browser_download_url: "https://example.com/b".to_string(),
            },
        ];
        let selected =
            select_release_asset(&assets, "tool-linux-amd64").expect("expected a selected asset");
        assert_eq!(selected.name, "tool-linux-amd64");
    }

    #[test]
    fn test_select_release_asset_fallback_to_prefixed_name() {
        let assets = vec![GitHubAsset {
            name: "tool-linux-amd64.tar.gz".to_string(),
            browser_download_url: "https://example.com/a".to_string(),
        }];
        let selected =
            select_release_asset(&assets, "tool-linux-amd64").expect("expected a selected asset");
        assert_eq!(selected.name, "tool-linux-amd64.tar.gz");
    }

    #[test]
    fn test_find_extracted_binary_by_exact_name() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("nested");
        fs::create_dir_all(&nested).expect("mkdir");
        let bin = nested.join("whatsmeow-bridge-darwin-arm64");
        fs::write(&bin, b"binary").expect("write");
        let found = find_extracted_binary(dir.path(), "whatsmeow-bridge-darwin-arm64")
            .expect("find binary");
        assert_eq!(found, bin);
    }

    #[test]
    fn test_find_extracted_binary_single_file_fallback() {
        let dir = tempdir().expect("tempdir");
        let bin = dir.path().join("bridge");
        fs::write(&bin, b"binary").expect("write");
        let found = find_extracted_binary(dir.path(), "whatsmeow-bridge-darwin-arm64")
            .expect("find binary");
        assert_eq!(found, bin);
    }
}
