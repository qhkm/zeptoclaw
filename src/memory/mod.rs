//! Workspace memory utilities (OpenClaw-style markdown memory).

#[cfg(feature = "memory-bm25")]
pub mod bm25_searcher;
pub mod builtin_searcher;
pub mod factory;
pub mod longterm;
pub mod traits;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

pub use traits::MemorySearcher;

use crate::config::MemoryConfig;
use crate::error::{Result, ZeptoError};
use crate::security::validate_path_in_workspace;

const CHUNK_LINES: usize = 18;
const CHUNK_OVERLAP: usize = 4;
const DEFAULT_GET_LINES: usize = 80;
const MAX_GET_LINES: usize = 400;

/// Maximum characters for memory injection into system prompt (~500 tokens).
pub const MEMORY_INJECTION_BUDGET: usize = 2000;

/// Search result entry returned by memory search.
#[derive(Debug, Clone, Serialize)]
pub struct MemorySearchResult {
    /// Workspace-relative file path.
    pub path: String,
    /// First line of the snippet (1-based).
    pub start_line: usize,
    /// Last line of the snippet (1-based).
    pub end_line: usize,
    /// Similarity score in `[0.0, 1.0]`.
    pub score: f32,
    /// Snippet content.
    pub snippet: String,
    /// Optional citation (`path#Lx-Ly`).
    pub citation: Option<String>,
}

/// File content read result for memory_get.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryReadResult {
    /// Workspace-relative file path.
    pub path: String,
    /// Starting line actually returned.
    pub start_line: usize,
    /// Ending line actually returned.
    pub end_line: usize,
    /// Total line count in file.
    pub total_lines: usize,
    /// Whether the returned content is truncated.
    pub truncated: bool,
    /// Returned snippet text.
    pub text: String,
}

/// Search memory markdown files in workspace (async wrapper).
///
/// Offloads the CPU+IO bound search to a blocking thread via
/// `tokio::task::spawn_blocking` so the Tokio runtime is not blocked.
pub async fn search_workspace_memory(
    workspace: &Path,
    query: &str,
    config: &MemoryConfig,
    searcher: Arc<dyn MemorySearcher>,
    max_results: Option<usize>,
    min_score: Option<f32>,
    include_citations: bool,
) -> Result<Vec<MemorySearchResult>> {
    let workspace = workspace.to_path_buf();
    let query = query.to_string();
    let config = config.clone();

    tokio::task::spawn_blocking(move || {
        search_workspace_memory_sync(
            &workspace,
            &query,
            &config,
            &*searcher,
            max_results,
            min_score,
            include_citations,
        )
    })
    .await
    .map_err(|e| ZeptoError::Tool(format!("Memory search task failed: {}", e)))?
}

/// Synchronous implementation of workspace memory search.
fn search_workspace_memory_sync(
    workspace: &Path,
    query: &str,
    config: &MemoryConfig,
    searcher: &dyn MemorySearcher,
    max_results: Option<usize>,
    min_score: Option<f32>,
    include_citations: bool,
) -> Result<Vec<MemorySearchResult>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ZeptoError::Tool("Memory query cannot be empty".to_string()));
    }

    let files = collect_memory_files(workspace, config)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let max_results = max_results
        .unwrap_or(config.max_results as usize)
        .clamp(1, 50);
    let min_score = min_score.unwrap_or(config.min_score).clamp(0.0, 1.0);
    let snippet_chars = (config.max_snippet_chars as usize).max(64);

    let mut results = Vec::new();

    for file in files {
        let content = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            continue;
        }

        let relative = relative_path(workspace, &file);
        let step = CHUNK_LINES.saturating_sub(CHUNK_OVERLAP).max(1);

        for start in (0..lines.len()).step_by(step) {
            let end = (start + CHUNK_LINES).min(lines.len());
            let chunk = lines[start..end].join("\n");
            if chunk.trim().is_empty() {
                if end == lines.len() {
                    break;
                }
                continue;
            }

            let score = searcher.score(&chunk, query);
            if score < min_score {
                if end == lines.len() {
                    break;
                }
                continue;
            }

            let mut snippet = chunk.trim().to_string();
            if snippet.chars().count() > snippet_chars {
                snippet = truncate_chars(&snippet, snippet_chars);
            }

            let citation = if include_citations {
                Some(format_citation(&relative, start + 1, end))
            } else {
                None
            };

            if let Some(ref c) = citation {
                snippet = format!("{}\n\nSource: {}", snippet, c);
            }

            results.push(MemorySearchResult {
                path: relative.clone(),
                start_line: start + 1,
                end_line: end,
                score,
                snippet,
                citation,
            });

            if end == lines.len() {
                break;
            }
        }
    }

    results.sort_by(|a, b| b.score.total_cmp(&a.score));
    results.truncate(max_results);

    Ok(results)
}

