//! Spill storage: write oversized tool output to private session-scoped
//! files and keep the directory within its retention caps.

pub mod policy;

pub use policy::{
    format_with_locator, needs_spill, preview, sanitize_preserving_locator, split_locator, SpillRef,
};

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::config::{Config, SpillConfig};
use crate::utils::secure_fs::{ensure_private_dir, write_private_file_exclusive};

/// Bound on retries when writers race for a sequence number. Contenders
/// that all observed the same starting point resolve in a strict
/// tournament — each collision eliminates exactly one contender per
/// attempt — so this only needs to cover the worst realistic number of
/// tool calls racing in parallel, not be unbounded.
const MAX_SEQ_ATTEMPTS: usize = 32;

/// Writes spill payloads for one session.
pub struct SpillStore {
    root: PathBuf,
}

impl SpillStore {
    /// Store rooted at an explicit directory. Callers resolve the path —
    /// see `SpillStore::session_root` for the canonical location.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Canonical spill directory for a session.
    ///
    /// `session_id` is sanitized with
    /// [`crate::session::sanitize_session_key`] before it becomes a path
    /// component, for the same reason `sanitize_tool_name` exists below: the
    /// value is externally sourced. A session key is `"{channel}:{chat_id}"`,
    /// and `WebhookChannel` takes `chat_id` straight from the POST body when
    /// `trust_payload_identity` is on — so an unsanitized `chat_id` of
    /// `../../../../tmp/x` would have `ensure_private_dir` create that
    /// directory and write tool output into it.
    ///
    /// Reusing the session layer's encoder rather than inventing one keeps
    /// the writer and [`SpillStore::remove_session`] in agreement, and keeps
    /// a session's spill directory named from the same key, the same way, as
    /// its session file.
    pub fn session_root(session_id: &str) -> PathBuf {
        Self::session_root_in(&Config::dir(), session_id)
    }

    /// [`SpillStore::session_root`] against an explicit base directory.
    ///
    /// Exists so the key-to-path agreement can be exercised in a temp dir
    /// rather than against the caller's real home directory.
    pub fn session_root_in(base: &Path, session_id: &str) -> PathBuf {
        base.join("sessions")
            .join(crate::session::sanitize_session_key(session_id))
            .join("spill")
    }

