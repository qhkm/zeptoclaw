//! Tool execution metrics collector.
//!
//! Provides a lightweight, thread-safe metrics collector for tracking tool
//! execution statistics within a session. Uses interior mutability via
//! `Mutex` so all recording methods take `&self`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-tool execution statistics.
#[derive(Debug, Clone, Default)]
pub struct ToolMetrics {
    /// Total number of calls made to this tool.
    pub call_count: u64,
    /// Number of calls that resulted in an error.
    pub error_count: u64,
    /// Cumulative duration of all calls.
    pub total_duration: Duration,
    /// Shortest call duration observed.
    pub min_duration: Option<Duration>,
    /// Longest call duration observed.
    pub max_duration: Option<Duration>,
}

impl ToolMetrics {
    /// Returns the average call duration, or `None` if no calls have been recorded.
    pub fn average_duration(&self) -> Option<Duration> {
        if self.call_count == 0 {
            return None;
        }
        Some(self.total_duration / self.call_count as u32)
    }

    /// Returns the success rate as a value between 0.0 and 1.0.
    ///
    /// If no calls have been recorded, returns 1.0 (100%).
    pub fn success_rate(&self) -> f64 {
        if self.call_count == 0 {
            return 1.0;
        }
        (self.call_count - self.error_count) as f64 / self.call_count as f64
    }
}

/// Session-level metrics collector.
///
/// Thread-safe via interior `Mutex`. All recording methods take `&self`,
/// making it easy to share across async tasks via `Arc<MetricsCollector>`.
#[derive(Debug)]
pub struct MetricsCollector {
    tools: Mutex<HashMap<String, ToolMetrics>>,
    session_start: Instant,
    total_tokens_in: Mutex<u64>,
    total_tokens_out: Mutex<u64>,
}

impl MetricsCollector {
    /// Creates a new metrics collector. The session clock starts immediately.
    pub fn new() -> Self {
        Self {
            tools: Mutex::new(HashMap::new()),
            session_start: Instant::now(),
            total_tokens_in: Mutex::new(0),
            total_tokens_out: Mutex::new(0),
        }
    }

    /// Records a single tool call.
    ///
    /// Updates the per-tool `ToolMetrics` entry, creating it if this is the
    /// first call for the given tool name.
    pub fn record_tool_call(&self, tool_name: &str, duration: Duration, success: bool) {
        let mut tools = self.tools.lock().unwrap();
        let metrics = tools.entry(tool_name.to_string()).or_default();

        metrics.call_count += 1;
        if !success {
            metrics.error_count += 1;
        }
        metrics.total_duration += duration;

        metrics.min_duration = Some(match metrics.min_duration {
            Some(current) => current.min(duration),
            None => duration,
        });

        metrics.max_duration = Some(match metrics.max_duration {
            Some(current) => current.max(duration),
            None => duration,
        });
    }

    /// Adds to the running token totals.
    pub fn record_tokens(&self, input_tokens: u64, output_tokens: u64) {
        *self.total_tokens_in.lock().unwrap() += input_tokens;
        *self.total_tokens_out.lock().unwrap() += output_tokens;
    }

    /// Returns a clone of the metrics for a specific tool, or `None` if the
    /// tool has never been called.
    pub fn tool_metrics(&self, tool_name: &str) -> Option<ToolMetrics> {
        let tools = self.tools.lock().unwrap();
        tools.get(tool_name).cloned()
    }

    /// Returns a snapshot of all per-tool metrics.
    pub fn all_tool_metrics(&self) -> HashMap<String, ToolMetrics> {
        self.tools.lock().unwrap().clone()
    }

    /// Returns the sum of `call_count` across all tools.
    pub fn total_tool_calls(&self) -> u64 {
        let tools = self.tools.lock().unwrap();
        tools.values().map(|m| m.call_count).sum()
    }

    /// Returns the running token totals as `(input, output)`.
    pub fn total_tokens(&self) -> (u64, u64) {
        let input = *self.total_tokens_in.lock().unwrap();
        let output = *self.total_tokens_out.lock().unwrap();
        (input, output)
    }

    /// Returns the elapsed time since the collector was created.
    pub fn session_duration(&self) -> Duration {
        self.session_start.elapsed()
    }

    /// Compute an approximate p95 tool duration across all tools.
    ///
    /// Uses `avg + 2 * (max - avg)` clamped to max as a rough estimate.
    /// Returns `None` if no tool calls have been recorded.
    pub fn approx_p95_duration(&self) -> Option<Duration> {
        let tools = self.tools.lock().unwrap();
        let mut max_p95 = Duration::ZERO;
        let mut found_any = false;

        for metrics in tools.values() {
            if let (Some(avg), Some(max)) = (metrics.average_duration(), metrics.max_duration) {
                found_any = true;
                let spread = max.saturating_sub(avg);
                // p95 ≈ avg + 2*(max-avg), clamped to max
                let p95 = (avg + spread.mul_f32(2.0)).min(max);
                if p95 > max_p95 {
                    max_p95 = p95;
                }
            }
        }

        if found_any {
            Some(max_p95)
        } else {
            None
        }
    }

