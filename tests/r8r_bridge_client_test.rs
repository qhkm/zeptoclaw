//! Integration tests for the R8rBridge WebSocket client.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use zeptoclaw::r8r_bridge::{Ack, BridgeEvent, BridgeEventEnvelope, R8rBridge};

/// Start a mock WebSocket server that:
/// 1. Accepts a single connection.
/// 2. Reads messages and forwards them to the returned mpsc receiver.
/// 3. When it receives a health ping, responds with a HealthStatus event.
///
/// Returns `(addr, receiver)` where `addr` is the bound address string.
async fn start_mock_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://127.0.0.1:{}", addr.port());

    let (tx, rx) = mpsc::channel::<String>(32);

    tokio::spawn(async move {
        // Accept one connection.
        let (stream, _) = listener.accept().await.unwrap();
        let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut ws_write, mut ws_read) = ws_stream.split();

        while let Some(Ok(msg)) = ws_read.next().await {
            if let WsMessage::Text(text) = msg {
                let text_str = text.to_string();

                // Forward to the test via mpsc.
                let _ = tx.send(text_str.clone()).await;

                // If it's a health ping, respond with a HealthStatus event.
                if let Ok(parsed) = serde_json::from_str::<Value>(&text_str) {
                    if parsed.get("type").and_then(|t| t.as_str()) == Some("zeptoclaw.health.ping")
                    {
                        let health_event = BridgeEvent::HealthStatus {
                            version: "0.1.0-test".to_string(),
                            uptime_secs: 42,
                            active_executions: 1,
                            pending_approvals: 0,
                            workflows_loaded: 3,
                        };
                        let envelope = BridgeEventEnvelope::new(health_event, None);
                        let json = serde_json::to_string(&envelope).unwrap();
                        let _ = ws_write.send(WsMessage::Text(json.into())).await;
                    }
                }
            }
        }
    });

    (ws_url, rx)
}

#[tokio::test]
async fn test_bridge_connects_and_sends_health_ping() {
    let (ws_url, mut rx) = start_mock_server().await;

    let bridge = R8rBridge::new(ws_url, None);

    // Connect to the mock server.
    bridge.connect().await.expect("connect should succeed");
    assert!(bridge.is_connected(), "bridge should be connected");

    // Send a health ping.
    bridge
        .send_health_ping()
        .await
        .expect("send_health_ping should succeed");

    // Verify the mock server received the ping.
    let received = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive message within timeout")
        .expect("channel should not be closed");

    let parsed: Value = serde_json::from_str(&received).unwrap();
    assert_eq!(
        parsed.get("type").and_then(|t| t.as_str()),
        Some("zeptoclaw.health.ping"),
        "mock server should have received a health ping"
    );

    // Wait briefly for the bridge to receive the health status response.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The mock server echoes a health status; we may also receive the ack first.
    // Drain the ack message from rx (the bridge sends an ack for the health status).
    // The health status should now be stored.
    let health = bridge.last_health_status().await;
    assert!(health.is_some(), "bridge should have stored health status");

    if let Some(BridgeEvent::HealthStatus {
        version,
        uptime_secs,
        ..
    }) = health
    {
        assert_eq!(version, "0.1.0-test");
        assert_eq!(uptime_secs, 42);
    } else {
        panic!("Expected HealthStatus variant");
    }

    // Clean up.
    bridge.disconnect().await;
    assert!(!bridge.is_connected(), "bridge should be disconnected");
}

#[tokio::test]
async fn test_bridge_acks_duplicate_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://127.0.0.1:{}", addr.port());
    let (tx, mut rx) = mpsc::channel::<String>(8);

    let duplicate_event = BridgeEventEnvelope::new(
        BridgeEvent::HealthStatus {
            version: "0.1.0-test".to_string(),
            uptime_secs: 42,
            active_executions: 1,
            pending_approvals: 0,
            workflows_loaded: 3,
        },
        None,
    );
    let duplicate_event_id = duplicate_event.id.clone();
    let duplicate_json = serde_json::to_string(&duplicate_event).unwrap();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();

        ws_stream
            .send(WsMessage::Text(duplicate_json.clone().into()))
            .await
            .unwrap();
        ws_stream
            .send(WsMessage::Text(duplicate_json.into()))
            .await
            .unwrap();

        for _ in 0..2 {
            if let Some(Ok(WsMessage::Text(text))) = ws_stream.next().await {
                let _ = tx.send(text.to_string()).await;
            }
        }
    });

    let bridge = R8rBridge::new(ws_url, None);
    bridge.connect().await.expect("connect should succeed");

    let ack_one = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive first ack")
        .expect("first ack channel item");
    let ack_two = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive second ack")
        .expect("second ack channel item");

    let ack_one: Ack = serde_json::from_str(&ack_one).unwrap();
    let ack_two: Ack = serde_json::from_str(&ack_two).unwrap();
    assert_eq!(ack_one.event_id, duplicate_event_id);
    assert_eq!(ack_two.event_id, duplicate_event_id);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(matches!(
        bridge.last_health_status().await,
        Some(BridgeEvent::HealthStatus { .. })
    ));

    bridge.disconnect().await;
}
