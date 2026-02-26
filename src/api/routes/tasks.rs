//! Kanban task routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn list_tasks(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let Some(ref store) = state.task_store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "Task store not available"})),
        )
            .into_response();
    };

    let tasks = store.list(None).await;
    (StatusCode::OK, Json(json!({ "tasks": tasks }))).into_response()
}

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let Some(ref store) = state.task_store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "Task store not available"})),
        );
    };

    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled");
    let column = body
        .get("column")
        .and_then(|v| v.as_str())
        .unwrap_or("backlog");
    let assignee = body
        .get("assignee")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match store.create(title, column, assignee).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": e })),
        ),
    }
}

pub async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let Some(ref store) = state.task_store else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };

    match store.update(&id, body).await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::NOT_FOUND,
    }
}

pub async fn delete_task(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    let Some(ref store) = state.task_store else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };

    match store.delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::NOT_FOUND,
    }
}

pub async fn move_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let Some(ref store) = state.task_store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "Task store not available"})),
        );
    };

    let column = match body.get("column").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "column is required" })),
            )
        }
    };

    match store.move_task(&id, &column).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "column": column }))),
        Err(e) => {
            // Distinguish "not found" from "invalid column" for cleaner HTTP status.
            let status = if e.contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            (status, Json(json!({ "error": e })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;
    use crate::api::tasks::TaskStore;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    fn state_with_store() -> (State<Arc<AppState>>, Arc<TaskStore>) {
        let store = Arc::new(TaskStore::new_in_memory());
        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.task_store = Some(store.clone());
        (State(Arc::new(state)), store)
    }

    // ── no store (returns 503) ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_tasks_no_store_returns_503() {
        let resp = list_tasks(test_state()).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_create_task_no_store_returns_503() {
        let (status, _) = create_task(test_state(), Json(json!({"title": "test"}))).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_move_task_no_store_returns_503() {
        let (status, _) = move_task(
            test_state(),
            Path("t1".into()),
            Json(json!({"column": "done"})),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── with real store ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_tasks_with_store() {
        use axum::body::to_bytes;

        let (state, store) = state_with_store();
        store.create("Alpha", "backlog", None).await.unwrap();
        store.create("Beta", "in_progress", None).await.unwrap();

        let resp = list_tasks(state).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let tasks = body["tasks"].as_array().expect("tasks array");
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn test_create_task_with_store() {
        let (state, store) = state_with_store();
        let (status, Json(body)) = create_task(
            state,
            Json(json!({"title": "My Task", "column": "backlog"})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = body["id"].as_str().expect("id string");
        assert!(store.get(id).await.is_some());
    }

    #[tokio::test]
    async fn test_create_task_invalid_column() {
        let (state, _) = state_with_store();
        let (status, Json(body)) =
            create_task(state, Json(json!({"title": "Bad", "column": "wip"}))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn test_update_task_with_store() {
        let (state, store) = state_with_store();
        let id = store.create("Old", "backlog", None).await.unwrap();

        let status = update_task(state, Path(id.clone()), Json(json!({"title": "New"}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(store.get(&id).await.unwrap().title, "New");
    }

    #[tokio::test]
    async fn test_update_task_not_found() {
        let (state, _) = state_with_store();
        let status = update_task(state, Path("ghost-id".into()), Json(json!({"title": "x"}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_task_with_store() {
        let (state, store) = state_with_store();
        let id = store.create("Delete me", "done", None).await.unwrap();

        let status = delete_task(state, Path(id.clone())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(store.get(&id).await.is_none());
    }

    #[tokio::test]
    async fn test_delete_task_not_found() {
        let (state, _) = state_with_store();
        let status = delete_task(state, Path("ghost".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_move_task_with_store() {
        let (state, store) = state_with_store();
        let id = store.create("Moveable", "backlog", None).await.unwrap();

        let (status, Json(body)) = move_task(
            state,
            Path(id.clone()),
            Json(json!({"column": "in_progress"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["column"], "in_progress");
        assert_eq!(store.get(&id).await.unwrap().column, "in_progress");
    }

    #[tokio::test]
    async fn test_move_task_invalid_column() {
        let (state, store) = state_with_store();
        let id = store.create("Task", "backlog", None).await.unwrap();

        let (status, Json(body)) =
            move_task(state, Path(id.clone()), Json(json!({"column": "wip"}))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].is_string());
    }

    #[tokio::test]
    async fn test_move_task_not_found() {
        let (state, _) = state_with_store();
        let (status, _) = move_task(
            state,
            Path("ghost-id".into()),
            Json(json!({"column": "done"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_move_task_missing_column_field() {
        let (state, _) = state_with_store();
        let (status, Json(body)) = move_task(state, Path("t1".into()), Json(json!({}))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].is_string());
    }
}
