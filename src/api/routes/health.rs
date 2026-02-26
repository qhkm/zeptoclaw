//! Health endpoint for the panel API.

use axum::Json;
use serde_json::{json, Value};

/// GET /api/health — returns basic system health info.
pub async fn get_health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_health_returns_ok() {
        let Json(body) = get_health().await;
        assert_eq!(body["status"], "ok");
        assert!(body["version"].is_string());
    }
}
