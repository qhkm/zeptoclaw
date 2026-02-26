//! Channel status routes.

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

/// Per-channel status item returned in the list response.
#[derive(Serialize)]
struct ChannelStatus {
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    restart_count: u64,
}

/// Known channel component name prefixes registered in `HealthRegistry`.
///
/// Channel components are registered under their channel type name (e.g.
/// "telegram", "discord", "whatsapp").  We surface all health checks whose
/// name matches one of these known prefixes so the panel can display live
/// channel status without hard-coding assumptions about which channels are
/// enabled.
const CHANNEL_NAMES: &[&str] = &[
    "telegram", "discord", "slack", "whatsapp", "webhook", "email", "serial", "lark",
];

pub async fn list_channels(State(state): State<Arc<AppState>>) -> Json<Value> {
    let Some(ref registry) = state.health_registry else {
        return Json(json!({ "channels": [] }));
    };

    let channels: Vec<ChannelStatus> = registry
        .all_checks()
        .into_iter()
        .filter(|c| {
            CHANNEL_NAMES
                .iter()
                .any(|&prefix| c.name.starts_with(prefix))
        })
        .map(|c| ChannelStatus {
            name: c.name,
            status: match c.status {
                crate::health::HealthStatus::Ok => "ok".to_string(),
                crate::health::HealthStatus::Degraded => "degraded".to_string(),
                crate::health::HealthStatus::Down => "down".to_string(),
            },
            message: c.message,
            restart_count: c.restart_count,
        })
        .collect();

    Json(json!({ "channels": channels }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_list_channels_no_registry() {
        let Json(body) = list_channels(test_state()).await;
        assert!(body["channels"].is_array());
        assert_eq!(body["channels"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_list_channels_with_registry_channel_checks() {
        use crate::health::{HealthCheck, HealthRegistry, HealthStatus};

        let registry = Arc::new(HealthRegistry::new());
        registry.register(HealthCheck {
            name: "telegram".into(),
            status: HealthStatus::Ok,
            message: Some("running".into()),
            ..Default::default()
        });
        registry.register(HealthCheck {
            name: "discord".into(),
            status: HealthStatus::Degraded,
            message: None,
            ..Default::default()
        });
        // Non-channel check — should be filtered out.
        registry.register(HealthCheck {
            name: "provider".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.health_registry = Some(registry);

        let Json(body) = list_channels(State(Arc::new(state))).await;
        let channels = body["channels"].as_array().expect("channels array");
        assert_eq!(channels.len(), 2);

        let names: Vec<&str> = channels.iter().filter_map(|c| c["name"].as_str()).collect();
        assert!(names.contains(&"telegram"));
        assert!(names.contains(&"discord"));
    }

    #[tokio::test]
    async fn test_list_channels_filters_non_channel_checks() {
        use crate::health::{HealthCheck, HealthRegistry, HealthStatus};

        let registry = Arc::new(HealthRegistry::new());
        registry.register(HealthCheck {
            name: "provider".into(),
            status: HealthStatus::Ok,
            ..Default::default()
        });
        registry.register(HealthCheck {
            name: "db".into(),
            status: HealthStatus::Ok,
            ..Default::default()
        });

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.health_registry = Some(registry);

        let Json(body) = list_channels(State(Arc::new(state))).await;
        let channels = body["channels"].as_array().expect("channels array");
        assert_eq!(channels.len(), 0);
    }
}
