//! Tests for r8r bridge event deserialization and deduplication.

use zeptoclaw::r8r_bridge::dedup::Deduplicator;
use zeptoclaw::r8r_bridge::events::{BridgeEvent, BridgeEventEnvelope};

#[test]
fn test_event_envelope_deserialization() {
    let json = r#"{
        "id": "evt_abc123",
        "type": "r8r.approval.requested",
        "timestamp": "2026-03-14T12:00:00Z",
        "data": {
            "approval_id": "apr_001",
            "workflow": "deploy-prod",
            "execution_id": "exec_42",
            "node_id": "approve-deploy",
            "message": "Deploy to production?",
            "timeout_secs": 300,
            "requester": "ci-bot",
            "context": {"env": "production"}
        },
        "correlation_id": "corr_99"
    }"#;

    let envelope: BridgeEventEnvelope = serde_json::from_str(json).unwrap();
    assert_eq!(envelope.id, "evt_abc123");
    assert_eq!(envelope.event_type, "r8r.approval.requested");
    assert_eq!(envelope.correlation_id.as_deref(), Some("corr_99"));

    // Reconstruct the typed event from the envelope
    let event = BridgeEvent::from_type_and_data(&envelope.event_type, &envelope.data).unwrap();
    match event {
        BridgeEvent::ApprovalRequested {
            approval_id,
            workflow,
            execution_id,
            node_id,
            message,
            timeout_secs,
            requester,
            ..
        } => {
            assert_eq!(approval_id, "apr_001");
            assert_eq!(workflow, "deploy-prod");
            assert_eq!(execution_id, "exec_42");
            assert_eq!(node_id, "approve-deploy");
            assert_eq!(message, "Deploy to production?");
            assert_eq!(timeout_secs, 300);
            assert_eq!(requester.as_deref(), Some("ci-bot"));
        }
        other => panic!("Expected ApprovalRequested, got: {:?}", other),
    }
}

#[test]
fn test_dedup_skips_duplicate() {
    let mut dedup = Deduplicator::new(200, 600);

    // First time — new
    assert!(dedup.is_new("evt_abc123"));

    // Second time — duplicate
    assert!(!dedup.is_new("evt_abc123"));

    // Different ID — new
    assert!(dedup.is_new("evt_def456"));
}

#[test]
fn test_dedup_evicts_oldest() {
    let mut dedup = Deduplicator::new(3, 600);

    assert!(dedup.is_new("evt_1"));
    assert!(dedup.is_new("evt_2"));
    assert!(dedup.is_new("evt_3"));

    // At capacity (3).  Adding evt_4 should evict evt_1.
    assert!(dedup.is_new("evt_4"));

    // evt_1 was evicted — it is "new" again
    assert!(dedup.is_new("evt_1"));

    // evt_2 was evicted when evt_1 was re-added
    assert!(dedup.is_new("evt_2"));

    // evt_3 was evicted when evt_2 was re-added
    assert!(dedup.is_new("evt_3"));

    // evt_4 should still be remembered (it was added most recently after evictions)
    // After the sequence: [evt_4, evt_1, evt_2] -> add evt_3 evicts evt_4
    // So evt_4 is now evicted:
    assert!(dedup.is_new("evt_4"));
}

#[test]
fn test_envelope_round_trip() {
    let event = BridgeEvent::ApprovalDecision {
        approval_id: "apr_001".into(),
        execution_id: "exec_42".into(),
        node_id: "approve-deploy".into(),
        decision: "approved".into(),
        reason: "LGTM".into(),
        decided_by: "admin".into(),
        channel: "telegram".into(),
    };

    let envelope = BridgeEventEnvelope::new(event, Some("corr_99".into()));
    assert_eq!(envelope.event_type, "zeptoclaw.approval.decision");
    assert!(envelope.id.starts_with("evt_"));
    assert_eq!(envelope.correlation_id.as_deref(), Some("corr_99"));

    // Serialize and deserialize the full envelope
    let json = serde_json::to_string(&envelope).unwrap();
    let parsed: BridgeEventEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.event_type, "zeptoclaw.approval.decision");

    let reconstructed = BridgeEvent::from_type_and_data(&parsed.event_type, &parsed.data).unwrap();
    match reconstructed {
        BridgeEvent::ApprovalDecision {
            decision, reason, ..
        } => {
            assert_eq!(decision, "approved");
            assert_eq!(reason, "LGTM");
        }
        other => panic!("Expected ApprovalDecision, got: {:?}", other),
    }
}

#[test]
fn test_health_ping_event() {
    let event = BridgeEvent::HealthPing;
    let envelope = BridgeEventEnvelope::new(event, None);
    assert_eq!(envelope.event_type, "zeptoclaw.health.ping");
    assert!(envelope.correlation_id.is_none());

    let json = serde_json::to_string(&envelope).unwrap();
    let parsed: BridgeEventEnvelope = serde_json::from_str(&json).unwrap();

    let reconstructed = BridgeEvent::from_type_and_data(&parsed.event_type, &parsed.data).unwrap();
    assert!(matches!(reconstructed, BridgeEvent::HealthPing));
}

#[test]
fn test_ack_serialization() {
    use zeptoclaw::r8r_bridge::events::Ack;

    let ack = Ack {
        event_id: "evt_abc123".into(),
    };
    let json = serde_json::to_string(&ack).unwrap();
    assert!(json.contains(r#""type":"ack""#));
    assert!(json.contains(r#""event_id":"evt_abc123""#));

    let parsed: Ack = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.event_id, "evt_abc123");
}
