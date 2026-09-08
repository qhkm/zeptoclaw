//! Filesystem helpers for secret-bearing files and directories.

use std::fs;
use std::io;
use std::path::Path;

/// Create `path` if needed and restrict it to the current user on Unix.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }

    Ok(())
}

/// Restrict an existing file to the current user on Unix.
pub fn restrict_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    let _ = path;

    Ok(())
}

/// Write a secret-bearing file with user-only permissions on Unix.
///
/// The creation mode protects new files immediately, while the explicit
/// permission update also repairs files created by older versions.
pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        if path.exists() {
            restrict_private_file(path)?;
        }

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents)?;
        restrict_private_file(path)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, contents)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_directory_is_user_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");

        ensure_private_dir(&path).unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn private_file_creation_and_rewrite_are_user_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("secret");

        write_private_file(&path, b"first").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&path, b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
