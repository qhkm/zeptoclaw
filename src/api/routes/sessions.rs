//! Session management routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

pub async fn list_sessions(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "sessions": [] }))
}

pub async fn get_session(
    State(_state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Json<Value> {
    Json(json!({ "key": key, "messages": [] }))
}

pub async fn delete_session(
    State(_state): State<Arc<AppState>>,
    Path(_key): Path<String>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let Json(body) = list_sessions(test_state()).await;
        assert!(body["sessions"].is_array());
    }

    #[tokio::test]
    async fn test_get_session() {
        let Json(body) = get_session(test_state(), Path("test:123".into())).await;
        assert_eq!(body["key"], "test:123");
    }

    #[tokio::test]
    async fn test_delete_session() {
        let status = delete_session(test_state(), Path("test:123".into())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
}
