//! Per-reason cooldown tracking for LLM provider fallback.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::error::ProviderError;

/// Why a provider failed — determines cooldown duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailoverReason {
    RateLimit,
    Overloaded,
    Timeout,
    Auth,
    Billing,
    Format,
    Unknown,
}

impl FailoverReason {
    /// Classify a `ProviderError` into a `FailoverReason`.
    pub fn from_provider_error(err: &ProviderError) -> Self {
        match err {
            ProviderError::RateLimit(_) => Self::RateLimit,
            ProviderError::Overloaded(_) => Self::Overloaded,
            ProviderError::Timeout(_) => Self::Timeout,
            ProviderError::Auth(_) => Self::Auth,
            ProviderError::Billing(_) => Self::Billing,
            ProviderError::Format(_) | ProviderError::InvalidRequest(_) => Self::Format,
            _ => Self::Unknown,
        }
    }

    /// Cooldown duration for this failure reason, given consecutive failure count.
    pub fn cooldown_duration(&self, consecutive: u32) -> Duration {
        match self {
            // Billing: exponential hours. 1h, 2h, 4h, 8h, max 24h.
            Self::Billing => {
                let hours = 2u64.saturating_pow(consecutive.saturating_sub(1));
                Duration::from_secs(hours.min(24) * 3600)
            }
            // Rate limit: exponential minutes. 1m, 2m, 4m, max 30m.
            Self::RateLimit => {
                let mins = 2u64.saturating_pow(consecutive.saturating_sub(1));
                Duration::from_secs(mins.min(30) * 60)
            }
            // Overloaded: short backoff. 30s, 60s, 120s, max 5m.
            Self::Overloaded => {
                let secs = 30u64 * 2u64.saturating_pow(consecutive.saturating_sub(1));
                Duration::from_secs(secs.min(300))
            }
            // Timeout: like overloaded but shorter.
            Self::Timeout => {
                let secs = 15u64 * 2u64.saturating_pow(consecutive.saturating_sub(1));
                Duration::from_secs(secs.min(120))
            }
            // Auth/Format: non-retriable — set a short cooldown to avoid spam.
            Self::Auth | Self::Format => Duration::from_secs(300),
            Self::Unknown => Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
struct CooldownEntry {
    consecutive: u32,
    cooldown_end: Option<Instant>,
    last_failure: Option<Instant>,
    billing_disabled_until: Option<Instant>,
}

impl CooldownEntry {
    fn new() -> Self {
        Self {
            consecutive: 0,
            cooldown_end: None,
            last_failure: None,
            billing_disabled_until: None,
        }
    }

    fn is_in_cooldown(&self) -> bool {
        let now = Instant::now();
        if let Some(billing) = self.billing_disabled_until {
            if now < billing {
                return true;
            }
        }
        if let Some(end) = self.cooldown_end {
            return now < end;
        }
        false
    }

    fn reset_if_stale(&mut self) {
        if let Some(last) = self.last_failure {
            if last.elapsed() > Duration::from_secs(86400) {
                self.consecutive = 0;
                self.cooldown_end = None;
                self.billing_disabled_until = None;
            }
        }
    }
}

/// Thread-safe per-provider cooldown tracker.
#[derive(Clone)]
pub struct CooldownTracker {
    entries: Arc<RwLock<HashMap<String, CooldownEntry>>>,
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Returns `true` if the provider is currently in cooldown and should be skipped.
    pub fn is_in_cooldown(&self, provider: &str) -> bool {
        let entries = self.entries.read().unwrap();
        entries
            .get(provider)
            .map(|e| e.is_in_cooldown())
            .unwrap_or(false)
    }

    /// Record a failure for the given provider.
    pub fn mark_failure(&self, provider: &str, reason: FailoverReason) {
        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .entry(provider.to_string())
            .or_insert_with(CooldownEntry::new);
        entry.reset_if_stale();
        entry.consecutive += 1;
        entry.last_failure = Some(Instant::now());
        let duration = reason.cooldown_duration(entry.consecutive);
        if reason == FailoverReason::Billing {
            entry.billing_disabled_until = Some(Instant::now() + duration);
        } else {
            entry.cooldown_end = Some(Instant::now() + duration);
        }
    }

    /// Record a success — reset all cooldown state for the provider.
    pub fn mark_success(&self, provider: &str) {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(provider) {
            entry.consecutive = 0;
            entry.cooldown_end = None;
            entry.billing_disabled_until = None;
        }
    }
}

impl Default for CooldownTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_in_cooldown_initially() {
        let tracker = CooldownTracker::new();
        assert!(!tracker.is_in_cooldown("anthropic"));
    }

    #[test]
    fn test_mark_failure_puts_in_cooldown() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("anthropic", FailoverReason::RateLimit);
        assert!(tracker.is_in_cooldown("anthropic"));
    }

    #[test]
    fn test_mark_success_clears_cooldown() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("anthropic", FailoverReason::RateLimit);
        tracker.mark_success("anthropic");
        assert!(!tracker.is_in_cooldown("anthropic"));
    }

