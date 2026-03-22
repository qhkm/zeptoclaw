//! Token budget tracking for per-session usage limits.
//!
//! Provides a thread-safe, atomic counter for tracking LLM token consumption
//! against an optional budget limit. The agent loop should check [`TokenBudget::is_exceeded`]
//! before each LLM call to enforce the budget.
//!
//! # Design
//!
//! - All counters use `AtomicU64` with `Relaxed` ordering for lock-free updates.
//! - A limit of `0` means unlimited (no budget constraint).
//! - Budget checking is advisory; enforcement lives in the agent loop.
//!
//! # Example
//!
//! ```rust,ignore
//! use zeptoclaw::agent::budget::TokenBudget;
//!
//! let budget = TokenBudget::new(10_000);
//! budget.record(500, 200);
//! assert!(!budget.is_exceeded());
//! assert_eq!(budget.remaining(), Some(9300));
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-session token budget tracker.
///
/// Thread-safe via atomics. Tracks input and output tokens separately
/// with an overall budget limit.
#[derive(Debug)]
pub struct TokenBudget {
    /// Maximum total tokens allowed (input + output). 0 = unlimited.
    limit: u64,
    /// Running total of input tokens consumed.
    input_used: AtomicU64,
    /// Running total of output tokens consumed.
    output_used: AtomicU64,
}

impl TokenBudget {
    /// Creates a new token budget with the given limit.
    ///
    /// A limit of `0` means unlimited -- the budget will never be exceeded.
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            input_used: AtomicU64::new(0),
            output_used: AtomicU64::new(0),
        }
    }

    /// Creates an unlimited token budget (no cap on usage).
    pub fn unlimited() -> Self {
        Self::new(0)
    }

    /// Records token consumption from an LLM call.
    ///
    /// Adds `input_tokens` and `output_tokens` to the running totals.
    /// This method takes `&self` (not `&mut self`) thanks to interior
    /// mutability via atomics.
    pub fn record(&self, input_tokens: u64, output_tokens: u64) {
        self.input_used.fetch_add(input_tokens, Ordering::Relaxed);
        self.output_used.fetch_add(output_tokens, Ordering::Relaxed);
    }

    /// Returns the total tokens consumed (input + output).
    pub fn total_used(&self) -> u64 {
        self.input_used.load(Ordering::Relaxed) + self.output_used.load(Ordering::Relaxed)
    }

    /// Returns the total input tokens consumed.
    pub fn input_used(&self) -> u64 {
        self.input_used.load(Ordering::Relaxed)
    }

    /// Returns the total output tokens consumed.
    pub fn output_used(&self) -> u64 {
        self.output_used.load(Ordering::Relaxed)
    }

    /// Returns the number of tokens remaining before the budget is exhausted.
    ///
    /// Returns `None` if the budget is unlimited.
    /// Returns `Some(0)` if the budget is already exceeded.
    pub fn remaining(&self) -> Option<u64> {
        if self.is_unlimited() {
            return None;
        }
        let used = self.total_used();
        if used >= self.limit {
            Some(0)
        } else {
            Some(self.limit - used)
        }
    }

    /// Returns `true` if the budget has been exceeded.
    ///
    /// Always returns `false` for unlimited budgets.
    pub fn is_exceeded(&self) -> bool {
        if self.is_unlimited() {
            return false;
        }
        self.total_used() >= self.limit
    }

    /// Returns `true` if this budget has no limit.
    pub fn is_unlimited(&self) -> bool {
        self.limit == 0
    }

    /// Returns the configured token limit.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Returns the usage as a percentage of the budget.
    ///
    /// Returns `None` if the budget is unlimited.
    /// The value can exceed 100.0 if the budget has been exceeded.
    pub fn usage_percentage(&self) -> Option<f64> {
        if self.is_unlimited() {
            return None;
        }
        Some((self.total_used() as f64 / self.limit as f64) * 100.0)
    }

    /// Returns a human-readable summary of token usage.
    ///
    /// Examples:
    /// - `"Tokens: 1500/10000 (15.0%)"`
    /// - `"Tokens: 1500 (unlimited)"`
    pub fn summary(&self) -> String {
        let used = self.total_used();
        if self.is_unlimited() {
            format!("Tokens: {} (unlimited)", used)
        } else {
            let pct = self.usage_percentage().unwrap_or(0.0);
            format!("Tokens: {}/{} ({:.1}%)", used, self.limit, pct)
        }
    }

    /// Resets all token counters to zero.
    ///
    /// The limit remains unchanged. Useful for reusing a budget
    /// across multiple sessions with the same configuration.
    pub fn reset(&self) {
        self.input_used.store(0, Ordering::Relaxed);
        self.output_used.store(0, Ordering::Relaxed);
    }
}

impl fmt::Display for TokenBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.summary())
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::unlimited()
    }
}

