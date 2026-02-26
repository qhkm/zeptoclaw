//! Kanban task routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn list_tasks(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "tasks": [] }))
}

pub async fn create_task(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    (StatusCode::CREATED, Json(json!({ "id": "stub" })))
}

pub async fn update_task(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_body): Json<Value>,
) -> StatusCode {
    StatusCode::OK
}

pub async fn delete_task(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

pub async fn move_task(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_body): Json<Value>,
) -> Json<Value> {
    Json(json!({ "column": "in_progress" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_list_tasks() {
        let Json(body) = list_tasks(test_state()).await;
        assert!(body["tasks"].is_array());
    }

    #[tokio::test]
    async fn test_create_task() {
        let (status, _) = create_task(test_state(), Json(json!({"title": "test"}))).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_move_task() {
        let Json(body) = move_task(
            test_state(),
            Path("t1".into()),
            Json(json!({"column": "done"})),
        )
        .await;
        assert!(body["column"].is_string());
    }
}
