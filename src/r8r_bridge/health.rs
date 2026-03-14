//! Health ping loop and CLI status for r8r bridge.

use std::sync::Arc;

use tokio::time;
use tracing::warn;

use crate::r8r_bridge::events::BridgeEvent;
use crate::r8r_bridge::R8rBridge;

// ---------------------------------------------------------------------------
// start_health_ping_loop
// ---------------------------------------------------------------------------

/// Spawn a background task that sends periodic health pings to r8r.
///
/// The task ticks every `interval_secs` seconds.  On each tick, if the bridge
/// reports [`R8rBridge::is_connected`], it calls [`R8rBridge::send_health_ping`]
/// and logs a warning if the send fails.
///
/// The caller receives a [`tokio::task::JoinHandle`] and can abort the loop at
/// any time via [`tokio::task::JoinHandle::abort`].
pub fn start_health_ping_loop(
    bridge: Arc<R8rBridge>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            if bridge.is_connected() {
                if let Err(e) = bridge.send_health_ping().await {
                    warn!("r8r bridge: health ping failed: {e}");
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// format_health_status
// ---------------------------------------------------------------------------

/// Format a human-readable health status string for CLI output.
///
/// * If `connected` is `false`, returns `"r8r bridge: not connected"`.
/// * If connected and `status` is `Some(BridgeEvent::HealthStatus { .. })`,
///   returns a multiline block with version, uptime, and counters.
/// * If connected but `status` is `None`, returns
///   `"r8r bridge: connected (no status yet)"`.
pub fn format_health_status(connected: bool, status: &Option<BridgeEvent>) -> String {
    if !connected {
        return "r8r bridge: not connected".to_string();
    }

    match status {
        Some(BridgeEvent::HealthStatus {
            version,
            uptime_secs,
            active_executions,
            pending_approvals,
            workflows_loaded,
        }) => {
            format!(
                "r8r bridge: connected\n  version: {version}\n  uptime: {uptime}\n  active executions: {active_executions}\n  pending approvals: {pending_approvals}\n  workflows loaded: {workflows_loaded}",
                uptime = format_uptime(*uptime_secs),
            )
        }
        _ => "r8r bridge: connected (no status yet)".to_string(),
    }
}

// ---------------------------------------------------------------------------
// format_uptime helper
// ---------------------------------------------------------------------------

/// Format a duration (in seconds) as a compact human-readable string.
///
/// * `>= 86400` → `"Xd Yh Zm"`
/// * `>= 3600`  → `"Xh Ym"`
/// * else       → `"Xm"`
fn format_uptime(secs: u64) -> String {
    if secs >= 86_400 {
        let days = secs / 86_400;
        let hours = (secs % 86_400) / 3600;
        let mins = (secs % 3600) / 60;
        format!("{days}d {hours}h {mins}m")
    } else if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{hours}h {mins}m")
    } else {
        let mins = secs / 60;
        format!("{mins}m")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_uptime_minutes_only() {
        assert_eq!(format_uptime(0), "0m");
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(60), "1m");
        assert_eq!(format_uptime(3599), "59m");
    }

    #[test]
    fn test_format_uptime_hours() {
        assert_eq!(format_uptime(3600), "1h 0m");
        assert_eq!(format_uptime(7384), "2h 3m"); // 7384 = 2*3600 + 3*60 + 4
        assert_eq!(format_uptime(86399), "23h 59m");
    }

    #[test]
    fn test_format_uptime_days() {
        assert_eq!(format_uptime(86400), "1d 0h 0m");
        assert_eq!(format_uptime(90061), "1d 1h 1m"); // 86400 + 3600 + 60 + 1
        assert_eq!(format_uptime(172800), "2d 0h 0m");
    }

    #[test]
    fn test_format_health_status_not_connected() {
        let result = format_health_status(false, &None);
        assert_eq!(result, "r8r bridge: not connected");
    }

    #[test]
    fn test_format_health_status_no_status_yet() {
        let result = format_health_status(true, &None);
        assert_eq!(result, "r8r bridge: connected (no status yet)");
    }

    #[test]
    fn test_format_health_status_with_data() {
        let status = Some(BridgeEvent::HealthStatus {
            version: "1.2.3".to_string(),
            uptime_secs: 3661,
            active_executions: 5,
            pending_approvals: 2,
            workflows_loaded: 10,
        });
        let result = format_health_status(true, &status);
        assert!(result.starts_with("r8r bridge: connected\n"));
        assert!(result.contains("version: 1.2.3"));
        assert!(result.contains("uptime: 1h 1m"));
        assert!(result.contains("active executions: 5"));
        assert!(result.contains("pending approvals: 2"));
        assert!(result.contains("workflows loaded: 10"));
    }

    #[test]
    fn test_format_health_status_not_connected_ignores_status() {
        let status = Some(BridgeEvent::HealthStatus {
            version: "0.1.0".to_string(),
            uptime_secs: 100,
            active_executions: 0,
            pending_approvals: 0,
            workflows_loaded: 0,
        });
        let result = format_health_status(false, &status);
        assert_eq!(result, "r8r bridge: not connected");
    }
}