/// Read a memory markdown file (optionally line-ranged, async wrapper).
///
/// Offloads the IO bound read to a blocking thread via
/// `tokio::task::spawn_blocking` so the Tokio runtime is not blocked.
pub async fn read_workspace_memory(
    workspace: &Path,
    rel_path: &str,
    from: Option<usize>,
    lines: Option<usize>,
    config: &MemoryConfig,
) -> Result<MemoryReadResult> {
    let workspace = workspace.to_path_buf();
    let rel_path = rel_path.to_string();
    let config = config.clone();

    tokio::task::spawn_blocking(move || {
        read_workspace_memory_sync(&workspace, &rel_path, from, lines, &config)
    })
    .await
    .map_err(|e| ZeptoError::Tool(format!("Memory read task failed: {}", e)))?
}

/// Synchronous implementation of workspace memory read.
fn read_workspace_memory_sync(
    workspace: &Path,
    rel_path: &str,
    from: Option<usize>,
    lines: Option<usize>,
    config: &MemoryConfig,
) -> Result<MemoryReadResult> {
    let requested = normalize_rel_path(rel_path);
    if requested.is_empty() {
        return Err(ZeptoError::Tool("'path' cannot be empty".to_string()));
    }

    let candidates = collect_memory_files(workspace, config)?;
    let target = candidates
        .into_iter()
        .find(|path| normalize_rel_path(&relative_path(workspace, path)) == requested)
        .ok_or_else(|| {
            ZeptoError::Tool(format!(
                "Memory path not found or not allowed: {}",
                rel_path
            ))
        })?;

    let content = fs::read_to_string(&target)
        .map_err(|e| ZeptoError::Tool(format!("Failed to read memory file: {}", e)))?;

    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len();

    let start_line = from.unwrap_or(1).max(1);
    let line_count = lines.unwrap_or(DEFAULT_GET_LINES).clamp(1, MAX_GET_LINES);

    if total_lines == 0 || start_line > total_lines {
        return Ok(MemoryReadResult {
            path: relative_path(workspace, &target),
            start_line,
            end_line: start_line.saturating_sub(1),
            total_lines,
            truncated: false,
            text: String::new(),
        });
    }

    let start_idx = start_line - 1;
    let end_idx = (start_idx + line_count).min(total_lines);
    let text = all_lines[start_idx..end_idx].join("\n");

    Ok(MemoryReadResult {
        path: relative_path(workspace, &target),
        start_line,
        end_line: end_idx,
        total_lines,
        truncated: end_idx < total_lines,
        text,
    })
}

/// Build memory context string for injection into the system prompt.
///
/// Collects pinned memories first (always included), then query-matched
/// results from the user's message. Stops when budget_chars is reached.
/// Returns empty string if no memories qualify.
pub fn build_memory_injection(
    ltm: &crate::memory::longterm::LongTermMemory,
    user_message: &str,
    budget_chars: usize,
) -> String {
    let mut parts = Vec::new();
    let mut used_chars = 0usize;
    let mut seen_keys = std::collections::HashSet::new();

    // 1. Always inject pinned memories
    let pinned = ltm.list_by_category("pinned");
    let mut pinned_lines = Vec::new();
    for entry in pinned {
        let line = format!("- {}: {}", entry.key, entry.value);
        if used_chars + line.len() + 1 > budget_chars {
            break;
        }
        used_chars += line.len() + 1; // +1 for newline
        seen_keys.insert(entry.key.clone());
        pinned_lines.push(line);
    }

    // 2. Query-match from user message
    let mut relevant_lines = Vec::new();
    if !user_message.trim().is_empty() {
        let results = ltm.search(user_message);
        for entry in results.iter().take(5) {
            if seen_keys.contains(&entry.key) {
                continue;
            }
            let line = format!("- {}: {}", entry.key, entry.value);
            if used_chars + line.len() + 1 > budget_chars {
                break;
            }
            used_chars += line.len() + 1;
            seen_keys.insert(entry.key.clone());
            relevant_lines.push(line);
        }
    }

    // 3. Build output
    if pinned_lines.is_empty() && relevant_lines.is_empty() {
        return String::new();
    }

    parts.push("## Memory\n".to_string());
    if !pinned_lines.is_empty() {
        parts.push("### Pinned".to_string());
        parts.extend(pinned_lines);
        parts.push(String::new()); // blank line
    }
    if !relevant_lines.is_empty() {
        parts.push("### Relevant".to_string());
        parts.extend(relevant_lines);
    }

    parts.join("\n").trim().to_string()
}

