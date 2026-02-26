//! Session management routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::api::server::AppState;

/// Compact session summary returned in list responses.
#[derive(Serialize)]
struct SessionSummary {
    key: String,
    message_count: usize,
    created_at: String,
    updated_at: String,
}

pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Value> {
    let Some(ref manager) = state.session_manager else {
        return Json(json!({ "sessions": [] }));
    };

    let keys = match manager.list().await {
        Ok(k) => k,
        Err(_) => return Json(json!({ "sessions": [] })),
    };

    let mut summaries = Vec::with_capacity(keys.len());
    for key in &keys {
        if let Ok(Some(session)) = manager.get(key).await {
            summaries.push(SessionSummary {
                key: session.key.clone(),
                message_count: session.messages.len(),
                created_at: session.created_at.to_rfc3339(),
                updated_at: session.updated_at.to_rfc3339(),
            });
        }
    }

    Json(
        serde_json::to_value(json!({ "sessions": summaries }))
            .unwrap_or_else(|_| json!({ "sessions": [] })),
    )
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> (StatusCode, Json<Value>) {
    let Some(ref manager) = state.session_manager else {
        return (StatusCode::OK, Json(json!({ "key": key, "messages": [] })));
    };

    match manager.get(&key).await {
        Ok(Some(session)) => {
            let body = serde_json::to_value(&session)
                .unwrap_or_else(|_| json!({ "key": key, "messages": [] }));
            (StatusCode::OK, Json(body))
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to load session" })),
        ),
    }
}

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> StatusCode {
    let Some(ref manager) = state.session_manager else {
        return StatusCode::NO_CONTENT;
    };

    match manager.delete(&key).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::events::EventBus;

    fn test_state() -> State<Arc<AppState>> {
        State(Arc::new(AppState::new("tok".into(), EventBus::new(16))))
    }

    #[tokio::test]
    async fn test_list_sessions_no_manager() {
        let Json(body) = list_sessions(test_state()).await;
        assert!(body["sessions"].is_array());
        assert_eq!(body["sessions"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_get_session_no_manager() {
        let (status, Json(body)) = get_session(test_state(), Path("test:123".into())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["key"], "test:123");
    }

    #[tokio::test]
    async fn test_delete_session_no_manager() {
        let status = delete_session(test_state(), Path("test:123".into())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_list_sessions_with_manager() {
        use crate::session::SessionManager;

        let manager = Arc::new(SessionManager::new_memory());
        manager.get_or_create("chan:1").await.unwrap();
        manager.get_or_create("chan:2").await.unwrap();

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.session_manager = Some(manager);

        let Json(body) = list_sessions(State(Arc::new(state))).await;
        let sessions = body["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_get_session_found() {
        use crate::session::SessionManager;

        let manager = Arc::new(SessionManager::new_memory());
        manager.get_or_create("chan:42").await.unwrap();

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.session_manager = Some(manager);

        let (status, Json(body)) =
            get_session(State(Arc::new(state)), Path("chan:42".into())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["key"], "chan:42");
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        use crate::session::SessionManager;

        let manager = Arc::new(SessionManager::new_memory());

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.session_manager = Some(manager);

        let (status, _) = get_session(State(Arc::new(state)), Path("missing:99".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_session_with_manager() {
        use crate::session::SessionManager;

        let manager = Arc::new(SessionManager::new_memory());
        manager.get_or_create("del:1").await.unwrap();

        let mut state = AppState::new("tok".into(), EventBus::new(16));
        state.session_manager = Some(manager.clone());

        let status = delete_session(State(Arc::new(state)), Path("del:1".into())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!manager.exists("del:1").await);
    }
}
