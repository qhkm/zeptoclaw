//! Path validation utilities for secure file operations
//!
//! This module provides path validation to prevent directory traversal attacks
//! and symlink-based workspace escapes.

use std::path::{Component, Path, PathBuf};

use crate::audit::{log_audit_event, AuditCategory, AuditSeverity};
use crate::error::{Result, ZeptoError};

/// A validated path that is guaranteed to be within the workspace.
///
/// This struct can only be created through `validate_path_in_workspace`,
/// ensuring that any `SafePath` instance represents a path that has been
/// verified to be within the allowed workspace directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafePath {
    path: PathBuf,
}

impl SafePath {
    /// Returns a reference to the underlying path.
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// Converts the SafePath into a PathBuf.
    pub fn into_path_buf(self) -> PathBuf {
        self.path
    }
}

impl AsRef<Path> for SafePath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

/// Validates that a path is within the specified workspace directory.
///
/// This function performs the following checks:
/// 1. Resolves the target path (joins with workspace if relative)
/// 2. Normalizes the path to remove `.` and `..` components
/// 3. **Checks for symlinks in any existing ancestor that escape workspace**
/// 4. Verifies the normalized path starts with the canonical workspace path
///
/// # Arguments
///
/// * `path` - The path to validate (can be relative or absolute)
/// * `workspace` - The workspace directory that the path must be within
///
/// # Returns
///
/// * `Ok(SafePath)` - If the path is valid and within the workspace
/// * `Err(ZeptoError::SecurityViolation)` - If the path escapes the workspace
///
/// # Examples
///
/// ```
/// use zeptoclaw::security::validate_path_in_workspace;
///
/// // Relative path within workspace
/// let result = validate_path_in_workspace("src/main.rs", "/workspace");
/// assert!(result.is_ok());
///
/// // Path traversal attempt
/// let result = validate_path_in_workspace("../../../etc/passwd", "/workspace");
/// assert!(result.is_err());
/// ```
pub fn validate_path_in_workspace(path: &str, workspace: &str) -> Result<SafePath> {
    // Check for obvious traversal patterns in the raw input
    if contains_traversal_pattern(path) {
        log_audit_event(
            AuditCategory::PathSecurity,
            AuditSeverity::Critical,
            "path_traversal",
            &format!("Path contains suspicious traversal pattern: {}", path),
            true,
        );
        return Err(ZeptoError::SecurityViolation(format!(
            "Path contains suspicious traversal pattern: {}",
            path
        )));
    }

    let workspace_path = Path::new(workspace);
    let target_path = Path::new(path);

    // Resolve the target path - join with workspace if relative
    let resolved_path = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        workspace_path.join(target_path)
    };

    // Normalize the path to resolve . and .. components
    let normalized_path = normalize_path(&resolved_path);

    // Get the canonical workspace path for comparison
    // If workspace doesn't exist, use the normalized workspace path
    let canonical_workspace = workspace_path
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(workspace_path));

    // SECURITY: Check for symlink escapes in existing ancestor directories
    // This prevents attacks where a subdir is a symlink to outside workspace
    check_symlink_escape(&normalized_path, &canonical_workspace)?;

    // Check if the normalized path starts with the workspace
    if !normalized_path.starts_with(&canonical_workspace) {
        log_audit_event(
            AuditCategory::PathSecurity,
            AuditSeverity::Critical,
            "path_escape",
            &format!(
                "Path escapes workspace: {} is not within {}",
                path, workspace
            ),
            true,
        );
        return Err(ZeptoError::SecurityViolation(format!(
            "Path escapes workspace: {} is not within {}",
            path, workspace
        )));
    }

    Ok(SafePath {
        path: normalized_path,
    })
}

