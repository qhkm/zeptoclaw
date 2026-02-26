//! Metrics and cost routes.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn get_metrics(State(state): State<Arc<AppState>>) -> Json<Value> {
    // Token totals — prefer MetricsCollector (per-session), fall back to
    // UsageMetrics (gateway-level), then zero.
    let (tokens_in, tokens_out) = if let Some(ref mc) = state.metrics_collector {
        mc.total_tokens()
    } else if let Some(ref um) = state.usage_metrics {
        (
            um.input_tokens.load(Ordering::Relaxed),
            um.output_tokens.load(Ordering::Relaxed),
        )
    } else {
        (0, 0)
    };

    // Per-tool stats from MetricsCollector.
    let tools_json: Value = if let Some(ref mc) = state.metrics_collector {
        let all = mc.all_tool_metrics();
        let mut map = serde_json::Map::new();
        for (name, m) in &all {
            map.insert(
                name.clone(),
                json!({
                    "call_count": m.call_count,
                    "error_count": m.error_count,
                    "success_rate": m.success_rate(),
                    "avg_ms": m.average_duration().map(|d| d.as_millis()),
                }),
            );
        }
        Value::Object(map)
    } else {
        json!({})
    };

    Json(json!({
        "tokens": { "input": tokens_in, "output": tokens_out },
        "cost": { "total": 0.0, "by_provider": {}, "by_model": {} },
        "tools": tools_json,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_get_metrics_no_stores() {
        let Json(body) = get_metrics(test_state()).await;
        assert_eq!(body["tokens"]["input"], 0);
        assert_eq!(body["tokens"]["output"], 0);
        assert!(body["cost"]["total"].is_number());
        assert!(body["tools"].is_object());
    }

    #[tokio::test]
    async fn test_get_metrics_with_metrics_collector() {
        use crate::utils::metrics::MetricsCollector;
        use std::time::Duration;

        let mc = Arc::new(MetricsCollector::new());
        mc.record_tokens(1000, 500);
        mc.record_tool_call("shell", Duration::from_millis(100), true);
        mc.record_tool_call("shell", Duration::from_millis(200), false);

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.metrics_collector = Some(mc);

        let Json(body) = get_metrics(State(Arc::new(state))).await;
        assert_eq!(body["tokens"]["input"], 1000);
        assert_eq!(body["tokens"]["output"], 500);
        let tools = body["tools"].as_object().expect("tools object");
        assert!(tools.contains_key("shell"));
        assert_eq!(tools["shell"]["call_count"], 2);
        assert_eq!(tools["shell"]["error_count"], 1);
    }

    #[tokio::test]
    async fn test_get_metrics_with_usage_metrics_fallback() {
        use crate::health::UsageMetrics;

        let um = Arc::new(UsageMetrics::new());
        um.record_tokens(300, 150);

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.usage_metrics = Some(um);

        let Json(body) = get_metrics(State(Arc::new(state))).await;
        assert_eq!(body["tokens"]["input"], 300);
        assert_eq!(body["tokens"]["output"], 150);
    }

    #[tokio::test]
    async fn test_get_metrics_collector_takes_priority_over_usage_metrics() {
        use crate::health::UsageMetrics;
        use crate::utils::metrics::MetricsCollector;

        let mc = Arc::new(MetricsCollector::new());
        mc.record_tokens(999, 888);

        let um = Arc::new(UsageMetrics::new());
        um.record_tokens(1, 1);

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.metrics_collector = Some(mc);
        state.usage_metrics = Some(um);

        let Json(body) = get_metrics(State(Arc::new(state))).await;
        // MetricsCollector wins
        assert_eq!(body["tokens"]["input"], 999);
        assert_eq!(body["tokens"]["output"], 888);
    }
}
