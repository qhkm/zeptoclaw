use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Sliding-window rate limiter. Per-IP request tracking.
pub struct SlidingWindowRateLimiter {
    limit: u32,
    window: Duration,
    entries: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl SlidingWindowRateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if the request is allowed, false if rate-limited.
    /// A limit of 0 means unlimited (always allows).
    pub fn check(&self, ip: IpAddr) -> bool {
        if self.limit == 0 {
            return true;
        }

        let now = Instant::now();
        let cutoff = now - self.window;
        let mut entries = self.entries.lock().unwrap();

        let timestamps = entries.entry(ip).or_default();

        // Remove expired entries
        while timestamps.front().is_some_and(|&t| t <= cutoff) {
            timestamps.pop_front();
        }

        if timestamps.len() >= self.limit as usize {
            return false;
        }

        timestamps.push_back(now);
        true
    }

    /// Remove IPs with no active timestamps (call periodically).
    pub fn sweep(&self) {
        let now = Instant::now();
        let cutoff = now - self.window;
        let mut entries = self.entries.lock().unwrap();

        entries.retain(|_, timestamps| {
            while timestamps.front().is_some_and(|&t| t <= cutoff) {
                timestamps.pop_front();
            }
            !timestamps.is_empty()
        });
    }

    /// Number of tracked IPs (for testing).
    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// Combined rate limiter for gateway endpoints.
pub struct GatewayRateLimiter {
    pair: SlidingWindowRateLimiter,
    webhook: SlidingWindowRateLimiter,
}

impl GatewayRateLimiter {
    pub fn new(pair_per_min: u32, webhook_per_min: u32, window: Duration) -> Self {
        Self {
            pair: SlidingWindowRateLimiter::new(pair_per_min, window),
            webhook: SlidingWindowRateLimiter::new(webhook_per_min, window),
        }
    }

    pub fn check_pair(&self, ip: IpAddr) -> bool {
        self.pair.check(ip)
    }

    pub fn check_webhook(&self, ip: IpAddr) -> bool {
        self.webhook.check(ip)
    }

    pub fn sweep(&self) {
        self.pair.sweep();
        self.webhook.sweep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn localhost() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    }

    fn other_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
    }

    #[test]
    fn test_zero_limit_allows_all() {
        let limiter = SlidingWindowRateLimiter::new(0, Duration::from_secs(60));
        for _ in 0..100 {
            assert!(limiter.check(localhost()));
        }
    }

    #[test]
    fn test_allows_up_to_limit() {
        let limiter = SlidingWindowRateLimiter::new(3, Duration::from_secs(60));
        assert!(limiter.check(localhost()));
        assert!(limiter.check(localhost()));
        assert!(limiter.check(localhost()));
        assert!(!limiter.check(localhost())); // 4th should be denied
    }

    #[test]
    fn test_different_ips_independent() {
        let limiter = SlidingWindowRateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.check(localhost()));
        assert!(limiter.check(other_ip())); // different IP, separate bucket
        assert!(!limiter.check(localhost())); // same IP, over limit
    }

    #[test]
    fn test_expired_entries_freed() {
        // Use a large enough window that two consecutive calls are never in
        // separate windows, even on a heavily-loaded debug build.
        let limiter = SlidingWindowRateLimiter::new(1, Duration::from_secs(5));
        assert!(limiter.check(localhost()));
        assert!(!limiter.check(localhost())); // still within window, denied

        // Now use a short-window limiter to verify expiry resets the counter.
        let short = SlidingWindowRateLimiter::new(1, Duration::from_millis(50));
        assert!(short.check(localhost()));
        std::thread::sleep(Duration::from_millis(100)); // outlast the window
        assert!(short.check(localhost())); // window expired, should be allowed
    }

    #[test]
    fn test_sweep_clears_stale_ips() {
        let limiter = SlidingWindowRateLimiter::new(1, Duration::from_millis(1));
        assert!(limiter.check(localhost()));
        std::thread::sleep(Duration::from_millis(5));
        limiter.sweep();
        assert_eq!(limiter.entry_count(), 0);
    }

    #[test]
    fn test_gateway_rate_limiter_separate_limits() {
        let grl = GatewayRateLimiter::new(1, 2, Duration::from_secs(60));
        assert!(grl.check_pair(localhost()));
        assert!(!grl.check_pair(localhost())); // pair limit = 1
        assert!(grl.check_webhook(localhost()));
        assert!(grl.check_webhook(localhost()));
        assert!(!grl.check_webhook(localhost())); // webhook limit = 2
    }
}
