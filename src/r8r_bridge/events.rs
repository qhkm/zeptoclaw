//! Bridge event types for r8r <-> ZeptoClaw communication.
//!
//! These types mirror the r8r bridge event schema so both sides can exchange
//! JSON messages over WebSocket without sharing a crate.  The wire format is
//! identical: a [`BridgeEventEnvelope`] wrapper whose `type` field selects the
//! variant, and `data` carries the variant-specific payload.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// BridgeEvent — all event variants (both directions)
// ---------------------------------------------------------------------------

/// All bridge event variants (both directions).
///
/// Each variant's inner fields are individually serializable.  The variant type
/// is determined by the `type` string on the envelope, not by serde tagging.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BridgeEvent {
    // r8r -> ZeptoClaw
    /// An approval gate has been reached and is waiting for a decision.
    ApprovalRequested {
        approval_id: String,
        workflow: String,
        execution_id: String,
        node_id: String,
        message: String,
        timeout_secs: u64,
        requester: Option<String>,
        context: Value,
    },

    /// An approval timed out without a decision.
    ApprovalTimeout {
        workflow: String,
        execution_id: String,
        node_id: String,
        elapsed_secs: u64,
    },

    /// A workflow execution completed successfully.
    ExecutionCompleted {
        workflow: String,
        execution_id: String,
        status: String,
        duration_ms: i64,
        node_count: usize,
    },

    /// A workflow execution failed.
    ExecutionFailed {
        workflow: String,
        execution_id: String,
        status: String,
        error_code: String,
        error_message: String,
        failed_node: String,
        duration_ms: i64,
    },

    /// Periodic health status report from r8r.
    HealthStatus {
        version: String,
        uptime_secs: u64,
        active_executions: usize,
        pending_approvals: usize,
        workflows_loaded: usize,
    },

    // ZeptoClaw -> r8r
    /// An approval decision from ZeptoClaw.
    ApprovalDecision {
        approval_id: String,
        execution_id: String,
        node_id: String,
        decision: String,
        reason: String,
        decided_by: String,
        channel: String,
    },

    /// A request from ZeptoClaw to trigger a workflow.
    WorkflowTrigger {
        workflow: String,
        params: Value,
        triggered_by: String,
        channel: String,
    },

    /// A health ping from ZeptoClaw (keepalive).
    HealthPing,
}

