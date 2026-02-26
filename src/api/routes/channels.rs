//! Channel status routes.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn list_channels(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "channels": [] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    #[tokio::test]
    async fn test_list_channels() {
        let state = State(Arc::new(AppState::new("tok".into(), EventBus::new(16))));
        let Json(body) = list_channels(state).await;
        assert!(body["channels"].is_array());
    }
}
