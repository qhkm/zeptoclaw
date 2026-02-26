//! WebSocket event streaming for the panel.

use crate::api::events::EventBus;
use crate::api::server::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use std::sync::Arc;

/// GET /ws/events — upgrades to WebSocket, streams PanelEvents as JSON.
///
/// Enforces a hard cap of [`AppState::MAX_WS_CONNECTIONS`] concurrent
/// WebSocket connections via a semaphore stored in [`AppState`].  When the
/// cap is reached the handler responds with HTTP 503 before the upgrade so
/// the client gets a meaningful error rather than a silent hang.
pub async fn ws_events(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    // Try to acquire a connection slot.  `try_acquire_owned` is non-blocking:
    // it either succeeds immediately or returns `TryAcquireError::NoPermits`.
    let permit = match state.ws_semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                .body(axum::body::Body::from("Too many WebSocket connections"))
                .expect("response build is infallible")
        }
    };

    let event_bus = state.event_bus.clone();
    // Move the permit into the connection task so it is dropped (released)
    // only when the WebSocket connection closes.
    ws.on_upgrade(move |socket| handle_ws(socket, event_bus, permit))
}

async fn handle_ws(
    mut socket: WebSocket,
    event_bus: EventBus,
    // Held for the lifetime of the connection; dropped when this future
    // resolves, which releases the semaphore permit.
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let mut rx = event_bus.subscribe();
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(e) => {
                        let json = match serde_json::to_string(&e) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // Ignore client messages for now
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // WebSocket handlers are hard to unit test directly.
    // Integration tests will cover the WS upgrade + event flow.
    // Here we test that the handler function exists and compiles.
    use super::*;
    use crate::api::server::AppState;

    #[test]
    fn test_ws_handler_compiles() {
        // Verify the handler signature is correct for axum routing.
        let _: fn(WebSocketUpgrade, State<Arc<AppState>>) -> _ = |ws, state| ws_events(ws, state);
    }

    #[test]
    fn test_ws_semaphore_exhaustion_reduces_permits() {
        // Verify that acquiring all permits leaves the semaphore at zero.
        let sem = Arc::new(tokio::sync::Semaphore::new(AppState::MAX_WS_CONNECTIONS));
        let mut permits = Vec::new();
        for _ in 0..AppState::MAX_WS_CONNECTIONS {
            permits.push(sem.clone().try_acquire_owned().expect("permit available"));
        }
        assert_eq!(sem.available_permits(), 0);
        // The next acquire should fail.
        assert!(sem.clone().try_acquire_owned().is_err());
        // Releasing one permit makes room again.
        drop(permits.pop());
        assert_eq!(sem.available_permits(), 1);
    }
}
