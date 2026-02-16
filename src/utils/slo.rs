//! Session-level SLO (Service Level Objective) tracking.
//!
//! Computes three metrics at session end:
//! 1. Tool success rate (target: >= 0.95)
//! 2. Agent completion (did the agent produce a final response?)
//! 3. Approximate p95 tool latency (target: < 10s)

use std::time::Duration;

use tracing::info;

use super::metrics::MetricsCollector;

/// SLO thresholds.
const SLO_TOOL_SUCCESS_RATE: f64 = 0.95;
const SLO_P95_LATENCY_SECS: f64 = 10.0;

/// Evaluated SLO result for a single session.
#[derive(Debug, Clone)]
pub struct SessionSLO {
    /// Tool success rate (0.0-1.0).
    pub tool_success_rate: f64,
    /// Whether the tool success rate meets the SLO.
    pub tool_success_met: bool,
    /// Whether the agent produced a final response.
    pub agent_completed: bool,
    /// Approximate p95 tool latency.
    pub p95_latency: Option<Duration>,
    /// Whether the p95 latency meets the SLO.
    pub p95_latency_met: bool,
    /// Total tool calls in this session.
    pub total_tool_calls: u64,
    /// Session duration.
    pub session_duration: Duration,
}

impl SessionSLO {
    /// Evaluate SLOs from a completed session's metrics.
    pub fn evaluate(metrics: &MetricsCollector, agent_completed: bool) -> Self {
        let tool_success_rate = metrics.aggregate_success_rate();
        let p95_latency = metrics.approx_p95_duration();
        let p95_latency_met = match p95_latency {
            Some(d) => d.as_secs_f64() < SLO_P95_LATENCY_SECS,
            None => true,
        };

        Self {
            tool_success_rate,
            tool_success_met: tool_success_rate >= SLO_TOOL_SUCCESS_RATE,
            agent_completed,
            p95_latency,
            p95_latency_met,
            total_tool_calls: metrics.total_tool_calls(),
            session_duration: metrics.session_duration(),
        }
    }

    /// Whether all SLOs are met.
    pub fn all_met(&self) -> bool {
        self.tool_success_met && self.agent_completed && self.p95_latency_met
    }

    /// Emit a structured tracing event with SLO results.
    pub fn emit(&self) {
        let p95_ms = self.p95_latency.map(|d| d.as_millis() as u64).unwrap_or(0);

        info!(
            event = "session_slo",
            tool_success_rate = format!("{:.2}", self.tool_success_rate),
            tool_success_met = self.tool_success_met,
            agent_completed = self.agent_completed,
            p95_latency_ms = p95_ms,
            p95_latency_met = self.p95_latency_met,
            all_met = self.all_met(),
            total_tool_calls = self.total_tool_calls,
            session_duration_secs = self.session_duration.as_secs(),
            "Session SLO evaluation"
        );
    }

    /// Human-readable summary string.
    pub fn summary(&self) -> String {
        let p95_str = match self.p95_latency {
            Some(d) => format!("{:.1}s", d.as_secs_f64()),
            None => "N/A".to_string(),
        };
        format!(
            "SLOs: tool_success={:.0}% [{}] | completed={} | p95={} [{}]",
            self.tool_success_rate * 100.0,
            if self.tool_success_met { "OK" } else { "MISS" },
            if self.agent_completed { "yes" } else { "no" },
            p95_str,
            if self.p95_latency_met { "OK" } else { "MISS" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_slo_all_met() {
        let metrics = MetricsCollector::new();
        for _ in 0..10 {
            metrics.record_tool_call("shell", Duration::from_millis(100), true);
        }

        let slo = SessionSLO::evaluate(&metrics, true);
        assert!(slo.all_met());
        assert!(slo.tool_success_met);
        assert!(slo.agent_completed);
        assert!(slo.p95_latency_met);
        assert_eq!(slo.total_tool_calls, 10);
    }

    #[test]
    fn test_session_slo_tool_success_miss() {
        let metrics = MetricsCollector::new();
        metrics.record_tool_call("shell", Duration::from_millis(100), true);
        metrics.record_tool_call("shell", Duration::from_millis(100), false);

        let slo = SessionSLO::evaluate(&metrics, true);
        assert!(!slo.tool_success_met);
        assert!(!slo.all_met());
        assert!((slo.tool_success_rate - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_session_slo_agent_not_completed() {
        let metrics = MetricsCollector::new();
        let slo = SessionSLO::evaluate(&metrics, false);
        assert!(!slo.agent_completed);
        assert!(!slo.all_met());
    }

    #[test]
    fn test_session_slo_no_tools() {
        let metrics = MetricsCollector::new();
        let slo = SessionSLO::evaluate(&metrics, true);
        assert!(slo.all_met());
        assert_eq!(slo.total_tool_calls, 0);
        assert!(slo.p95_latency.is_none());
    }

    #[test]
    fn test_session_slo_summary_format() {
        let metrics = MetricsCollector::new();
        metrics.record_tool_call("shell", Duration::from_millis(200), true);
        let slo = SessionSLO::evaluate(&metrics, true);
        let summary = slo.summary();
        assert!(summary.contains("tool_success="));
        assert!(summary.contains("completed="));
        assert!(summary.contains("p95="));
    }

    #[test]
    fn test_session_slo_emit_does_not_panic() {
        let metrics = MetricsCollector::new();
        metrics.record_tool_call("shell", Duration::from_millis(100), true);
        let slo = SessionSLO::evaluate(&metrics, true);
        slo.emit();
    }
}