/// Checks if any path component WITHIN the workspace is a symlink that resolves
/// outside the workspace. This prevents symlink-based escape attacks.
///
/// For a path like `/workspace/subdir/newfile.txt`:
/// - If `subdir` is a symlink to `/etc`, writing to `newfile.txt` would
///   actually write to `/etc/newfile.txt`
/// - This function detects such escapes by checking each component after
///   the workspace prefix and ensuring it stays within the workspace
fn check_symlink_escape(path: &Path, canonical_workspace: &Path) -> Result<()> {
    // Start from the canonical workspace and check only components beyond it
    // This avoids false positives from symlinks in the workspace path itself
    // (e.g., /var -> /private/var on macOS)

    // Get the relative path from workspace to target
    let relative = match path.strip_prefix(canonical_workspace) {
        Ok(rel) => rel,
        Err(_) => {
            // Path doesn't start with workspace - try with non-canonical
            // This handles cases where normalize_path returns a non-canonical path
            return Ok(());
        }
    };

    // Check each component in the relative path
    let mut current = canonical_workspace.to_path_buf();

    for component in relative.components() {
        current.push(component);

        // Use symlink_metadata instead of exists() — exists() follows symlinks
        // and returns false for dangling symlinks, letting them bypass validation.
        // symlink_metadata returns metadata for the symlink itself (lstat).
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    // It's a symlink — try to canonicalize to check where it points
                    match current.canonicalize() {
                        Ok(canonical) => {
                            // Symlink resolves — check if target is within workspace
                            if !canonical.starts_with(canonical_workspace) {
                                log_audit_event(
                                    AuditCategory::PathSecurity,
                                    AuditSeverity::Critical,
                                    "symlink_escape",
                                    &format!(
                                        "Symlink escape: '{}' resolves to '{}' outside workspace",
                                        current.display(),
                                        canonical.display()
                                    ),
                                    true,
                                );
                                return Err(ZeptoError::SecurityViolation(format!(
                                    "Symlink escape detected: '{}' resolves to '{}' which is outside workspace",
                                    current.display(),
                                    canonical.display()
                                )));
                            }
                        }
                        Err(_) => {
                            // Dangling symlink — target doesn't exist, so we can't
                            // verify it stays within workspace. Reject it since the
                            // target could be created or retargeted outside workspace.
                            log_audit_event(
                                AuditCategory::PathSecurity,
                                AuditSeverity::Critical,
                                "dangling_symlink",
                                &format!(
                                    "Dangling symlink: '{}' cannot be resolved",
                                    current.display()
                                ),
                                true,
                            );
                            return Err(ZeptoError::SecurityViolation(format!(
                                "Dangling symlink detected: '{}' target does not exist and cannot be validated",
                                current.display()
                            )));
                        }
                    }
                } else if meta.is_dir() {
                    // Regular directory — canonicalize to check for nested symlinks
                    if let Ok(canonical) = current.canonicalize() {
                        if !canonical.starts_with(canonical_workspace) {
                            log_audit_event(
                                AuditCategory::PathSecurity,
                                AuditSeverity::Critical,
                                "symlink_escape",
                                &format!(
                                    "Symlink escape: '{}' resolves to '{}' outside workspace",
                                    current.display(),
                                    canonical.display()
                                ),
                                true,
                            );
                            return Err(ZeptoError::SecurityViolation(format!(
                                "Symlink escape detected: '{}' resolves to '{}' which is outside workspace",
                                current.display(),
                                canonical.display()
                            )));
                        }
                    }
                }
                // Regular files: no escape check needed (they can't redirect traversal)
            }
            Err(_) => {
                // Path component doesn't exist yet — this is fine for new file creation
                // (e.g., writing to workspace/subdir/newfile.txt where newfile.txt doesn't exist)
            }
        }
    }

    Ok(())
}

/// Re-validates a previously validated path immediately before I/O.
///
/// This narrows the TOCTOU window between validation and use. Call this
/// right before every filesystem read/write operation on a path that was
/// validated earlier by `validate_path_in_workspace`.
///
/// Performs:
/// 1. Symlink escape check (including dangling symlink detection)
/// 2. Workspace boundary check via canonicalization
pub fn revalidate_path(path: &Path, workspace: &str) -> Result<()> {
    let workspace_path = Path::new(workspace);
    let canonical_workspace = workspace_path
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(workspace_path));

    // Re-check symlink escapes (components may have changed since initial validation)
    check_symlink_escape(path, &canonical_workspace)?;

    // If the path now exists, verify its canonical form is still within workspace
    if let Ok(canonical) = path.canonicalize() {
        if !canonical.starts_with(&canonical_workspace) {
            log_audit_event(
                AuditCategory::PathSecurity,
                AuditSeverity::Critical,
                "toctou_escape",
                &format!(
                    "Path moved outside workspace between validation and use: '{}' -> '{}'",
                    path.display(),
                    canonical.display()
                ),
                true,
            );
            return Err(ZeptoError::SecurityViolation(format!(
                "Path escaped workspace between validation and use: '{}' resolves to '{}'",
                path.display(),
                canonical.display()
            )));
        }
    }

    Ok(())
}

