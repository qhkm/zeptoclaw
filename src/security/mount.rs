//! Runtime mount validation utilities.
//!
//! Validates additional runtime mounts against an allowlist file to prevent
//! accidental exposure of sensitive host paths.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::audit::{log_audit_event, AuditCategory, AuditSeverity};
use crate::error::{Result, ZeptoError};

pub const DEFAULT_BLOCKED_PATTERNS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".gpg",
    ".aws",
    ".azure",
    ".gcloud",
    ".kube",
    ".docker",
    "credentials",
    ".env",
    ".netrc",
    "id_rsa",
    "id_ed25519",
    "private_key",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AllowedRoot {
    path: String,
    #[serde(default)]
    allow_read_write: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MountAllowlist {
    #[serde(default)]
    allowed_roots: Vec<AllowedRoot>,
    #[serde(default)]
    blocked_patterns: Vec<String>,
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(suffix) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(suffix);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|e| {
        ZeptoError::SecurityViolation(format!(
            "Mount path '{}' is invalid or does not exist: {}",
            path.display(),
            e
        ))
    })
}

fn path_contains_blocked_pattern(path: &Path, patterns: &[String]) -> Option<String> {
    let lower_path = path.to_string_lossy().to_lowercase();
    patterns
        .iter()
        .find(|pattern| lower_path.contains(&pattern.to_lowercase()))
        .cloned()
}

fn parse_mount_spec(spec: &str) -> Result<(String, String, bool)> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.as_slice() {
        [host, container] => Ok((host.to_string(), container.to_string(), false)),
        [host, container, mode] if *mode == "ro" => {
            Ok((host.to_string(), container.to_string(), true))
        }
        [_, _, mode] => Err(ZeptoError::SecurityViolation(format!(
            "Invalid mount mode '{}'; only 'ro' is supported",
            mode
        ))),
        _ => Err(ZeptoError::SecurityViolation(format!(
            "Invalid mount format '{}'; expected 'host:container' or 'host:container:ro'",
            spec
        ))),
    }
}

fn load_allowlist(allowlist_path: &Path) -> Result<MountAllowlist> {
    if !allowlist_path.exists() {
        return Err(ZeptoError::SecurityViolation(format!(
            "Mount allowlist not found at '{}'",
            allowlist_path.display()
        )));
    }

    let content = std::fs::read_to_string(allowlist_path).map_err(|e| {
        ZeptoError::SecurityViolation(format!(
            "Failed to read mount allowlist '{}': {}",
            allowlist_path.display(),
            e
        ))
    })?;

    serde_json::from_str::<MountAllowlist>(&content).map_err(|e| {
        ZeptoError::SecurityViolation(format!(
            "Invalid mount allowlist JSON at '{}': {}",
            allowlist_path.display(),
            e
        ))
    })
}

fn is_under_root(path: &Path, root: &Path) -> bool {
    let relative = match path.strip_prefix(root) {
        Ok(relative) => relative,
        Err(_) => return false,
    };
    !relative.is_absolute()
}