/// Resolve effective token budget from global config and optional template override.
///
/// - `global` = 0 means unlimited; `template` = None means inherit global.
/// - `template` = Some(0) means "no override" (treated same as None, since 0 = unlimited
///   in TokenBudget, and a template must not expand beyond global).
/// - When both are set (non-zero), the lower value wins.
/// - When global is unlimited (0) and template sets a non-zero value, template wins.
pub fn resolve_token_budget(global: u64, template: Option<u64>) -> u64 {
    match template {
        None | Some(0) => global,
        Some(tpl) => {
            if global == 0 {
                tpl
            } else {
                global.min(tpl)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_budget_new() {
        let budget = TokenBudget::new(10_000);
        assert_eq!(budget.limit(), 10_000);
        assert_eq!(budget.input_used(), 0);
        assert_eq!(budget.output_used(), 0);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn test_token_budget_unlimited() {
        let budget = TokenBudget::unlimited();
        assert_eq!(budget.limit(), 0);
        assert!(budget.is_unlimited());
        assert!(!budget.is_exceeded());
    }

    #[test]
    fn test_token_budget_default() {
        let budget = TokenBudget::default();
        assert_eq!(budget.limit(), 0);
        assert!(budget.is_unlimited());
        assert!(!budget.is_exceeded());
    }

    #[test]
    fn test_record_tokens() {
        let budget = TokenBudget::new(10_000);
        budget.record(500, 200);
        assert_eq!(budget.input_used(), 500);
        assert_eq!(budget.output_used(), 200);
    }

    #[test]
    fn test_total_used() {
        let budget = TokenBudget::new(10_000);
        budget.record(500, 200);
        assert_eq!(budget.total_used(), 700);
    }

    #[test]
    fn test_remaining_with_limit() {
        let budget = TokenBudget::new(10_000);
        budget.record(2_000, 1_000);
        assert_eq!(budget.remaining(), Some(7_000));
    }

    #[test]
    fn test_remaining_unlimited() {
        let budget = TokenBudget::unlimited();
        budget.record(5_000, 3_000);
        assert_eq!(budget.remaining(), None);
    }

    #[test]
    fn test_is_exceeded_under_limit() {
        let budget = TokenBudget::new(10_000);
        budget.record(3_000, 2_000);
        assert!(!budget.is_exceeded());
    }

    #[test]
    fn test_is_exceeded_at_limit() {
        let budget = TokenBudget::new(10_000);
        budget.record(6_000, 4_000);
        assert!(budget.is_exceeded());
    }

    #[test]
    fn test_is_exceeded_over_limit() {
        let budget = TokenBudget::new(10_000);
        budget.record(7_000, 5_000);
        assert!(budget.is_exceeded());
    }

    #[test]
    fn test_is_exceeded_unlimited() {
        let budget = TokenBudget::unlimited();
        budget.record(1_000_000, 1_000_000);
        assert!(!budget.is_exceeded());
    }

    #[test]
    fn test_usage_percentage() {
        let budget = TokenBudget::new(10_000);
        budget.record(2_000, 1_000);
        let pct = budget.usage_percentage().unwrap();
        assert!((pct - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_usage_percentage_unlimited() {
        let budget = TokenBudget::unlimited();
        budget.record(5_000, 3_000);
        assert!(budget.usage_percentage().is_none());
    }

    #[test]
    fn test_summary_with_limit() {
        let budget = TokenBudget::new(10_000);
        budget.record(1_000, 500);
        let s = budget.summary();
        assert!(s.contains("1500"));
        assert!(s.contains("10000"));
        assert!(s.contains("15.0%"));
    }

    #[test]
    fn test_summary_unlimited() {
        let budget = TokenBudget::unlimited();
        budget.record(1_000, 500);
        let s = budget.summary();
        assert!(s.contains("1500"));
        assert!(s.contains("unlimited"));
    }

    #[test]
    fn test_display() {
        let budget = TokenBudget::new(10_000);
        budget.record(1_000, 500);
        let display = format!("{}", budget);
        assert_eq!(display, budget.summary());
    }

    #[test]
    fn test_reset() {
        let budget = TokenBudget::new(10_000);
        budget.record(3_000, 2_000);
        assert_eq!(budget.total_used(), 5_000);
        budget.reset();
        assert_eq!(budget.input_used(), 0);
        assert_eq!(budget.output_used(), 0);
        assert_eq!(budget.total_used(), 0);
        // Limit should remain unchanged.
        assert_eq!(budget.limit(), 10_000);
    }

    #[test]
    fn test_multiple_records() {
        let budget = TokenBudget::new(50_000);
        budget.record(1_000, 500);
        budget.record(2_000, 800);
        budget.record(3_000, 1_200);
        assert_eq!(budget.input_used(), 6_000);
        assert_eq!(budget.output_used(), 2_500);
        assert_eq!(budget.total_used(), 8_500);
    }

    #[test]
    fn test_resolve_token_budget_template_only() {
        assert_eq!(resolve_token_budget(0, Some(50000)), 50000);
    }

    #[test]
    fn test_resolve_token_budget_global_only() {
        assert_eq!(resolve_token_budget(100000, None), 100000);
    }

    #[test]
    fn test_resolve_token_budget_both_takes_min() {
        assert_eq!(resolve_token_budget(100000, Some(50000)), 50000);
        assert_eq!(resolve_token_budget(30000, Some(50000)), 30000);
    }

    #[test]
    fn test_resolve_token_budget_both_unlimited() {
        assert_eq!(resolve_token_budget(0, None), 0);
    }

    #[test]
    fn test_resolve_token_budget_template_zero_cannot_expand() {
        // Template setting 0 must NOT disable a finite global budget
        assert_eq!(resolve_token_budget(50000, Some(0)), 50000);
        // When global is also unlimited, stays unlimited
        assert_eq!(resolve_token_budget(0, Some(0)), 0);
    }
}
