//! Memory CLI command handlers.

use anyhow::{Context, Result};
use zeptoclaw::memory::longterm::LongTermMemory;

use super::MemoryAction;

pub(crate) async fn cmd_memory(action: MemoryAction) -> Result<()> {
    match action {
        MemoryAction::List { category } => cmd_memory_list(category).await,
        MemoryAction::Search { query } => cmd_memory_search(query).await,
        MemoryAction::Set {
            key,
            value,
            category,
            tags,
        } => cmd_memory_set(key, value, category, tags).await,
        MemoryAction::Delete { key } => cmd_memory_delete(key).await,
        MemoryAction::Stats => cmd_memory_stats().await,
        MemoryAction::Cleanup { threshold } => cmd_memory_cleanup(threshold).await,
    }
}

async fn cmd_memory_list(category: Option<String>) -> Result<()> {
    let mem = LongTermMemory::new().with_context(|| "Failed to open long-term memory")?;
    let entries = if let Some(ref cat) = category {
        mem.list_by_category(cat)
    } else {
        mem.list_all()
    };

    if entries.is_empty() {
        if let Some(cat) = category {
            println!("No memories in category '{}'.", cat);
        } else {
            println!("No memories stored yet.");
            println!("Store one: zeptoclaw memory set user:name \"Your Name\"");
        }
        return Ok(());
    }

    println!("Long-term Memories ({})", entries.len());
    println!("{}", "-".repeat(60));
    for entry in &entries {
        let tags_str = if entry.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", entry.tags.join(", "))
        };
        println!("  {} ({}){}", entry.key, entry.category, tags_str);
        println!("    {}", truncate_value(&entry.value, 80));
    }
    Ok(())
}

async fn cmd_memory_search(query: String) -> Result<()> {
    let mem = LongTermMemory::new().with_context(|| "Failed to open long-term memory")?;
    let results = mem.search(&query);

    if results.is_empty() {
        println!("No memories matching '{}'.", query);
        return Ok(());
    }

    println!("Search results for '{}' ({})", query, results.len());
    println!("{}", "-".repeat(60));
    for entry in &results {
        println!("  {} ({})", entry.key, entry.category);
        println!("    {}", truncate_value(&entry.value, 80));
    }
    Ok(())
}

async fn cmd_memory_set(
    key: String,
    value: String,
    category: String,
    tags: Option<String>,
) -> Result<()> {
    let mut mem = LongTermMemory::new().with_context(|| "Failed to open long-term memory")?;
    let tag_vec: Vec<String> = tags
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    // Use default importance 1.0 for CLI-set memories
    mem.set(&key, &value, &category, tag_vec, 1.0).await?;
    println!(
        "Stored: {} = \"{}\" ({})",
        key,
        truncate_value(&value, 60),
        category
    );
    Ok(())
}

async fn cmd_memory_delete(key: String) -> Result<()> {
    let mut mem = LongTermMemory::new().with_context(|| "Failed to open long-term memory")?;
    if mem.delete(&key).await? {
        println!("Deleted: {}", key);
    } else {
        println!("Memory '{}' not found.", key);
    }
    Ok(())
}

async fn cmd_memory_stats() -> Result<()> {
    let mem = LongTermMemory::new().with_context(|| "Failed to open long-term memory")?;
    let count = mem.count();
    let categories = mem.categories();

    println!("Memory Statistics");
    println!("-----------------");
    println!("  Total entries: {}", count);
    println!(
        "  Categories:    {}",
        if categories.is_empty() {
            "none".to_string()
        } else {
            categories.join(", ")
        }
    );

    if !categories.is_empty() {
        println!();
        for cat in &categories {
            let cat_count = mem.list_by_category(cat).len();
            println!("  {}: {} entries", cat, cat_count);
        }
    }

    let path = zeptoclaw::config::Config::dir()
        .join("memory")
        .join("longterm.json");
    if path.exists() {
        if let Ok(meta) = std::fs::metadata(&path) {
            let size_kb = meta.len() as f64 / 1024.0;
            println!();
            println!("  Storage: {:?} ({:.1} KB)", path, size_kb);
        }
    }

    Ok(())
}

async fn cmd_memory_cleanup(threshold: f32) -> Result<()> {
    if !(0.0..=1.0).contains(&threshold) || !threshold.is_finite() {
        anyhow::bail!("Threshold must be between 0.0 and 1.0");
    }
    let mut mem = LongTermMemory::new().with_context(|| "Failed to open long-term memory")?;
    let before = mem.count();
    let removed = mem.cleanup_expired(threshold)?;
    println!(
        "Memory cleanup: removed {} of {} entries (threshold: {:.2})",
        removed, before, threshold
    );
    if removed > 0 {
        println!("Remaining: {} entries", mem.count());
    }
    Ok(())
}

fn truncate_value(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find a char boundary at or before `max` to avoid panicking on multi-byte UTF-8.
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i <= max)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_value_short() {
        assert_eq!(truncate_value("hello", 80), "hello");
    }

    #[test]
    fn test_truncate_value_long() {
        let long = "a".repeat(100);
        let result = truncate_value(&long, 10);
        assert!(result.len() <= 14); // 10 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_value_exact() {
        let s = "a".repeat(80);
        assert_eq!(truncate_value(&s, 80), s);
    }

    #[test]
    fn test_truncate_value_multibyte_utf8() {
        // Each emoji is 4 bytes. Truncating at byte 5 should land on a char boundary.
        let s = "\u{1F600}\u{1F601}\u{1F602}"; // 3 emoji = 12 bytes
        let result = truncate_value(s, 5);
        assert!(result.ends_with("..."));
        // Should include only the first emoji (4 bytes), not slice mid-char
        assert!(result.starts_with("\u{1F600}"));
    }

    #[test]
    fn test_truncate_value_cjk() {
        // CJK chars are 3 bytes each
        let s = "\u{4F60}\u{597D}\u{4E16}\u{754C}"; // 你好世界 = 12 bytes
        let result = truncate_value(s, 7);
        assert!(result.ends_with("..."));
        // 7 bytes = 2 full CJK chars (6 bytes) + partial, so boundary at 6
        assert_eq!(result, "\u{4F60}\u{597D}...");
    }
}