/// Validate additional mounts and return normalized mount specs.
///
/// If `mounts` is empty, this function returns `Ok(vec![])` and does not read
/// the allowlist file.
pub fn validate_extra_mounts(mounts: &[String], allowlist_path: &str) -> Result<Vec<String>> {
    if mounts.is_empty() {
        return Ok(Vec::new());
    }

    let allowlist_path = expand_path(allowlist_path);
    let allowlist = load_allowlist(&allowlist_path)?;
    if allowlist.allowed_roots.is_empty() {
        return Err(ZeptoError::SecurityViolation(
            "Mount allowlist has no allowedRoots entries".to_string(),
        ));
    }

    let mut blocked_patterns: Vec<String> = DEFAULT_BLOCKED_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    blocked_patterns.extend(allowlist.blocked_patterns.clone());

    let mut normalized = Vec::with_capacity(mounts.len());

    for mount in mounts {
        let (host, container, requested_read_only) = parse_mount_spec(mount)?;

        if container.is_empty() || !container.starts_with('/') || container.contains("..") {
            return Err(ZeptoError::SecurityViolation(format!(
                "Invalid container mount path '{}' in '{}'",
                container, mount
            )));
        }

        let host_path = canonicalize_existing(&expand_path(&host))?;

        if let Some(pattern) = path_contains_blocked_pattern(&host_path, &blocked_patterns) {
            log_audit_event(
                AuditCategory::MountSecurity,
                AuditSeverity::Critical,
                "mount_blocked_pattern",
                &format!(
                    "Mount '{}' blocked by pattern '{}'",
                    host_path.display(),
                    pattern
                ),
                true,
            );
            return Err(ZeptoError::SecurityViolation(format!(
                "Mount '{}' blocked by pattern '{}'",
                host_path.display(),
                pattern
            )));
        }

        let allowed_root = allowlist
            .allowed_roots
            .iter()
            .filter_map(|root| {
                let root_path = canonicalize_existing(&expand_path(&root.path)).ok()?;
                if is_under_root(&host_path, &root_path) {
                    Some(root)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| {
                log_audit_event(
                    AuditCategory::MountSecurity,
                    AuditSeverity::Critical,
                    "mount_outside_roots",
                    &format!(
                        "Mount '{}' is outside allowedRoots in '{}'",
                        host_path.display(),
                        allowlist_path.display()
                    ),
                    true,
                );
                ZeptoError::SecurityViolation(format!(
                    "Mount '{}' is outside allowedRoots in '{}'",
                    host_path.display(),
                    allowlist_path.display()
                ))
            })?;

        let effective_read_only = requested_read_only || !allowed_root.allow_read_write;
        let host_norm = host_path.to_string_lossy();
        let normalized_mount = if effective_read_only {
            format!("{}:{}:ro", host_norm, container)
        } else {
            format!("{}:{}", host_norm, container)
        };
        normalized.push(normalized_mount);
    }

    Ok(normalized)
}

/// Validate that a mount spec does not reference any blocked sensitive paths.
///
/// This performs a lightweight check against [`DEFAULT_BLOCKED_PATTERNS`] without
/// requiring a full allowlist file.  Useful for contexts (e.g. the container
/// agent proxy) where the full allowlist may not be configured.
///
/// The mount spec must follow `host_path:container_path[:ro]` format.
pub fn validate_mount_not_blocked(mount_spec: &str) -> Result<()> {
    let (host, container, _read_only) = parse_mount_spec(mount_spec)?;

    if container.is_empty() || !container.starts_with('/') || container.contains("..") {
        return Err(ZeptoError::SecurityViolation(format!(
            "Invalid container mount path '{}' in '{}'",
            container, mount_spec
        )));
    }

    // Check host path against default blocked patterns.
    let blocked: Vec<String> = DEFAULT_BLOCKED_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect();

    let host_path = expand_path(&host);
    if let Some(pattern) = path_contains_blocked_pattern(&host_path, &blocked) {
        return Err(ZeptoError::SecurityViolation(format!(
            "Mount '{}' blocked by sensitive pattern '{}'",
            mount_spec, pattern
        )));
    }

    // Also check for path traversal in host path.
    if host.contains("..") {
        return Err(ZeptoError::SecurityViolation(format!(
            "Mount host path '{}' contains path traversal",
            host
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_allowlist(path: &Path, allowed_root: &Path, allow_rw: bool) {
        let json = format!(
            r#"{{
  "allowedRoots": [{{"path": "{}", "allowReadWrite": {}}}],
  "blockedPatterns": []
}}"#,
            allowed_root.display(),
            if allow_rw { "true" } else { "false" }
        );
        std::fs::write(path, json).unwrap();
    }

    #[test]
    fn test_validate_empty_mounts_does_not_require_allowlist() {
        let result = validate_extra_mounts(&[], "/nonexistent/allowlist.json");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_validate_mount_in_allowed_root() {
        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let file = data_dir.join("file.txt");
        std::fs::write(&file, "ok").unwrap();

        let allowlist = temp.path().join("allowlist.json");
        write_allowlist(&allowlist, temp.path(), true);

        let mounts = vec![format!("{}:/workspace/data", file.display())];
        let validated =
            validate_extra_mounts(&mounts, allowlist.to_str().unwrap()).expect("should validate");
        assert_eq!(validated.len(), 1);
        assert!(validated[0].contains(":/workspace/data"));
    }

    #[test]
    fn test_validate_mount_forces_ro_when_root_disallows_rw() {
        let temp = tempdir().unwrap();
        let data_dir = temp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let allowlist = temp.path().join("allowlist.json");
        write_allowlist(&allowlist, temp.path(), false);

        let mounts = vec![format!("{}:/workspace/data", data_dir.display())];
        let validated =
            validate_extra_mounts(&mounts, allowlist.to_str().unwrap()).expect("should validate");
        assert!(validated[0].ends_with(":ro"));
    }

    #[test]
    fn test_validate_mount_outside_allowed_root_fails() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_dir = outside.path().join("outside");
        std::fs::create_dir_all(&outside_dir).unwrap();

        let allowlist = temp.path().join("allowlist.json");
        write_allowlist(&allowlist, temp.path(), true);

        let mounts = vec![format!("{}:/workspace/data", outside_dir.display())];
        let err = validate_extra_mounts(&mounts, allowlist.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("outside allowedRoots"));
    }

    // -----------------------------------------------------------------------
    // validate_mount_not_blocked tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_not_blocked_accepts_safe_path() {
        let temp = tempdir().unwrap();
        let safe = temp.path().join("project");
        std::fs::create_dir_all(&safe).unwrap();
        let spec = format!("{}:/data/project", safe.display());
        assert!(validate_mount_not_blocked(&spec).is_ok());
    }

    #[test]
    fn test_not_blocked_rejects_ssh_dir() {
        let result = validate_mount_not_blocked("/home/user/.ssh:/secrets");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".ssh"));
    }

    #[test]
    fn test_not_blocked_rejects_gnupg_dir() {
        let result = validate_mount_not_blocked("/home/user/.gnupg:/gpg");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".gnupg"));
    }

    #[test]
    fn test_not_blocked_rejects_kube_dir() {
        let result = validate_mount_not_blocked("/home/user/.kube:/kube");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".kube"));
    }

    #[test]
    fn test_not_blocked_rejects_credentials_in_path() {
        let result = validate_mount_not_blocked("/app/credentials:/creds");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("credentials"));
    }

    #[test]
    fn test_not_blocked_rejects_netrc() {
        let result = validate_mount_not_blocked("/home/user/.netrc:/netrc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".netrc"));
    }

    #[test]
    fn test_not_blocked_rejects_id_rsa() {
        let result = validate_mount_not_blocked("/home/user/id_rsa:/key");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("id_rsa"));
    }

    #[test]
    fn test_not_blocked_rejects_id_ed25519() {
        let result = validate_mount_not_blocked("/home/user/id_ed25519:/key");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("id_ed25519"));
    }

    #[test]
    fn test_not_blocked_rejects_traversal_in_host() {
        let result = validate_mount_not_blocked("/home/user/../etc:/etc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }

    #[test]
    fn test_not_blocked_rejects_relative_container_path() {
        let temp = tempdir().unwrap();
        let safe = temp.path().join("data");
        std::fs::create_dir_all(&safe).unwrap();
        let spec = format!("{}:relative", safe.display());
        let result = validate_mount_not_blocked(&spec);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid container"));
    }

    #[test]
    fn test_not_blocked_rejects_container_path_with_dotdot() {
        let temp = tempdir().unwrap();
        let safe = temp.path().join("data");
        std::fs::create_dir_all(&safe).unwrap();
        let spec = format!("{}:/container/../etc", safe.display());
        let result = validate_mount_not_blocked(&spec);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid container"));
    }

    #[test]
    fn test_not_blocked_rejects_malformed_mount_spec() {
        let result = validate_mount_not_blocked("single-value");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid mount format"));
    }

    #[test]
    fn test_not_blocked_rejects_invalid_mode() {
        let result = validate_mount_not_blocked("/data:/container:rw");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid mount mode"));
    }
}
