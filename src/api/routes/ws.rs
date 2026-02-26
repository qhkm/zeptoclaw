//! WebSocket event streaming for the panel.

use crate::api::events::EventBus;
use crate::api::server::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use std::sync::Arc;

/// GET /ws/events — upgrades to WebSocket, streams PanelEvents as JSON.
pub async fn ws_events(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state.event_bus.clone()))
}

async fn handle_ws(mut socket: WebSocket, event_bus: EventBus) {
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

    #[test]
    fn test_ws_handler_compiles() {
        // Verify the handler signature is correct for axum routing
        let _: fn(WebSocketUpgrade, State<Arc<AppState>>) -> _ = |ws, state| ws_events(ws, state);
    }
}