    /// Remove a spill directory and everything in it. Missing is not an error.
    pub fn remove_root(root: &Path) -> std::io::Result<()> {
        match std::fs::remove_dir_all(root) {
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Remove the spill directory belonging to a session.
    pub fn remove_session(session_id: &str) -> std::io::Result<()> {
        Self::remove_root(&Self::session_root(session_id))
    }

    /// [`SpillStore::remove_session`] against an explicit base directory.
    pub fn remove_session_in(base: &Path, session_id: &str) -> std::io::Result<()> {
        Self::remove_root(&Self::session_root_in(base, session_id))
    }

    /// Persist `text` and return its locator. Enforces retention afterwards.
    ///
    /// `tool` is sanitized before it becomes part of the filename: MCP and
    /// plugin tool names originate outside this process, and an
    /// unsanitized `/` or `..` would let a caller escape `root`.
    ///
    /// The sequence number is allocated with an exclusive create-and-retry.
    /// `next_seq` and the write are not atomic together, and the agent loop
    /// runs tool calls in parallel, so two writers can compute the same
    /// number; retrying on `AlreadyExists` means the loser advances instead
    /// of silently truncating the winner's payload. Retries increment
    /// locally from one directory scan rather than rescanning on every
    /// attempt — a rescan lags behind concurrent writers and can hand back
    /// a number that's already been claimed again, wasting attempts.
    pub fn write(&self, tool: &str, text: &str, cfg: &SpillConfig) -> std::io::Result<SpillRef> {
        ensure_private_dir(&self.root)?;
        let tool = sanitize_tool_name(tool);

        let base = self.next_seq()?;
        let mut written = None;
        for offset in 0..MAX_SEQ_ATTEMPTS {
            let candidate = self.root.join(format!("{:03}-{tool}.txt", base + offset));
            match write_private_file_exclusive(&candidate, text.as_bytes()) {
                Ok(()) => {
                    written = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        let path = written.ok_or_else(|| {
            std::io::Error::other("spill: exhausted retries allocating a sequence number")
        })?;

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

    /// This store's own spill files as `(sequence, path, byte length)`.
    /// Filtered to plain files matching this store's naming
    /// (`{seq}-{tool}.txt`) — anything else living in `root` (a manifest, a
    /// lock file, a subdirectory) is neither counted toward the retention
    /// caps nor eligible for eviction.
    fn own_files(&self) -> std::io::Result<Vec<(usize, PathBuf, u64)>> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let Some(seq) = parse_seq(&entry.file_name()) else {
                continue;
            };
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((seq, entry.path(), len));
        }
        Ok(files)
    }

    /// One past the highest sequence currently on disk (1 if empty).
    fn next_seq(&self) -> std::io::Result<usize> {
        Ok(self
            .own_files()?
            .into_iter()
            .map(|(seq, _, _)| seq)
            .max()
            .unwrap_or(0)
            + 1)
    }

    /// Evict oldest-first, ordered by the parsed sequence number rather than
    /// the path: `{seq:03}` padding is cosmetic, so once the sequence rolls
    /// over from 3 digits to 4, a lexicographic path sort would place the
    /// newest file before older 3-digit ones and evict it instead — starving
    /// retention on every subsequent write. `keep` is never evicted:
    /// dropping everything to admit one oversized payload is worse than
    /// exceeding the cap once.
    fn enforce_retention(&self, cfg: &SpillConfig, keep: &Path) -> std::io::Result<()> {
        let mut files = self.own_files()?;
        files.sort_by_key(|(seq, _, _)| *seq);

        let mut total: u64 = files.iter().map(|(_, _, len)| *len).sum();
        let mut count = files.len();

        for (_, path, len) in files {
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

/// Sanitize a tool name for use in a spill filename: anything outside
/// `[A-Za-z0-9_-]` becomes `_`. `tool` comes from MCP/plugin registries
/// outside this process, so `write` can't assume it's already a safe path
/// component — an unsanitized `/` or `..` would escape `root`, and an
/// unsanitized non-ASCII byte could be normalized differently by the
/// filesystem than the `PathBuf` this method builds, breaking the `keep`
/// comparison in `enforce_retention` and letting the just-written payload be
/// evicted.
fn sanitize_tool_name(tool: &str) -> String {
    tool.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Parse the sequence number from a spill filename (`{seq}-{tool}.txt`).
/// This parses any digit width — the zero-padding `write` applies is
/// cosmetic — so ordering by the returned number, not the raw filename,
/// stays chronological past 999 entries.
fn parse_seq(file_name: &std::ffi::OsStr) -> Option<usize> {
    let name = file_name.to_str()?;
    let name = name.strip_suffix(".txt")?;
    name.split('-').next()?.parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpillConfig;

    fn store(dir: &tempfile::TempDir) -> SpillStore {
        SpillStore::with_root(dir.path().to_path_buf())
    }

    #[test]
    fn session_root_is_under_the_session_directory() {
        let root = SpillStore::session_root("abc123");
        assert!(
            root.ends_with("sessions/abc123/spill"),
            "got {}",
            root.display()
        );
    }

    // Regression: IMPORTANT #3 — a session key is "{channel}:{chat_id}" and
    // `WebhookChannel` accepts a caller-supplied `chat_id` from the POST body
    // when `trust_payload_identity` is on. An unsanitized key is therefore an
    // attacker-controlled path fragment. Mirrors
    // `tool_name_is_sanitized_to_prevent_path_escape`.
    #[test]
    fn session_id_is_sanitized_to_prevent_path_escape() {
        let base = tempfile::tempdir().unwrap();
        let sessions = base.path().join("sessions");

        let root = SpillStore::session_root_in(base.path(), "webhook:../../../../tmp/x");

        assert_eq!(
            root.parent().and_then(|p| p.parent()),
            Some(sessions.as_path()),
            "spill root must stay one level under sessions/: {}",
            root.display()
        );
        // `..` may survive as literal characters inside the component --
        // what matters is that the separators around it are encoded, so it
        // cannot split into traversing components.
        let component = root
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            !component.contains('/') && !component.contains('\\'),
            "sanitized component must not contain a separator: {component}"
        );
        assert_eq!(
            std::path::Path::new(component).components().count(),
            1,
            "sanitized key must be exactly one path component: {component}"
        );

        // And the directory that actually gets created is inside it.
        SpillStore::with_root(root.clone())
            .write("shell", "payload\n", &SpillConfig::default())
            .unwrap();
        assert!(root.is_dir());
        assert!(!base.path().join("tmp").exists());
        assert!(!std::path::Path::new("/tmp/x/spill").exists());
    }

    #[test]
    fn session_id_with_leading_dots_cannot_name_a_parent_directory() {
        let base = tempfile::tempdir().unwrap();
        for key in ["..", ".", "../..", ".hidden"] {
            let root = SpillStore::session_root_in(base.path(), key);
            let component = root
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                component != "." && component != "..",
                "key {key:?} produced component {component:?}"
            );
        }
    }

    // The writer and the remover must agree on the encoding, or deletion
    // silently misses and spill files outlive their session.
    #[test]
    fn remove_session_finds_the_directory_the_writer_created() {
        let base = tempfile::tempdir().unwrap();
        let key = "telegram:chat 123:weird%/path";

        let root = SpillStore::session_root_in(base.path(), key);
        SpillStore::with_root(root.clone())
            .write("shell", "payload\n", &SpillConfig::default())
            .unwrap();
        assert!(root.is_dir());

        SpillStore::remove_session_in(base.path(), key).unwrap();
        assert!(!root.exists(), "remove_session must reach the same path");
    }

    // The spill directory and the session file are named from the same key
    // the same way, so `cleanup_old`'s returned keys resolve to both.
    #[test]
    fn session_root_uses_the_session_layers_encoding() {
        let base = tempfile::tempdir().unwrap();
        let key = "cli:cli";
        let root = SpillStore::session_root_in(base.path(), key);
        assert_eq!(
            root.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            crate::session::sanitize_session_key(key)
        );
        assert_eq!(
            root.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            "cli%3Acli"
        );
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

    // Regression: CRITICAL #1 — eviction must order by parsed sequence
    // number, not by lexicographic path. Once the sequence rolls over from
    // 3 digits to 4, `"1000-..." < "980-..."` lexicographically (`'1' <
    // '9'`), so a path-sorted sweep would delete the file it just wrote
    // (or, past the first rollover, the newest surviving file) instead of
    // the oldest one — and keep doing so on every subsequent write.
    #[test]
    fn eviction_orders_by_parsed_sequence_not_lexicographic_path() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let cfg = SpillConfig {
            max_files_per_session: 2,
            ..Default::default()
        };

        let seeded = dir.path().join("999-shell.txt");
        std::fs::write(&seeded, "seed\n").unwrap();

        let first = s.write("shell", "a\n", &cfg).unwrap(); // seq 1000, no eviction yet (count == cap)
        assert!(first
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("1000-"));
        assert!(seeded.exists());

        let second = s.write("shell", "b\n", &cfg).unwrap(); // seq 1001, triggers eviction
        assert!(second
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("1001-"));

        assert!(
            !seeded.exists(),
            "oldest (999) should be evicted, not the newest"
        );
        assert!(
            first.path.exists(),
            "1000 is newer than 999 and must survive"
        );
        assert!(second.path.exists());
    }

    // Regression: IMPORTANT #2 — files that aren't this store's own spill
    // files (no parseable `{seq}-...txt` name) must never be counted toward
    // the retention caps or evicted. Before the fix, a subdirectory counted
    // toward `count`/`total` but `remove_file` on it always failed, so the
    // sweep compensated by deleting one extra real spill file on every call.
    #[test]
    fn unrelated_files_and_subdirectories_are_not_evicted_or_counted() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let cfg = SpillConfig {
            max_files_per_session: 1,
            ..Default::default()
        };

        let manifest = dir.path().join("manifest.json");
        std::fs::write(&manifest, "{}").unwrap();
        let sub = dir.path().join("nested");
        std::fs::create_dir(&sub).unwrap();

        let first = s.write("shell", "a\n", &cfg).unwrap();
        let second = s.write("shell", "b\n", &cfg).unwrap();

        assert!(manifest.exists(), "non-spill file must survive retention");
        assert!(sub.is_dir(), "subdirectory must survive retention");
        assert!(
            !first.path.exists(),
            "cap of 1 should still evict the oldest spill file"
        );
        assert!(second.path.exists());
    }

    // Regression: IMPORTANT #3 — concurrent writers must never silently
    // clobber each other. `next_seq` + write are not atomic, so two writers
    // can compute the same sequence number; the exclusive-create retry loop
    // must make the loser advance instead of truncating the winner.
    #[test]
    fn concurrent_writes_do_not_clobber_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(SpillStore::with_root(dir.path().to_path_buf()));
        let cfg = SpillConfig {
            max_files_per_session: 10_000,
            session_max_bytes: u64::MAX,
            ..Default::default()
        };

        let handles: Vec<_> = (0..8)
            .map(|t| {
                let store = std::sync::Arc::clone(&store);
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    let mut written = Vec::new();
                    for i in 0..20 {
                        let body = format!("thread {t} write {i}\n");
                        let r = store.write("shell", &body, &cfg).unwrap();
                        written.push((r, body));
                    }
                    written
                })
            })
            .collect();

        let mut seen = std::collections::HashSet::new();
        for h in handles {
            for (r, body) in h.join().unwrap() {
                assert!(
                    seen.insert(r.path.clone()),
                    "sequence number reused: {:?}",
                    r.path
                );
                assert_eq!(std::fs::read_to_string(&r.path).unwrap(), body);
            }
        }
        assert_eq!(seen.len(), 8 * 20);
    }

    // Regression: IMPORTANT #4 — `tool` is caller-supplied (MCP/plugin
    // names) and must not be able to escape `root` via `/` or `..`.
    #[test]
    fn tool_name_is_sanitized_to_prevent_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);

        let r = s
            .write("../../etc/passwd", "a\n", &SpillConfig::default())
            .unwrap();

        assert_eq!(r.path.parent(), Some(dir.path()));
        let name = r.path.file_name().unwrap().to_str().unwrap();
        assert!(
            !name.contains('/'),
            "sanitized name must not contain a separator"
        );
        assert!(!name.contains(".."), "sanitized name must not contain '..'");
    }

    #[test]
    fn remove_session_deletes_the_spill_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("spill");
        let s = SpillStore::with_root(root.clone());
        s.write("shell", "a\n", &SpillConfig::default()).unwrap();
        assert!(root.exists());

        SpillStore::remove_root(&root).unwrap();

        assert!(!root.exists());
    }
}