fn collect_memory_files(workspace: &Path, config: &MemoryConfig) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !workspace.exists() {
        return Ok(files);
    }

    let workspace_str = workspace.to_string_lossy().to_string();

    if config.include_default_memory {
        collect_if_markdown(&workspace.join("MEMORY.md"), &workspace_str, &mut files);
        collect_if_markdown(&workspace.join("memory.md"), &workspace_str, &mut files);
        collect_markdown_dir(&workspace.join("memory"), &workspace_str, &mut files);
    }

    for extra in &config.extra_paths {
        if extra.trim().is_empty() {
            continue;
        }
        let safe = match validate_path_in_workspace(extra, &workspace_str) {
            Ok(safe) => safe.into_path_buf(),
            Err(_) => continue,
        };
        if safe.is_file() {
            collect_if_markdown(&safe, &workspace_str, &mut files);
        } else if safe.is_dir() {
            collect_markdown_dir(&safe, &workspace_str, &mut files);
        }
    }

    Ok(dedup_paths(files))
}

fn collect_if_markdown(path: &Path, workspace: &str, files: &mut Vec<PathBuf>) {
    if !path.is_file() || !is_markdown(path) {
        return;
    }

    let path_str = path.to_string_lossy();
    if validate_path_in_workspace(&path_str, workspace).is_ok() {
        files.push(path.to_path_buf());
    }
}

const MAX_DIR_DEPTH: usize = 10;

fn collect_markdown_dir(dir: &Path, workspace: &str, files: &mut Vec<PathBuf>) {
    collect_markdown_dir_recursive(dir, workspace, files, 0);
}

