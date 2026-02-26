//! Health endpoint for the panel API.

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

/// Response shape for `GET /api/health`.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    uptime_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<UsageSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checks: Option<Vec<ComponentCheck>>,
}

#[derive(Serialize)]
struct UsageSnapshot {
    requests: u64,
    tool_calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    errors: u64,
}

#[derive(Serialize)]
struct ComponentCheck {
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    restart_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

/// GET /api/health — returns live system health when stores are wired in.
pub async fn get_health(State(state): State<Arc<AppState>>) -> Json<Value> {
    use std::sync::atomic::Ordering;

    let uptime_secs = state.health_registry.as_ref().map(|r| r.uptime().as_secs());

    let usage = state.usage_metrics.as_ref().map(|m| UsageSnapshot {
        requests: m.requests.load(Ordering::Relaxed),
        tool_calls: m.tool_calls.load(Ordering::Relaxed),
        input_tokens: m.input_tokens.load(Ordering::Relaxed),
        output_tokens: m.output_tokens.load(Ordering::Relaxed),
        errors: m.errors.load(Ordering::Relaxed),
    });

    let checks = state.health_registry.as_ref().map(|r| {
        r.all_checks()
            .into_iter()
            .map(|c| ComponentCheck {
                name: c.name,
                status: match c.status {
                    crate::health::HealthStatus::Ok => "ok".to_string(),
                    crate::health::HealthStatus::Degraded => "degraded".to_string(),
                    crate::health::HealthStatus::Down => "down".to_string(),
                },
                message: c.message,
                restart_count: c.restart_count,
                last_error: c.last_error,
            })
            .collect::<Vec<_>>()
    });

    let status = match &state.health_registry {
        Some(r) => {
            if r.is_ready() {
                "ok"
            } else {
                "degraded"
            }
        }
        None => "ok",
    };

    let resp = HealthResponse {
        status,
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs,
        usage,
        checks,
    };

    Json(
        serde_json::to_value(resp)
            .unwrap_or_else(|_| json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_get_health_returns_ok() {
        let Json(body) = get_health(test_state()).await;
        assert_eq!(body["status"], "ok");
        assert!(body["version"].is_string());
    }

    #[tokio::test]
    async fn test_get_health_no_optional_fields_when_no_stores() {
        let Json(body) = get_health(test_state()).await;
        // When no stores are wired, optional fields should be absent.
        assert!(body.get("uptime_secs").is_none());
        assert!(body.get("usage").is_none());
        assert!(body.get("checks").is_none());
    }

    #[tokio::test]
    async fn test_get_health_with_registry() {
        use crate::health::{HealthCheck, HealthRegistry, HealthStatus};

        let registry = Arc::new(HealthRegistry::new());
        registry.register(HealthCheck {
            name: "telegram".into(),
            status: HealthStatus::Ok,
            message: Some("running".into()),
            ..Default::default()
        });

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.health_registry = Some(registry);

        let Json(body) = get_health(State(Arc::new(state))).await;
        assert_eq!(body["status"], "ok");
        assert!(body["uptime_secs"].is_number());
        let checks = body["checks"].as_array().expect("checks array");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0]["name"], "telegram");
        assert_eq!(checks[0]["status"], "ok");
    }

    #[tokio::test]
    async fn test_get_health_with_usage_metrics() {
        use crate::health::UsageMetrics;
        use std::sync::atomic::Ordering;

        let metrics = Arc::new(UsageMetrics::new());
        metrics.requests.fetch_add(5, Ordering::Relaxed);
        metrics.tool_calls.fetch_add(10, Ordering::Relaxed);

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.usage_metrics = Some(metrics);

        let Json(body) = get_health(State(Arc::new(state))).await;
        assert_eq!(body["usage"]["requests"], 5);
        assert_eq!(body["usage"]["tool_calls"], 10);
    }
}
