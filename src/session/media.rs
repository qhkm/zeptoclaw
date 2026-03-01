//! Media storage for disk-based image persistence.
//!
//! This module provides the [`MediaStore`] struct for saving and loading image
//! data to disk. Images are named by the first 16 hex characters of their
//! SHA-256 hash, which provides automatic deduplication — identical content
//! is written only once.
//!
//! # Layout
//!
//! ```text
//! {base_dir}/
//! └── media/
//!     ├── a1b2c3d4e5f6g7h8.jpg
//!     └── deadbeef01234567.png
//! ```
//!
//! Session JSON files store the relative path (`"media/a1b2c3d4e5f6g7h8.jpg"`)
//! rather than embedding base64, keeping session files small.

use crate::error::{Result, ZeptoError};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

// ============================================================================
// Constants
// ============================================================================

/// Maximum accepted image size (20 MiB).
pub const MAX_IMAGE_SIZE: usize = 20 * 1024 * 1024;

/// MIME types accepted by [`validate_image`].
pub const SUPPORTED_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

// ============================================================================
// MediaStore
// ============================================================================

/// Disk-based store for image files.
///
/// Images are saved under `{base_dir}/media/` and named by a 16-character
/// hex prefix of their SHA-256 digest. Duplicate content is detected by
/// hash and skipped on write.
///
/// # Example
///
/// ```no_run
/// use zeptoclaw::session::media::MediaStore;
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() {
///     let store = MediaStore::new(PathBuf::from("/tmp/sessions"));
///     let path = store.save(b"...", "image/jpeg").await.unwrap();
///     // path == "media/<hash>.jpg"
///     let bytes = store.load(&path).await.unwrap();
/// }
/// ```
pub struct MediaStore {
    base_dir: PathBuf,
}

impl MediaStore {
    /// Create a new `MediaStore` rooted at `base_dir`.
    ///
    /// The `media/` subdirectory is created lazily on first write.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Save image `data` to disk and return its relative path.
    ///
    /// The path has the form `"media/<16-char-hash>.<ext>"`.
    /// If an identical file already exists (same hash), the write is skipped
    /// and the existing path is returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error if the `media/` directory cannot be created or the
    /// file cannot be written.
    pub async fn save(&self, data: &[u8], mime_type: &str) -> Result<String> {
        let ext = mime_to_ext(mime_type);
        let hash = sha256_prefix(data);
        let filename = format!("{}.{}", hash, ext);
        let rel_path = format!("media/{}", filename);
        let abs_path = self.base_dir.join(&rel_path);

        // Create the media/ directory if it does not exist yet.
        let media_dir = self.base_dir.join("media");
        fs::create_dir_all(&media_dir).await?;

        // Skip write if the file already exists (deduplication).
        if abs_path.exists() {
            return Ok(rel_path);
        }

        fs::write(&abs_path, data).await?;
        Ok(rel_path)
    }

    /// Load image bytes from a relative path previously returned by [`save`].
    ///
    /// # Errors
    ///
    /// Returns an error if the file does not exist or cannot be read.
    pub async fn load(&self, rel_path: &str) -> Result<Vec<u8>> {
        let abs_path = self.base_dir.join(rel_path);
        let bytes = fs::read(&abs_path).await?;
        Ok(bytes)
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Map a MIME type string to a file extension.
///
/// Returns `"bin"` for any unrecognised type.
pub fn mime_to_ext(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Validate that `data` is within `max_size` bytes and that `mime_type` is
/// one of the [`SUPPORTED_TYPES`].
///
/// # Errors
///
/// Returns [`ZeptoError::Tool`] with a descriptive message if either check
/// fails.
pub fn validate_image(data: &[u8], mime_type: &str, max_size: usize) -> Result<()> {
    if data.len() > max_size {
        return Err(ZeptoError::Tool(format!(
            "Image size {} bytes exceeds the maximum allowed size of {} bytes",
            data.len(),
            max_size
        )));
    }

    if !SUPPORTED_TYPES.contains(&mime_type) {
        return Err(ZeptoError::Tool(format!(
            "Unsupported image type '{}'. Supported types: {}",
            mime_type,
            SUPPORTED_TYPES.join(", ")
        )));
    }

    Ok(())
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Compute SHA-256 of `data` and return the first 16 hex characters.
fn sha256_prefix(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    // hex-encode the full digest then take the first 16 chars (8 bytes).
    hex::encode(digest)[..16].to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_save_media_creates_file_with_hash_name() {
        let tmp = TempDir::new().unwrap();
        let store = MediaStore::new(tmp.path().to_path_buf());
        let data = b"fake jpeg data";
        let path = store.save(data, "image/jpeg").await.unwrap();
        assert!(path.starts_with("media/"));
        assert!(path.ends_with(".jpg"));
        assert!(tmp.path().join(&path).exists());
    }

    #[tokio::test]
    async fn test_save_media_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let store = MediaStore::new(tmp.path().to_path_buf());
        let data = b"same data";
        let path1 = store.save(data, "image/png").await.unwrap();
        let path2 = store.save(data, "image/png").await.unwrap();
        assert_eq!(path1, path2);
    }

    #[tokio::test]
    async fn test_load_media_returns_bytes() {
        let tmp = TempDir::new().unwrap();
        let store = MediaStore::new(tmp.path().to_path_buf());
        let data = b"image bytes here";
        let path = store.save(data, "image/jpeg").await.unwrap();
        let loaded = store.load(&path).await.unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn test_mime_to_extension() {
        assert_eq!(mime_to_ext("image/jpeg"), "jpg");
        assert_eq!(mime_to_ext("image/png"), "png");
        assert_eq!(mime_to_ext("image/gif"), "gif");
        assert_eq!(mime_to_ext("image/webp"), "webp");
        assert_eq!(mime_to_ext("image/unknown"), "bin");
    }

    #[test]
    fn test_validate_image_valid_types() {
        assert!(validate_image(b"data", "image/jpeg", MAX_IMAGE_SIZE).is_ok());
        assert!(validate_image(b"data", "image/png", MAX_IMAGE_SIZE).is_ok());
        assert!(validate_image(b"data", "image/gif", MAX_IMAGE_SIZE).is_ok());
        assert!(validate_image(b"data", "image/webp", MAX_IMAGE_SIZE).is_ok());
    }

    #[test]
    fn test_validate_image_rejects_unsupported() {
        assert!(validate_image(b"data", "image/tiff", MAX_IMAGE_SIZE).is_err());
        assert!(validate_image(b"data", "application/pdf", MAX_IMAGE_SIZE).is_err());
    }

    #[test]
    fn test_validate_image_rejects_oversized() {
        let big = vec![0u8; 21 * 1024 * 1024];
        assert!(validate_image(&big, "image/jpeg", MAX_IMAGE_SIZE).is_err());
    }
}
