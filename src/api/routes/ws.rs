//! WebSocket event streaming for the panel.

use crate::api::events::EventBus;
use crate::api::server::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use std::collections::HashMap;
use std::sync::Arc;

/// GET /ws/events — upgrades to WebSocket, streams PanelEvents as JSON.
///
/// Authentication: Browsers first obtain a short-lived, single-use ticket from
/// the authenticated `POST /api/auth/ws-ticket` endpoint, then supply it as the
/// `?ticket=<ticket>` query parameter. The long-lived API token or JWT never
/// appears in the WebSocket URL.
///
/// Enforces a hard cap of [`AppState::MAX_WS_CONNECTIONS`] concurrent
/// WebSocket connections via a semaphore stored in [`AppState`].  When the
/// cap is reached the handler responds with HTTP 503 before the upgrade so
/// the client gets a meaningful error rather than a silent hang.
pub async fn ws_events(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    // Consume the one-time ticket before upgrading.
    let is_authenticated = match params.get("ticket") {
        Some(ticket) => state.ws_tickets.consume(ticket).await,
        None => false,
    };

    if !is_authenticated {
        return axum::response::Response::builder()
            .status(axum::http::StatusCode::UNAUTHORIZED)
            .body(axum::body::Body::from(
                "Missing or invalid WebSocket ticket",
            ))
            .expect("response build is infallible");
    }

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
    // Exercise the authentication handshake against a real loopback server;
    // the extractor requires Hyper's live connection-upgrade extension.
    use super::*;
    use crate::api::server::AppState;
    use axum::http::StatusCode;

    async fn spawn_ws_app(state: AppState) -> std::net::SocketAddr {
        let app = crate::api::server::build_router(state, None, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let addr = listener
            .local_addr()
            .expect("listener must have an address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server must run");
        });
        addr
    }

    #[test]
    fn test_ws_handler_compiles() {
        use axum::extract::Query;
        use std::collections::HashMap;
        // Verify the handler signature is correct for axum routing.
        let _: fn(WebSocketUpgrade, State<Arc<AppState>>, Query<HashMap<String, String>>) -> _ =
            |ws, state, query| ws_events(ws, state, query);
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

    #[tokio::test]
    async fn test_ws_upgrade_consumes_ticket_once() {
        let bus = EventBus::new(8);
        let state = AppState::new("static-token".into(), bus);
        let ticket = state.ws_tickets.issue().await;
        let addr = spawn_ws_app(state).await;
        let url = format!("ws://{addr}/ws/events?ticket={ticket}");

        let (mut socket, response) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("valid ticket must upgrade");
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        socket.close(None).await.expect("socket must close");

        let replay = tokio_tungstenite::connect_async(&url)
            .await
            .expect_err("consumed ticket must be rejected");
        match replay {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected HTTP rejection, got {other}"),
        }
    }

    #[tokio::test]
    async fn test_ws_upgrade_rejects_long_lived_token_query() {
        let bus = EventBus::new(8);
        let state = AppState::new("static-token".into(), bus);
        let addr = spawn_ws_app(state).await;

        let result =
            tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events?auth=static-token"))
                .await
                .expect_err("long-lived token query must be rejected");
        match result {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected HTTP rejection, got {other}"),
        }
    }
}