    #[test]
    fn test_billing_longer_than_rate_limit() {
        let billing = FailoverReason::Billing.cooldown_duration(1);
        let rate = FailoverReason::RateLimit.cooldown_duration(1);
        assert!(billing > rate);
    }

    #[test]
    fn test_overloaded_shorter_than_billing() {
        let billing = FailoverReason::Billing.cooldown_duration(1);
        let overloaded = FailoverReason::Overloaded.cooldown_duration(1);
        assert!(overloaded < billing);
    }

    #[test]
    fn test_exponential_backoff_increases() {
        let d1 = FailoverReason::RateLimit.cooldown_duration(1);
        let d2 = FailoverReason::RateLimit.cooldown_duration(2);
        let d3 = FailoverReason::RateLimit.cooldown_duration(3);
        assert!(d2 > d1);
        assert!(d3 > d2);
    }

    #[test]
    fn test_billing_caps_at_24h() {
        let d = FailoverReason::Billing.cooldown_duration(100);
        assert_eq!(d, Duration::from_secs(86400));
    }

    #[test]
    fn test_from_provider_error() {
        assert_eq!(
            FailoverReason::from_provider_error(&ProviderError::RateLimit("".into())),
            FailoverReason::RateLimit
        );
        assert_eq!(
            FailoverReason::from_provider_error(&ProviderError::Billing("".into())),
            FailoverReason::Billing
        );
        assert_eq!(
            FailoverReason::from_provider_error(&ProviderError::Auth("".into())),
            FailoverReason::Auth
        );
        assert_eq!(
            FailoverReason::from_provider_error(&ProviderError::Overloaded("".into())),
            FailoverReason::Overloaded
        );
        assert_eq!(
            FailoverReason::from_provider_error(&ProviderError::Format("".into())),
            FailoverReason::Format
        );
    }

    #[test]
    fn test_multiple_providers_independent() {
        let tracker = CooldownTracker::new();
        tracker.mark_failure("anthropic", FailoverReason::Billing);
        assert!(tracker.is_in_cooldown("anthropic"));
        assert!(!tracker.is_in_cooldown("openai"));
    }

    #[test]
    fn test_consecutive_increases_cooldown() {
        let tracker = CooldownTracker::new();
        // First failure
        tracker.mark_failure("p", FailoverReason::RateLimit);
        let entry1_end = {
            let entries = tracker.entries.read().unwrap();
            entries["p"].cooldown_end.unwrap()
        };
        // Second failure (before stale window)
        tracker.mark_failure("p", FailoverReason::RateLimit);
        let entry2_end = {
            let entries = tracker.entries.read().unwrap();
            entries["p"].cooldown_end.unwrap()
        };
        assert!(
            entry2_end >= entry1_end,
            "second failure should have equal or longer cooldown"
        );
    }
}