impl BridgeEvent {
    /// Returns the dotted type string for this event variant.
    pub fn event_type_str(&self) -> &'static str {
        match self {
            BridgeEvent::ApprovalRequested { .. } => "r8r.approval.requested",
            BridgeEvent::ApprovalTimeout { .. } => "r8r.approval.timeout",
            BridgeEvent::ExecutionCompleted { .. } => "r8r.execution.completed",
            BridgeEvent::ExecutionFailed { .. } => "r8r.execution.failed",
            BridgeEvent::HealthStatus { .. } => "r8r.health.status",
            BridgeEvent::ApprovalDecision { .. } => "zeptoclaw.approval.decision",
            BridgeEvent::WorkflowTrigger { .. } => "zeptoclaw.workflow.trigger",
            BridgeEvent::HealthPing => "zeptoclaw.health.ping",
        }
    }

    /// Reconstruct a `BridgeEvent` from its type string and data `Value`.
    ///
    /// This is the counterpart to `BridgeEventEnvelope::new()` which serializes
    /// the event into the `data` field.  Here we dispatch by type string to
    /// deserialize the correct variant.
    pub fn from_type_and_data(event_type: &str, data: &Value) -> Result<Self, String> {
        match event_type {
            "r8r.approval.requested" => {
                #[derive(Deserialize)]
                struct D {
                    approval_id: String,
                    workflow: String,
                    execution_id: String,
                    node_id: String,
                    message: String,
                    timeout_secs: u64,
                    requester: Option<String>,
                    context: Value,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize ApprovalRequested: {e}"))?;
                Ok(BridgeEvent::ApprovalRequested {
                    approval_id: d.approval_id,
                    workflow: d.workflow,
                    execution_id: d.execution_id,
                    node_id: d.node_id,
                    message: d.message,
                    timeout_secs: d.timeout_secs,
                    requester: d.requester,
                    context: d.context,
                })
            }
            "r8r.approval.timeout" => {
                #[derive(Deserialize)]
                struct D {
                    workflow: String,
                    execution_id: String,
                    node_id: String,
                    elapsed_secs: u64,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize ApprovalTimeout: {e}"))?;
                Ok(BridgeEvent::ApprovalTimeout {
                    workflow: d.workflow,
                    execution_id: d.execution_id,
                    node_id: d.node_id,
                    elapsed_secs: d.elapsed_secs,
                })
            }
            "r8r.execution.completed" => {
                #[derive(Deserialize)]
                struct D {
                    workflow: String,
                    execution_id: String,
                    status: String,
                    duration_ms: i64,
                    node_count: usize,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize ExecutionCompleted: {e}"))?;
                Ok(BridgeEvent::ExecutionCompleted {
                    workflow: d.workflow,
                    execution_id: d.execution_id,
                    status: d.status,
                    duration_ms: d.duration_ms,
                    node_count: d.node_count,
                })
            }
            "r8r.execution.failed" => {
                #[derive(Deserialize)]
                struct D {
                    workflow: String,
                    execution_id: String,
                    status: String,
                    error_code: String,
                    error_message: String,
                    failed_node: String,
                    duration_ms: i64,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize ExecutionFailed: {e}"))?;
                Ok(BridgeEvent::ExecutionFailed {
                    workflow: d.workflow,
                    execution_id: d.execution_id,
                    status: d.status,
                    error_code: d.error_code,
                    error_message: d.error_message,
                    failed_node: d.failed_node,
                    duration_ms: d.duration_ms,
                })
            }
            "r8r.health.status" => {
                #[derive(Deserialize)]
                struct D {
                    version: String,
                    uptime_secs: u64,
                    active_executions: usize,
                    pending_approvals: usize,
                    workflows_loaded: usize,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize HealthStatus: {e}"))?;
                Ok(BridgeEvent::HealthStatus {
                    version: d.version,
                    uptime_secs: d.uptime_secs,
                    active_executions: d.active_executions,
                    pending_approvals: d.pending_approvals,
                    workflows_loaded: d.workflows_loaded,
                })
            }
            "zeptoclaw.approval.decision" => {
                #[derive(Deserialize)]
                struct D {
                    approval_id: String,
                    execution_id: String,
                    node_id: String,
                    decision: String,
                    reason: String,
                    decided_by: String,
                    channel: String,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize ApprovalDecision: {e}"))?;
                Ok(BridgeEvent::ApprovalDecision {
                    approval_id: d.approval_id,
                    execution_id: d.execution_id,
                    node_id: d.node_id,
                    decision: d.decision,
                    reason: d.reason,
                    decided_by: d.decided_by,
                    channel: d.channel,
                })
            }
            "zeptoclaw.workflow.trigger" => {
                #[derive(Deserialize)]
                struct D {
                    workflow: String,
                    params: Value,
                    triggered_by: String,
                    channel: String,
                }
                let d: D = serde_json::from_value(data.clone())
                    .map_err(|e| format!("Failed to deserialize WorkflowTrigger: {e}"))?;
                Ok(BridgeEvent::WorkflowTrigger {
                    workflow: d.workflow,
                    params: d.params,
                    triggered_by: d.triggered_by,
                    channel: d.channel,
                })
            }
            "zeptoclaw.health.ping" => Ok(BridgeEvent::HealthPing),
            unknown => Err(format!("Unknown bridge event type: {unknown}")),
        }
    }
}

// ---------------------------------------------------------------------------
// BridgeEventEnvelope — JSON wire wrapper
// ---------------------------------------------------------------------------

/// JSON envelope wrapping every bridge event.
///
/// The `type` field identifies the event variant, while `data` contains the
/// serialized event payload.  This design avoids `#[serde(untagged)]` issues
/// where overlapping fields cause silent misdeserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeEventEnvelope {
    /// Unique event identifier (format: `evt_<uuid>`).
    pub id: String,

    /// Dotted event type string (e.g. `r8r.approval.requested`).
    #[serde(rename = "type")]
    pub event_type: String,

    /// ISO-8601 timestamp of when the event was created.
    pub timestamp: DateTime<Utc>,

    /// Event payload (variant-specific fields).
    pub data: Value,

    /// Optional correlation ID for request tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl BridgeEventEnvelope {
    /// Create a new envelope for the given event.
    ///
    /// Serializes the event into the `data` field via `serde_json::to_value()`.
    /// The `type` field is derived from the event variant.
    pub fn new(event: BridgeEvent, correlation_id: Option<String>) -> Self {
        let event_type = event.event_type_str().to_string();
        let data = match &event {
            BridgeEvent::HealthPing => Value::Object(serde_json::Map::new()),
            _ => serde_json::to_value(&event).unwrap_or(Value::Null),
        };
        Self {
            id: format!("evt_{}", Uuid::new_v4()),
            event_type,
            timestamp: Utc::now(),
            data,
            correlation_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Ack — simple acknowledgment
// ---------------------------------------------------------------------------

/// Simple acknowledgment message for received events.
///
/// Serializes with `"type": "ack"` automatically — callers only set `event_id`.
#[derive(Debug, Clone)]
pub struct Ack {
    /// The ID of the event being acknowledged.
    pub event_id: String,
}

impl Serialize for Ack {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("Ack", 2)?;
        s.serialize_field("type", "ack")?;
        s.serialize_field("event_id", &self.event_id)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for Ack {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct AckHelper {
            #[serde(rename = "type")]
            _type: String,
            event_id: String,
        }
        let helper = AckHelper::deserialize(deserializer)?;
        if helper._type != "ack" {
            return Err(serde::de::Error::custom(format!(
                "expected type \"ack\", got \"{}\"",
                helper._type
            )));
        }
        Ok(Ack {
            event_id: helper.event_id,
        })
    }
}