/// Securely ensure a directory chain exists within the workspace.
///
/// This creates missing directories one component at a time, re-checking the
/// workspace boundary and rejecting symlinked or non-directory ancestors.
pub fn ensure_directory_chain_secure(path: &Path, workspace: &str) -> Result<()> {
    let workspace_path = Path::new(workspace);
    let canonical_workspace = workspace_path
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(workspace_path));
    let normalized_path = normalize_path(path);

    if !normalized_path.starts_with(&canonical_workspace) {
        return Err(ZeptoError::SecurityViolation(format!(
            "Directory path escapes workspace: '{}' is not within '{}'",
            normalized_path.display(),
            canonical_workspace.display()
        )));
    }

    let relative = normalized_path
        .strip_prefix(&canonical_workspace)
        .map_err(|_| {
            ZeptoError::SecurityViolation(format!(
                "Directory path escapes workspace: '{}' is not within '{}'",
                normalized_path.display(),
                canonical_workspace.display()
            ))
        })?;

    let mut current = canonical_workspace.clone();
    for component in relative.components() {
        current.push(component);

        check_symlink_escape(&current, &canonical_workspace)?;

        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(ZeptoError::SecurityViolation(format!(
                        "Symlink escape detected while creating directory '{}'",
                        current.display()
                    )));
                }
                if !meta.is_dir() {
                    return Err(ZeptoError::SecurityViolation(format!(
                        "Cannot create directory '{}': existing path is not a directory",
                        current.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => {
                        return Err(ZeptoError::Tool(format!(
                            "Failed to create directory '{}': {}",
                            current.display(),
                            e
                        )));
                    }
                }

                let meta = std::fs::symlink_metadata(&current).map_err(|e| {
                    ZeptoError::Tool(format!(
                        "Failed to inspect directory '{}' after creation: {}",
                        current.display(),
                        e
                    ))
                })?;
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(ZeptoError::SecurityViolation(format!(
                        "Directory '{}' became unsafe during creation",
                        current.display()
                    )));
                }
            }
            Err(e) => {
                return Err(ZeptoError::Tool(format!(
                    "Failed to inspect directory '{}': {}",
                    current.display(),
                    e
                )));
            }
        }

        if let Ok(canonical) = current.canonicalize() {
            if !canonical.starts_with(&canonical_workspace) {
                return Err(ZeptoError::SecurityViolation(format!(
                    "Directory '{}' resolves outside workspace to '{}'",
                    current.display(),
                    canonical.display()
                )));
            }
        }
    }

    Ok(())
}

/// Checks if a file has multiple hard links, which could indicate it aliases
/// an inode outside the workspace trust boundary.
///
/// Call this before write operations on existing files. A file with `nlink > 1`
/// inside workspace may be a hardlink to an external inode on the same filesystem,
/// allowing writes to escape workspace boundaries.
///
/// Returns Ok(()) if the file doesn't exist (new file creation) or has exactly 1 link.
pub fn check_hardlink_write(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    match std::fs::metadata(path) {
        Ok(meta) => {
            if meta.nlink() > 1 {
                log_audit_event(
                    AuditCategory::PathSecurity,
                    AuditSeverity::Critical,
                    "hardlink_escape",
                    &format!(
                        "File has {} hard links, may alias external inode: '{}'",
                        meta.nlink(),
                        path.display()
                    ),
                    true,
                );
                return Err(ZeptoError::SecurityViolation(format!(
                    "Write blocked: '{}' has {} hard links and may alias content outside workspace",
                    path.display(),
                    meta.nlink()
                )));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // File doesn't exist yet — new file creation is fine
            Ok(())
        }
        Err(e) => Err(ZeptoError::Tool(format!(
            "Failed to check file metadata for '{}': {}",
            path.display(),
            e
        ))),
    }
}

/// Normalizes a path by resolving `.` and `..` components.
///
/// This function processes path components to remove:
/// - `.` (current directory) components
/// - `..` (parent directory) components by popping from the normalized path
///
/// If the resulting path exists on the filesystem, it returns the canonical path.
/// Otherwise, it returns the normalized path.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop the last component if possible
                normalized.pop();
            }
            Component::CurDir => {
                // Skip current directory components
            }
            _ => {
                // Push all other components (Normal, RootDir, Prefix)
                normalized.push(component);
            }
        }
    }

    // Try to canonicalize if the path exists
    normalized.canonicalize().unwrap_or(normalized)
}

