//! Spill storage: write oversized tool output to private session-scoped
//! files and keep the directory within its retention caps.

pub mod policy;

pub use policy::{format_with_locator, needs_spill, preview, SpillRef};

use std::path::PathBuf;

use crate::config::SpillConfig;
use crate::utils::secure_fs::{ensure_private_dir, write_private_file};

/// Writes spill payloads for one session.
pub struct SpillStore {
    root: PathBuf,
}

impl SpillStore {
    /// Store rooted at an explicit directory. Callers resolve the path —
    /// see `SpillStore::session_root` (Task 7) for the canonical location.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Persist `text` and return its locator. Enforces retention afterwards.
    pub fn write(&self, tool: &str, text: &str, cfg: &SpillConfig) -> std::io::Result<SpillRef> {
        ensure_private_dir(&self.root)?;
        let seq = self.next_seq()?;
        let path = self.root.join(format!("{seq:03}-{tool}.txt"));
        write_private_file(&path, text.as_bytes())?;

        let spilled = SpillRef {
            path: path.clone(),
            total_lines: text.split_inclusive('\n').count(),
            total_bytes: text.len(),
        };

        // Retention failures must not invalidate a payload already on disk.
        if let Err(e) = self.enforce_retention(cfg, &path) {
            tracing::warn!(error = %e, "spill retention sweep failed");
        }
        Ok(spilled)
    }

    /// One past the highest sequence currently on disk.
    fn next_seq(&self) -> std::io::Result<usize> {
        let mut max = 0;
        for entry in std::fs::read_dir(&self.root)? {
            let name = entry?.file_name();
            if let Some(n) = name
                .to_str()
                .and_then(|s| s.split('-').next())
                .and_then(|s| s.parse::<usize>().ok())
            {
                max = max.max(n);
            }
        }
        Ok(max + 1)
    }

    /// Evict oldest-first until both caps hold. `keep` is never evicted:
    /// dropping everything to admit one oversized payload is worse than
    /// exceeding the cap once.
    fn enforce_retention(&self, cfg: &SpillConfig, keep: &std::path::Path) -> std::io::Result<()> {
        let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(&self.root)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.metadata().ok().map(|m| (e.path(), m.len())))
            .collect();
        files.sort();

        let mut total: u64 = files.iter().map(|(_, len)| *len).sum();
        let mut count = files.len();

        for (path, len) in files {
            if path == keep {
                continue;
            }
            if count <= cfg.max_files_per_session && total <= cfg.session_max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                count -= 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpillConfig;

    fn store(dir: &tempfile::TempDir) -> SpillStore {
        SpillStore::with_root(dir.path().to_path_buf())
    }

    #[test]
    fn write_persists_full_text_and_reports_counts() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();

        let r = store(&dir)
            .write("shell", &body, &SpillConfig::default())
            .unwrap();

        assert_eq!(std::fs::read_to_string(&r.path).unwrap(), body);
        assert_eq!(r.total_lines, 10);
        assert_eq!(r.total_bytes, body.len());
    }

    #[test]
    fn sequence_numbers_increment() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let a = s.write("shell", "a\n", &SpillConfig::default()).unwrap();
        let b = s.write("grep", "b\n", &SpillConfig::default()).unwrap();
        assert!(a
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("001-shell"));
        assert!(b
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("002-grep"));
    }

    #[test]
    fn file_count_cap_evicts_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let cfg = SpillConfig {
            max_files_per_session: 2,
            ..Default::default()
        };

        let first = s.write("shell", "a\n", &cfg).unwrap();
        s.write("shell", "b\n", &cfg).unwrap();
        s.write("shell", "c\n", &cfg).unwrap();

        assert!(!first.path.exists(), "oldest should be evicted");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn byte_cap_evicts_oldest_until_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let cfg = SpillConfig {
            session_max_bytes: 10,
            ..Default::default()
        };

        let first = s.write("shell", "aaaaa\n", &cfg).unwrap();
        s.write("shell", "bbbbb\n", &cfg).unwrap();

        assert!(!first.path.exists());
    }

    #[test]
    fn single_oversized_payload_is_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = SpillConfig {
            session_max_bytes: 4,
            ..Default::default()
        };

        let r = store(&dir).write("shell", "aaaaaaaaaa\n", &cfg).unwrap();

        assert!(
            r.path.exists(),
            "evicting everything to fit one file is worse"
        );
    }

    #[test]
    fn unwritable_root_returns_err() {
        let s = SpillStore::with_root(std::path::PathBuf::from("/proc/nonexistent/spill"));
        assert!(s.write("shell", "a\n", &SpillConfig::default()).is_err());
    }
}
