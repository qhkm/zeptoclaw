//! Cron job management routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn list_jobs(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "jobs": [] }))
}

pub async fn create_job(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    (StatusCode::CREATED, Json(json!({ "id": "stub" })))
}

pub async fn update_job(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_body): Json<Value>,
) -> StatusCode {
    StatusCode::OK
}

pub async fn delete_job(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

pub async fn trigger_job(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> StatusCode {
    StatusCode::ACCEPTED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let Json(body) = list_jobs(test_state()).await;
        assert!(body["jobs"].is_array());
    }

    #[tokio::test]
    async fn test_create_job() {
        let (status, Json(body)) = create_job(test_state(), Json(json!({}))).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["id"], "stub");
    }

    #[tokio::test]
    async fn test_trigger_job() {
        let status = trigger_job(test_state(), Path("job1".into())).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
}
