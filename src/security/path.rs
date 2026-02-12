//! Path validation utilities for secure file operations
//!
//! This module provides path validation to prevent directory traversal attacks
//! and symlink-based workspace escapes.

use std::path::{Component, Path, PathBuf};

use crate::error::{PicoError, Result};

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
/// * `Err(PicoError::SecurityViolation)` - If the path escapes the workspace
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
        return Err(PicoError::SecurityViolation(format!(
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
        return Err(PicoError::SecurityViolation(format!(
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

        // Only check components that exist on the filesystem
        if current.exists() {
            // Canonicalize to resolve any symlinks
            if let Ok(canonical) = current.canonicalize() {
                // Check if the canonical path is still within workspace
                if !canonical.starts_with(canonical_workspace) {
                    return Err(PicoError::SecurityViolation(format!(
                        "Symlink escape detected: '{}' resolves to '{}' which is outside workspace",
                        current.display(),
                        canonical.display()
                    )));
                }
            }
        }
    }

    Ok(())
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

        if let Err(PicoError::SecurityViolation(msg)) = result {
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

        if let Err(PicoError::SecurityViolation(msg)) = result {
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

        if let Err(PicoError::SecurityViolation(msg)) = result {
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
}