fn collect_markdown_dir_recursive(
    dir: &Path,
    workspace: &str,
    files: &mut Vec<PathBuf>,
    depth: usize,
) {
    if depth > MAX_DIR_DEPTH {
        return;
    }

    if !dir.exists() || !dir.is_dir() {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.filter_map(|entry| entry.ok()) {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_markdown_dir_recursive(&path, workspace, files, depth + 1);
            continue;
        }
        collect_if_markdown(&path, workspace, files);
    }
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for path in paths {
        let key = path.canonicalize().unwrap_or(path.clone());
        if seen.insert(key) {
            out.push(path);
        }
    }

    out
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn normalize_rel_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

fn relative_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn format_citation(path: &str, start_line: usize, end_line: usize) -> String {
    if start_line == end_line {
        format!("{}#L{}", path, start_line)
    } else {
        format!("{}#L{}-L{}", path, start_line, end_line)
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::builtin_searcher::BuiltinSearcher;
    use super::*;
    use crate::config::{MemoryBackend, MemoryCitationsMode};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_search_workspace_memory_finds_entries() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        fs::write(
            workspace.join("MEMORY.md"),
            "Project: ZeptoClaw\nPreference: concise responses\n",
        )
        .unwrap();

        let config = MemoryConfig::default();
        let results = search_workspace_memory(
            workspace,
            "concise preference",
            &config,
            Arc::new(BuiltinSearcher),
            Some(5),
            Some(0.1),
            true,
        )
        .await
        .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].path, "MEMORY.md");
        assert!(results[0].citation.is_some());
    }

    #[tokio::test]
    async fn test_read_workspace_memory_reads_line_window() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        fs::create_dir_all(workspace.join("memory")).unwrap();
        fs::write(
            workspace.join("memory/2026-02-13.md"),
            "line1\nline2\nline3\nline4\n",
        )
        .unwrap();

        let config = MemoryConfig::default();
        let result =
            read_workspace_memory(workspace, "memory/2026-02-13.md", Some(2), Some(2), &config)
                .await
                .unwrap();

        assert_eq!(result.start_line, 2);
        assert_eq!(result.end_line, 3);
        assert_eq!(result.text, "line2\nline3");
        assert!(result.truncated);
    }

    #[test]
    fn test_collect_memory_files_respects_config_flags() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        fs::write(workspace.join("MEMORY.md"), "abc").unwrap();

        let mut config = MemoryConfig::default();
        config.backend = MemoryBackend::Disabled;
        config.citations = MemoryCitationsMode::Off;
        config.include_default_memory = false;

        let files = collect_memory_files(workspace, &config).unwrap();
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn test_build_memory_injection_pinned_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let mut ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();
        ltm.set("user:name", "Alice", "pinned", vec![], 1.0)
            .await
            .unwrap();
        ltm.set("pref:lang", "Rust", "pinned", vec![], 1.0)
            .await
            .unwrap();

        let result = build_memory_injection(&ltm, "", 2000);
        assert!(result.contains("## Memory"));
        assert!(result.contains("### Pinned"));
        assert!(result.contains("user:name: Alice"));
        assert!(result.contains("pref:lang: Rust"));
        assert!(!result.contains("### Relevant"));
    }

    #[tokio::test]
    async fn test_build_memory_injection_query_match() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let mut ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();
        ltm.set("fact:project", "ZeptoClaw is 4MB", "fact", vec![], 1.0)
            .await
            .unwrap();
        ltm.set("fact:other", "unrelated thing", "fact", vec![], 1.0)
            .await
            .unwrap();

        let result = build_memory_injection(&ltm, "ZeptoClaw", 2000);
        assert!(result.contains("### Relevant"));
        assert!(result.contains("ZeptoClaw is 4MB"));
    }

    #[tokio::test]
    async fn test_build_memory_injection_pinned_not_duplicated() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let mut ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();
        ltm.set("user:name", "Alice", "pinned", vec![], 1.0)
            .await
            .unwrap();

        let result = build_memory_injection(&ltm, "Alice", 2000);
        // "user:name: Alice" should appear only once (in pinned, not duplicated in relevant)
        assert_eq!(result.matches("user:name: Alice").count(), 1);
    }

    #[tokio::test]
    async fn test_build_memory_injection_budget_enforcement() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let mut ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();
        // Create entries that exceed a small budget
        for i in 0..20 {
            ltm.set(
                &format!("pin:{}", i),
                &"x".repeat(50),
                "pinned",
                vec![],
                1.0,
            )
            .await
            .unwrap();
        }

        let result = build_memory_injection(&ltm, "", 200);
        // Should be truncated to fit within ~200 chars
        assert!(
            result.len() < 300,
            "Result length {} should be < 300",
            result.len()
        );
    }

    #[test]
    fn test_build_memory_injection_empty_memories() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();

        let result = build_memory_injection(&ltm, "hello", 2000);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_build_memory_injection_empty_message_no_relevant() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let mut ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();
        ltm.set("fact:x", "value", "fact", vec![], 1.0)
            .await
            .unwrap();

        let result = build_memory_injection(&ltm, "", 2000);
        // With empty message, no query-match, and no pinned entries => empty
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_build_memory_injection_mixed_pinned_and_relevant() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lt.json");
        let mut ltm = crate::memory::longterm::LongTermMemory::with_path(path).unwrap();
        ltm.set("user:name", "Alice", "pinned", vec![], 1.0)
            .await
            .unwrap();
        ltm.set("fact:rust", "Rust is fast", "fact", vec![], 1.0)
            .await
            .unwrap();

        let result = build_memory_injection(&ltm, "Rust", 2000);
        assert!(result.contains("### Pinned"));
        assert!(result.contains("### Relevant"));
        assert!(result.contains("user:name: Alice"));
        assert!(result.contains("fact:rust: Rust is fast"));
    }
}
