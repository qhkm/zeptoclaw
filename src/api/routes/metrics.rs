//! Metrics and cost routes.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn get_metrics(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "tokens": { "input": 0, "output": 0 },
        "cost": { "total": 0.0, "by_provider": {}, "by_model": {} },
        "tools": {}
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    #[tokio::test]
    async fn test_get_metrics() {
        let state = State(Arc::new(AppState::new("tok".into(), EventBus::new(16))));
        let Json(body) = get_metrics(state).await;
        assert!(body["tokens"]["input"].is_number());
        assert!(body["cost"]["total"].is_number());
    }
}
