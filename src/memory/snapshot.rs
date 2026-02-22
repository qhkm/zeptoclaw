//! Memory snapshot — export/import longterm memory as JSON.
//!
//! Provides [`export_snapshot`] and [`import_snapshot`] for backup and migration.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::memory::longterm::LongTermMemory;

/// A single entry in a memory snapshot file.
#[derive(Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub key: String,
    pub value: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_importance")]
    pub importance: f32,
}

fn default_importance() -> f32 {
    1.0
}

/// Export all longterm memory entries to a JSON snapshot file.
///
/// Returns the number of entries exported. Creates parent directories if needed.
pub fn export_snapshot(memory: &LongTermMemory, path: &Path) -> Result<usize> {
    let entries = memory.list_all();

    let snapshot: Vec<SnapshotEntry> = entries
        .iter()
        .map(|entry| SnapshotEntry {
            key: entry.key.clone(),
            value: entry.value.clone(),
            category: entry.category.clone(),
            tags: entry.tags.clone(),
            importance: entry.importance,
        })
        .collect();

    let json = serde_json::to_string_pretty(&snapshot)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;

    Ok(snapshot.len())
}

/// Import entries from a JSON snapshot file into memory.
///
/// Returns `(imported_count, skipped_count)`.
/// - `overwrite = false`: existing keys are skipped.
/// - `overwrite = true`: existing keys are overwritten.
pub async fn import_snapshot(
    memory: &mut LongTermMemory,
    path: &Path,
    overwrite: bool,
) -> Result<(usize, usize)> {
    let content = std::fs::read_to_string(path)?;
    let entries: Vec<SnapshotEntry> = serde_json::from_str(&content)?;

    let mut imported = 0;
    let mut skipped = 0;

    for entry in &entries {
        if !overwrite && memory.get_readonly(&entry.key).is_some() {
            skipped += 1;
            continue;
        }

        memory
            .set(
                &entry.key,
                &entry.value,
                &entry.category,
                entry.tags.clone(),
                entry.importance,
            )
            .await?;

        imported += 1;
    }

    Ok((imported, skipped))
}

/// Default snapshot path: `~/.zeptoclaw/memory/snapshot.json`.
pub fn default_snapshot_path() -> std::path::PathBuf {
    crate::config::Config::dir()
        .join("memory")
        .join("snapshot.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_memory() -> (LongTermMemory, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("longterm.json");
        let mem = LongTermMemory::with_path(path).expect("memory");
        (mem, dir)
    }

    #[test]
    fn test_export_empty_memory() {
        let (mem, _dir) = temp_memory();
        let temp_path = std::env::temp_dir().join("zc_snap_test_empty.json");
        let count = export_snapshot(&mem, &temp_path).unwrap();
        assert_eq!(count, 0);
        let content = std::fs::read_to_string(&temp_path).unwrap();
        assert_eq!(content.trim(), "[]");
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_export_with_entries() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "v1", "user", vec![], 1.0).await.unwrap();
        mem.set("k2", "v2", "general", vec![], 1.0).await.unwrap();
        let temp_path = std::env::temp_dir().join("zc_snap_test_entries.json");
        let count = export_snapshot(&mem, &temp_path).unwrap();
        assert_eq!(count, 2);
        let content = std::fs::read_to_string(&temp_path).unwrap();
        let entries: Vec<SnapshotEntry> = serde_json::from_str(&content).unwrap();
        assert_eq!(entries.len(), 2);
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_import_merge_skips_existing() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "original", "user", vec![], 1.0)
            .await
            .unwrap();

        let temp_path = std::env::temp_dir().join("zc_snap_test_merge.json");
        let snap = serde_json::json!([
            {"key": "k1", "value": "overwritten", "category": "user"},
            {"key": "k2", "value": "new", "category": "general"}
        ]);
        std::fs::write(&temp_path, snap.to_string()).unwrap();

        let (imported, skipped) = import_snapshot(&mut mem, &temp_path, false).await.unwrap();
        assert_eq!(imported, 1); // k2
        assert_eq!(skipped, 1); // k1 skipped
        assert_eq!(mem.get_readonly("k1").unwrap().value, "original");
        assert_eq!(mem.get_readonly("k2").unwrap().value, "new");
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_import_overwrite() {
        let (mut mem, _dir) = temp_memory();
        mem.set("k1", "original", "user", vec![], 1.0)
            .await
            .unwrap();

        let temp_path = std::env::temp_dir().join("zc_snap_test_overwrite.json");
        let snap = serde_json::json!([{"key": "k1", "value": "updated", "category": "user"}]);
        std::fs::write(&temp_path, snap.to_string()).unwrap();

        let (imported, skipped) = import_snapshot(&mut mem, &temp_path, true).await.unwrap();
        assert_eq!(imported, 1);
        assert_eq!(skipped, 0);
        assert_eq!(mem.get_readonly("k1").unwrap().value, "updated");
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_import_malformed_json() {
        let (mut mem, _dir) = temp_memory();
        let temp_path = std::env::temp_dir().join("zc_snap_test_bad.json");
        std::fs::write(&temp_path, "not json").unwrap();
        assert!(import_snapshot(&mut mem, &temp_path, false).await.is_err());
        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_roundtrip() {
        let (mut mem, _dir) = temp_memory();
        mem.set("rt1", "value1", "user", vec!["tag1".to_string()], 0.9)
            .await
            .unwrap();
        mem.set("rt2", "value2", "general", vec![], 0.5)
            .await
            .unwrap();

        let temp_path = std::env::temp_dir().join("zc_snap_test_rt.json");
        export_snapshot(&mem, &temp_path).unwrap();

        let (mut mem2, _dir2) = temp_memory();
        let (imported, _) = import_snapshot(&mut mem2, &temp_path, false).await.unwrap();
        assert_eq!(imported, 2);
        assert_eq!(mem2.get_readonly("rt1").unwrap().value, "value1");
        assert_eq!(mem2.get_readonly("rt2").unwrap().value, "value2");
        let _ = std::fs::remove_file(&temp_path);
    }
}
