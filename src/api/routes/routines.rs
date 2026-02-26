//! Routine management routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn list_routines(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "routines": [] }))
}

pub async fn create_routine(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    (StatusCode::CREATED, Json(json!({ "id": "stub" })))
}

pub async fn update_routine(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_body): Json<Value>,
) -> StatusCode {
    StatusCode::OK
}

pub async fn delete_routine(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

pub async fn toggle_routine(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Json<Value> {
    Json(json!({ "enabled": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_list_routines() {
        let Json(body) = list_routines(test_state()).await;
        assert!(body["routines"].is_array());
    }

    #[tokio::test]
    async fn test_create_routine() {
        let (status, _) = create_routine(test_state(), Json(json!({}))).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_toggle_routine() {
        let Json(body) = toggle_routine(test_state(), Path("r1".into())).await;
        assert!(body["enabled"].is_boolean());
    }
}
