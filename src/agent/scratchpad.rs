//! Shared scratchpad for agent-to-agent communication within a swarm session.
//!
//! The `SwarmScratchpad` provides a thread-safe key-value store where sub-agents
//! can write their results and subsequent sub-agents can see what previous agents
//! produced. Keys are typically role names (e.g., "researcher", "writer").

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A shared scratchpad for passing context between sub-agents in a swarm session.
///
/// Each sub-agent's completion result is written to the scratchpad keyed by its
/// role name. Subsequent sub-agents receive the scratchpad contents injected into
/// their system prompt so they can build on previous work.
///
/// Thread-safe via `Arc<RwLock<...>>` — multiple readers, exclusive writer.
#[derive(Debug, Clone, Default)]
pub struct SwarmScratchpad {
    entries: Arc<RwLock<HashMap<String, String>>>,
}

impl SwarmScratchpad {
    /// Create a new empty scratchpad.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Write a role's output to the scratchpad.
    ///
    /// Overwrites any previous entry for the same role.
    pub async fn write(&self, role: &str, output: &str) {
        let mut entries = self.entries.write().await;
        entries.insert(role.to_string(), output.to_string());
    }

    /// Read a specific role's output from the scratchpad.
    pub async fn read(&self, role: &str) -> Option<String> {
        let entries = self.entries.read().await;
        entries.get(role).cloned()
    }

    /// Get all entries as a snapshot.
    pub async fn entries(&self) -> HashMap<String, String> {
        let entries = self.entries.read().await;
        entries.clone()
    }

    /// Check if the scratchpad has any entries.
    pub async fn is_empty(&self) -> bool {
        let entries = self.entries.read().await;
        entries.is_empty()
    }

    /// Format all scratchpad entries as a system prompt injection.
    ///
    /// Returns `None` if the scratchpad is empty.
    /// Returns a formatted string like:
    /// ```text
    /// Previous agent outputs:
    /// - Researcher: {result}
    /// - Writer: {result}
    /// ```
    pub async fn format_for_prompt(&self) -> Option<String> {
        let entries = self.entries.read().await;
        if entries.is_empty() {
            return None;
        }

        let mut lines = vec!["Previous agent outputs:".to_string()];
        // Sort for deterministic output
        let mut sorted: Vec<_> = entries.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (role, output) in sorted {
            // Truncate long outputs to avoid blowing up context
            let truncated = if output.len() > 2000 {
                format!("{}... [truncated]", &output[..2000])
            } else {
                output.clone()
            };
            lines.push(format!("- {}: {}", role, truncated));
        }
        Some(lines.join("\n"))
    }

    /// Clear all entries.
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scratchpad_new_is_empty() {
        let sp = SwarmScratchpad::new();
        assert!(sp.is_empty().await);
        assert_eq!(sp.format_for_prompt().await, None);
    }

    #[tokio::test]
    async fn test_scratchpad_write_read() {
        let sp = SwarmScratchpad::new();
        sp.write("researcher", "Found 3 results").await;
        assert_eq!(
            sp.read("researcher").await,
            Some("Found 3 results".to_string())
        );
        assert_eq!(sp.read("writer").await, None);
        assert!(!sp.is_empty().await);
    }

    #[tokio::test]
    async fn test_scratchpad_overwrite() {
        let sp = SwarmScratchpad::new();
        sp.write("analyst", "First analysis").await;
        sp.write("analyst", "Updated analysis").await;
        assert_eq!(
            sp.read("analyst").await,
            Some("Updated analysis".to_string())
        );
    }

    #[tokio::test]
    async fn test_scratchpad_format_for_prompt() {
        let sp = SwarmScratchpad::new();
        sp.write("researcher", "Found data").await;
        sp.write("writer", "Wrote summary").await;

        let prompt = sp.format_for_prompt().await.unwrap();
        assert!(prompt.starts_with("Previous agent outputs:"));
        assert!(prompt.contains("- researcher: Found data"));
        assert!(prompt.contains("- writer: Wrote summary"));
    }

    #[tokio::test]
    async fn test_scratchpad_format_truncates_long_output() {
        let sp = SwarmScratchpad::new();
        let long_output = "x".repeat(3000);
        sp.write("analyst", &long_output).await;

        let prompt = sp.format_for_prompt().await.unwrap();
        assert!(prompt.contains("[truncated]"));
        assert!(prompt.len() < 3000 + 200); // much less than full output
    }

    #[tokio::test]
    async fn test_scratchpad_clear() {
        let sp = SwarmScratchpad::new();
        sp.write("researcher", "data").await;
        assert!(!sp.is_empty().await);
        sp.clear().await;
        assert!(sp.is_empty().await);
    }

    #[tokio::test]
    async fn test_scratchpad_entries_snapshot() {
        let sp = SwarmScratchpad::new();
        sp.write("a", "alpha").await;
        sp.write("b", "beta").await;
        let entries = sp.entries().await;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("a"), Some(&"alpha".to_string()));
        assert_eq!(entries.get("b"), Some(&"beta".to_string()));
    }

    #[tokio::test]
    async fn test_scratchpad_clone_shares_state() {
        let sp = SwarmScratchpad::new();
        let sp2 = sp.clone();
        sp.write("role1", "data1").await;
        // Clone should share the same Arc
        assert_eq!(sp2.read("role1").await, Some("data1".to_string()));
    }
}