    /// Compute the aggregate success rate across all tools.
    ///
    /// Returns 1.0 if no calls have been recorded.
    pub fn aggregate_success_rate(&self) -> f64 {
        let tools = self.tools.lock().unwrap();
        let total_calls: u64 = tools.values().map(|m| m.call_count).sum();
        let total_errors: u64 = tools.values().map(|m| m.error_count).sum();

        if total_calls == 0 {
            return 1.0;
        }
        (total_calls - total_errors) as f64 / total_calls as f64
    }

    /// Produces a human-readable summary of the session metrics.
    ///
    /// Example output:
    /// ```text
    /// Session: 45s | Tools: 12 calls (2 errors) | Tokens: 1500 in / 800 out
    ///   shell: 5 calls, avg 200ms, 100% success
    ///   read_file: 4 calls, avg 5ms, 100% success
    ///   web_fetch: 3 calls, avg 1.2s, 67% success
    /// ```
    pub fn summary(&self) -> String {
        let tools = self.tools.lock().unwrap();
        let (tokens_in, tokens_out) = self.total_tokens();
        let session_secs = self.session_duration().as_secs();

        let total_calls: u64 = tools.values().map(|m| m.call_count).sum();
        let total_errors: u64 = tools.values().map(|m| m.error_count).sum();

        let mut summary = format!(
            "Session: {}s | Tools: {} calls ({} errors) | Tokens: {} in / {} out",
            session_secs, total_calls, total_errors, tokens_in, tokens_out,
        );

        // Sort tools by call_count descending.
        let mut entries: Vec<_> = tools.iter().collect();
        entries.sort_by(|a, b| b.1.call_count.cmp(&a.1.call_count));

        for (name, metrics) in entries {
            let avg = match metrics.average_duration() {
                Some(d) => format_duration(d),
                None => "N/A".to_string(),
            };
            let success_pct = (metrics.success_rate() * 100.0).round() as u64;
            summary.push_str(&format!(
                "\n  {}: {} calls, avg {}, {}% success",
                name, metrics.call_count, avg, success_pct,
            ));
        }

        summary
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Formats a duration in a human-friendly way.
///
/// - Under 1ms: shows microseconds (e.g. "500us")
/// - Under 1s: shows milliseconds (e.g. "200ms")
/// - 1s or more: shows seconds with one decimal (e.g. "1.2s")
fn format_duration(d: Duration) -> String {
    let micros = d.as_micros();
    if micros < 1_000 {
        format!("{}us", micros)
    } else if micros < 1_000_000 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_metrics_collector_new() {
        let collector = MetricsCollector::new();

        assert!(collector.all_tool_metrics().is_empty());
        assert_eq!(collector.total_tool_calls(), 0);
        assert_eq!(collector.total_tokens(), (0, 0));
    }

    #[test]
    fn test_record_tool_call_success() {
        let collector = MetricsCollector::new();
        let duration = Duration::from_millis(100);

        collector.record_tool_call("shell", duration, true);

        let metrics = collector.tool_metrics("shell").unwrap();
        assert_eq!(metrics.call_count, 1);
        assert_eq!(metrics.error_count, 0);
        assert_eq!(metrics.total_duration, duration);
        assert_eq!(metrics.min_duration, Some(duration));
        assert_eq!(metrics.max_duration, Some(duration));
    }

    #[test]
    fn test_record_tool_call_failure() {
        let collector = MetricsCollector::new();
        let duration = Duration::from_millis(50);

        collector.record_tool_call("web_fetch", duration, false);

        let metrics = collector.tool_metrics("web_fetch").unwrap();
        assert_eq!(metrics.call_count, 1);
        assert_eq!(metrics.error_count, 1);
    }

    #[test]
    fn test_record_multiple_calls() {
        let collector = MetricsCollector::new();

        collector.record_tool_call("shell", Duration::from_millis(100), true);
        collector.record_tool_call("shell", Duration::from_millis(200), true);
        collector.record_tool_call("shell", Duration::from_millis(300), true);

        let metrics = collector.tool_metrics("shell").unwrap();
        assert_eq!(metrics.call_count, 3);
        assert_eq!(metrics.error_count, 0);
        assert_eq!(metrics.total_duration, Duration::from_millis(600));
        assert_eq!(metrics.min_duration, Some(Duration::from_millis(100)));
        assert_eq!(metrics.max_duration, Some(Duration::from_millis(300)));

        let avg = metrics.average_duration().unwrap();
        assert_eq!(avg, Duration::from_millis(200));
    }

    #[test]
    fn test_record_tokens() {
        let collector = MetricsCollector::new();

        collector.record_tokens(500, 200);
        collector.record_tokens(1000, 600);

        assert_eq!(collector.total_tokens(), (1500, 800));
    }

    #[test]
    fn test_total_tool_calls() {
        let collector = MetricsCollector::new();

        collector.record_tool_call("shell", Duration::from_millis(10), true);
        collector.record_tool_call("shell", Duration::from_millis(20), true);
        collector.record_tool_call("read_file", Duration::from_millis(5), true);
        collector.record_tool_call("web_fetch", Duration::from_millis(1000), false);

        assert_eq!(collector.total_tool_calls(), 4);
    }

    #[test]
    fn test_tool_metrics_unknown_tool() {
        let collector = MetricsCollector::new();

        assert!(collector.tool_metrics("nonexistent").is_none());
    }

    #[test]
    fn test_all_tool_metrics() {
        let collector = MetricsCollector::new();

        collector.record_tool_call("shell", Duration::from_millis(10), true);
        collector.record_tool_call("read_file", Duration::from_millis(5), true);
        collector.record_tool_call("web_fetch", Duration::from_millis(1000), false);

        let all = collector.all_tool_metrics();
        assert_eq!(all.len(), 3);
        assert!(all.contains_key("shell"));
        assert!(all.contains_key("read_file"));
        assert!(all.contains_key("web_fetch"));
    }

    #[test]
    fn test_average_duration() {
        let mut metrics = ToolMetrics::default();
        assert!(metrics.average_duration().is_none());

        metrics.call_count = 4;
        metrics.total_duration = Duration::from_millis(400);
        assert_eq!(metrics.average_duration(), Some(Duration::from_millis(100)));
    }

    #[test]
    fn test_success_rate() {
        let collector = MetricsCollector::new();

        collector.record_tool_call("web_fetch", Duration::from_millis(100), true);
        collector.record_tool_call("web_fetch", Duration::from_millis(200), true);
        collector.record_tool_call("web_fetch", Duration::from_millis(300), true);
        collector.record_tool_call("web_fetch", Duration::from_millis(400), false);

        let metrics = collector.tool_metrics("web_fetch").unwrap();
        let rate = metrics.success_rate();
        assert!((rate - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_success_rate_zero_calls() {
        let metrics = ToolMetrics::default();
        assert!((metrics.success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_summary_format() {
        let collector = MetricsCollector::new();

        collector.record_tool_call("shell", Duration::from_millis(200), true);
        collector.record_tool_call("shell", Duration::from_millis(200), true);
        collector.record_tool_call("read_file", Duration::from_millis(5), true);
        collector.record_tool_call("web_fetch", Duration::from_millis(1200), false);

        collector.record_tokens(1500, 800);

        let summary = collector.summary();

        assert!(summary.contains("Session:"));
        assert!(summary.contains("Tools: 4 calls (1 errors)"));
        assert!(summary.contains("Tokens: 1500 in / 800 out"));
        assert!(summary.contains("shell: 2 calls"));
        assert!(summary.contains("read_file: 1 calls"));
        assert!(summary.contains("web_fetch: 1 calls"));
        assert!(summary.contains("% success"));
    }

    #[test]
    fn test_session_duration() {
        let collector = MetricsCollector::new();
        thread::sleep(Duration::from_millis(10));

        let duration = collector.session_duration();
        assert!(duration >= Duration::from_millis(10));
    }

    #[test]
    fn test_approx_p95_duration_no_calls() {
        let collector = MetricsCollector::new();
        assert!(collector.approx_p95_duration().is_none());
    }

    #[test]
    fn test_approx_p95_duration_single_tool() {
        let collector = MetricsCollector::new();
        collector.record_tool_call("shell", Duration::from_millis(100), true);
        collector.record_tool_call("shell", Duration::from_millis(500), true);

        let p95 = collector.approx_p95_duration().unwrap();
        assert!(p95 >= Duration::from_millis(100));
        assert!(p95 <= Duration::from_millis(500));
    }

    #[test]
    fn test_aggregate_success_rate_no_calls() {
        let collector = MetricsCollector::new();
        assert!((collector.aggregate_success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_success_rate_mixed() {
        let collector = MetricsCollector::new();
        collector.record_tool_call("a", Duration::from_millis(10), true);
        collector.record_tool_call("a", Duration::from_millis(10), true);
        collector.record_tool_call("b", Duration::from_millis(10), true);
        collector.record_tool_call("b", Duration::from_millis(10), false);

        let rate = collector.aggregate_success_rate();
        assert!((rate - 0.75).abs() < f64::EPSILON);
    }
}