/// Checks if a path string contains common traversal patterns.
///
/// This provides an early detection of obvious traversal attempts
/// before more expensive path normalization.
fn contains_traversal_pattern(path: &str) -> bool {
    // Check for common traversal patterns
    let patterns = [
        "..",         // Parent directory
        "%2e%2e",     // URL encoded ..
        "%252e%252e", // Double URL encoded ..
        "..%2f",      // Mixed encoding
        "%2f..",      // Mixed encoding
        "..\\",       // Windows style
        "\\..\\",     // Windows style with prefix
    ];

    let lower_path = path.to_lowercase();
    patterns.iter().any(|p| lower_path.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn test_valid_relative_path() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a subdirectory
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}").unwrap();

        let result = validate_path_in_workspace("src/main.rs", workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_valid_absolute_path_in_workspace() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a file
        fs::write(temp.path().join("file.txt"), "content").unwrap();

        let absolute_path = temp.path().join("file.txt");
        let result = validate_path_in_workspace(absolute_path.to_str().unwrap(), workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_traversal_with_double_dots() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        let result = validate_path_in_workspace("../../../etc/passwd", workspace);
        assert!(result.is_err());

        if let Err(ZeptoError::SecurityViolation(msg)) = result {
            assert!(msg.contains("traversal pattern") || msg.contains("escapes workspace"));
        } else {
            panic!("Expected SecurityViolation error");
        }
    }

    #[test]
    fn test_traversal_with_encoded_dots() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        let result = validate_path_in_workspace("%2e%2e/etc/passwd", workspace);
        assert!(result.is_err());
    }

    #[test]
    fn test_traversal_with_mixed_encoding() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        let result = validate_path_in_workspace("..%2f../etc/passwd", workspace);
        assert!(result.is_err());
    }

    #[test]
    fn test_absolute_path_outside_workspace() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        let result = validate_path_in_workspace("/etc/passwd", workspace);
        assert!(result.is_err());

        if let Err(ZeptoError::SecurityViolation(msg)) = result {
            assert!(msg.contains("escapes workspace"));
        } else {
            panic!("Expected SecurityViolation error");
        }
    }

    #[test]
    fn test_nested_traversal() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create nested directory
        fs::create_dir_all(temp.path().join("a/b/c")).unwrap();

        let result = validate_path_in_workspace("a/b/c/../../../../etc/passwd", workspace);
        assert!(result.is_err());
    }

    #[test]
    fn test_current_directory_reference() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a file
        fs::write(temp.path().join("file.txt"), "content").unwrap();

        // ./file.txt should be valid
        let result = validate_path_in_workspace("./file.txt", workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_complex_valid_path() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create nested structure
        fs::create_dir_all(temp.path().join("src/lib")).unwrap();
        fs::write(temp.path().join("src/lib/mod.rs"), "// module").unwrap();

        // This path has . but stays within workspace
        let result = validate_path_in_workspace("src/./lib/mod.rs", workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_safe_path_conversion() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        fs::write(temp.path().join("test.txt"), "content").unwrap();

        let safe_path = validate_path_in_workspace("test.txt", workspace).unwrap();

        // Test as_path
        assert!(safe_path.as_path().ends_with("test.txt"));

        // Test into_path_buf
        let path_buf = safe_path.clone().into_path_buf();
        assert!(path_buf.ends_with("test.txt"));

        // Test AsRef<Path>
        let path_ref: &Path = safe_path.as_ref();
        assert!(path_ref.ends_with("test.txt"));
    }

    #[test]
    fn test_windows_style_traversal() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        let result = validate_path_in_workspace("..\\..\\etc\\passwd", workspace);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_path() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Empty path should resolve to workspace itself, which is valid
        let result = validate_path_in_workspace("", workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_normalize_path_basic() {
        let path = Path::new("/a/b/../c/./d");
        let normalized = normalize_path(path);

        // Should normalize to /a/c/d
        let components: Vec<_> = normalized.components().collect();
        assert!(components
            .iter()
            .any(|c| matches!(c, Component::Normal(s) if s.to_str() == Some("a"))));
        assert!(components
            .iter()
            .any(|c| matches!(c, Component::Normal(s) if s.to_str() == Some("c"))));
        assert!(components
            .iter()
            .any(|c| matches!(c, Component::Normal(s) if s.to_str() == Some("d"))));
    }

    // ==================== SYMLINK ESCAPE TESTS (NEW) ====================

    #[test]
    fn test_symlink_escape_to_outside() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a symlink inside workspace pointing outside
        let symlink_path = temp.path().join("escape_link");
        symlink(outside.path(), &symlink_path).unwrap();

        // Attempting to write through the symlink should fail
        let result = validate_path_in_workspace("escape_link/secret.txt", workspace);
        assert!(result.is_err());

        if let Err(ZeptoError::SecurityViolation(msg)) = result {
            assert!(
                msg.contains("Symlink escape") || msg.contains("escapes workspace"),
                "Expected symlink escape error, got: {}",
                msg
            );
        } else {
            panic!("Expected SecurityViolation error");
        }
    }

    #[test]
    fn test_symlink_within_workspace_allowed() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a directory and file inside workspace
        fs::create_dir_all(temp.path().join("real_dir")).unwrap();
        fs::write(temp.path().join("real_dir/file.txt"), "content").unwrap();

        // Create a symlink inside workspace pointing to another location inside workspace
        let symlink_path = temp.path().join("link_to_real");
        symlink(temp.path().join("real_dir"), &symlink_path).unwrap();

        // This should be allowed - symlink stays within workspace
        let result = validate_path_in_workspace("link_to_real/file.txt", workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_nested_symlink_escape() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a/b/c where b is a symlink to outside
        fs::create_dir_all(temp.path().join("a")).unwrap();
        symlink(outside.path(), temp.path().join("a/b")).unwrap();

        // Attempting to access a/b/anything should fail
        let result = validate_path_in_workspace("a/b/secret.txt", workspace);
        assert!(result.is_err());
    }

    #[test]
    fn test_symlink_to_parent_blocked() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create a symlink pointing to parent directory (escape attempt)
        let symlink_path = temp.path().join("parent_link");
        if let Some(parent) = temp.path().parent() {
            symlink(parent, &symlink_path).unwrap();

            let result = validate_path_in_workspace("parent_link/etc/passwd", workspace);
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_new_file_in_symlinked_dir_blocked() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let workspace = temp.path().to_str().unwrap();

        // Create symlink to outside directory
        let symlink_path = temp.path().join("linked_dir");
        symlink(outside.path(), &symlink_path).unwrap();

        // Try to create a NEW file in the symlinked directory
        // This is the exact attack vector from the security finding
        let result = validate_path_in_workspace("linked_dir/new_file.txt", workspace);
        assert!(
            result.is_err(),
            "Should block writing new files through symlinks to outside"
        );
    }

    // ==================== DANGLING SYMLINK TESTS ====================

    #[test]
    fn test_dangling_symlink_rejected() {
        let temp = tempdir().unwrap();
        // Use canonical workspace to avoid macOS /var -> /private/var mismatch
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a symlink pointing to a non-existent target inside workspace
        // (target within workspace namespace so starts_with doesn't mask the check)
        let nonexistent_target = canonical.join("does_not_exist_subdir");
        let symlink_path = canonical.join("dangling_link");
        symlink(&nonexistent_target, &symlink_path).unwrap();

        // Dangling symlink should be rejected — target can't be validated
        let result = validate_path_in_workspace("dangling_link/file.txt", workspace);
        assert!(
            result.is_err(),
            "Should reject dangling symlinks whose target can't be verified"
        );

        if let Err(ZeptoError::SecurityViolation(msg)) = result {
            assert!(
                msg.contains("Dangling symlink") || msg.contains("cannot be validated"),
                "Expected dangling symlink error, got: {}",
                msg
            );
        }
    }

    #[test]
    fn test_dangling_symlink_to_outside_workspace() {
        let temp = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a symlink that points outside workspace to a path that doesn't exist
        let symlink_path = canonical.join("future_escape");
        symlink("/tmp/attacker_controlled_dir_nonexistent", &symlink_path).unwrap();

        let result = validate_path_in_workspace("future_escape/secret.txt", workspace);
        assert!(
            result.is_err(),
            "Should reject dangling symlink pointing outside workspace"
        );
    }

    #[test]
    fn test_nested_dangling_symlink() {
        let temp = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a/dangling where dangling is a broken symlink within workspace namespace
        fs::create_dir_all(canonical.join("a")).unwrap();
        let nonexistent_target = canonical.join("no_such_dir");
        symlink(&nonexistent_target, canonical.join("a/dangling")).unwrap();

        let result = validate_path_in_workspace("a/dangling/file.txt", workspace);
        assert!(result.is_err(), "Should reject nested dangling symlinks");
    }

    #[test]
    fn test_dangling_symlink_direct_access() {
        let temp = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a dangling symlink pointing to non-existent path within workspace
        let nonexistent_target = canonical.join("ghost");
        let symlink_path = canonical.join("broken_link");
        symlink(&nonexistent_target, &symlink_path).unwrap();

        // Accessing the symlink itself (not a child) — the symlink is the leaf
        let result = validate_path_in_workspace("broken_link", workspace);
        assert!(
            result.is_err(),
            "Should reject direct access to dangling symlink"
        );
    }

    // ==================== REVALIDATE_PATH TESTS ====================

    #[test]
    fn test_revalidate_path_valid_file() {
        let temp = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        let file = canonical.join("safe.txt");
        fs::write(&file, "content").unwrap();

        // Revalidation should pass for a normal file
        let result = revalidate_path(&file, workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_revalidate_path_nonexistent_file() {
        let temp = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Non-existent file — new file creation is fine
        let file = canonical.join("new_file.txt");
        let result = revalidate_path(&file, workspace);
        assert!(result.is_ok());
    }

    #[test]
    fn test_revalidate_path_symlink_escape() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        // Create a symlink pointing outside workspace
        let escape = canonical.join("escape");
        symlink(outside.path(), &escape).unwrap();

        let target = escape.join("secret.txt");
        let result = revalidate_path(&target, workspace);
        assert!(
            result.is_err(),
            "Should detect symlink escape on revalidation"
        );
    }

    #[test]
    fn test_ensure_directory_chain_secure_creates_nested_dirs() {
        let temp = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();
        let nested = canonical.join("a/b/c");

        let result = ensure_directory_chain_secure(&nested, workspace);
        assert!(result.is_ok());
        assert!(nested.is_dir());
    }

    #[test]
    fn test_ensure_directory_chain_secure_rejects_symlink_parent() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let workspace = canonical.to_str().unwrap();

        let linked = canonical.join("linked");
        symlink(outside.path(), &linked).unwrap();

        let result = ensure_directory_chain_secure(&linked.join("child"), workspace);
        assert!(result.is_err());
    }

    // ==================== CHECK_HARDLINK_WRITE TESTS ====================

    #[test]
    fn test_hardlink_write_single_link() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("single.txt");
        fs::write(&file, "content").unwrap();

        // Single link (nlink=1) should be allowed
        let result = check_hardlink_write(&file);
        assert!(result.is_ok());
    }

    #[test]
    fn test_hardlink_write_multiple_links() {
        let temp = tempdir().unwrap();
        let original = temp.path().join("original.txt");
        fs::write(&original, "content").unwrap();

        let link = temp.path().join("hardlink.txt");
        fs::hard_link(&original, &link).unwrap();

        // nlink=2 should be blocked
        let result = check_hardlink_write(&link);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hard links"),
            "Expected hardlink error, got: {}",
            err
        );

        // Original also has nlink=2 now
        let result = check_hardlink_write(&original);
        assert!(result.is_err());
    }

    #[test]
    fn test_hardlink_write_nonexistent_file() {
        let temp = tempdir().unwrap();
        let nonexistent = temp.path().join("does_not_exist.txt");

        // Non-existent file — new file creation is fine
        let result = check_hardlink_write(&nonexistent);
        assert!(result.is_ok());
    }
}
