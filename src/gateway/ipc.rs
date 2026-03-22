//! IPC protocol for containerized agent communication
//!
//! This module defines the request/response types for stdin/stdout
//! communication between the gateway and containerized agents.

use serde::{Deserialize, Serialize};

use std::sync::atomic::Ordering;

use crate::bus::InboundMessage;
use crate::config::AgentDefaults;
use crate::error::ZeptoError;
use crate::health::UsageMetrics;
use crate::session::Session;

/// Snapshot of usage counters returned from a containerized agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u64,
    pub errors: u64,
}

impl UsageSnapshot {
    /// Capture a snapshot from live `UsageMetrics`.
    pub fn from_metrics(metrics: &UsageMetrics) -> Self {
        Self {
            input_tokens: metrics.input_tokens.load(Ordering::Relaxed),
            output_tokens: metrics.output_tokens.load(Ordering::Relaxed),
            tool_calls: metrics.tool_calls.load(Ordering::Relaxed),
            errors: metrics.errors.load(Ordering::Relaxed),
        }
    }
}

/// Marker for start of response in stdout
pub const RESPONSE_START_MARKER: &str = "<<<AGENT_RESPONSE_START>>>";

/// Marker for end of response in stdout
pub const RESPONSE_END_MARKER: &str = "<<<AGENT_RESPONSE_END>>>";

/// Request sent to containerized agent via stdin.
///
/// Protocol fields intentionally include only execution-critical state:
/// request metadata, inbound message, agent defaults, and optional session snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Unique request identifier
    pub request_id: String,
    /// The inbound message to process
    pub message: InboundMessage,
    /// Agent configuration
    pub agent_config: AgentDefaults,
    /// Optional session state
    pub session: Option<Session>,
}

impl AgentRequest {
    /// Validate request consistency before execution.
    pub fn validate(&self) -> std::result::Result<(), ZeptoError> {
        if let Some(session) = &self.session {
            if session.key != self.message.session_key {
                return Err(ZeptoError::Session(format!(
                    "Session key mismatch: request.message.session_key='{}', request.session.key='{}'",
                    self.message.session_key, session.key
                )));
            }
        }

        Ok(())
    }
}

/// Response from containerized agent via stdout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Request identifier (matches AgentRequest.request_id)
    pub request_id: String,
    /// The result of processing
    pub result: AgentResult,
    /// Optional usage metrics snapshot from the agent process
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSnapshot>,
}

/// Result of agent processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentResult {
    /// Successful processing
    Success {
        /// Response content
        content: String,
        /// Updated session state
        session: Option<Session>,
    },
    /// Processing failed
    Error {
        /// Error message
        message: String,
        /// Error code
        code: String,
    },
}

impl AgentResponse {
    /// Create a success response
    pub fn success(request_id: &str, content: &str, session: Option<Session>) -> Self {
        Self {
            request_id: request_id.to_string(),
            result: AgentResult::Success {
                content: content.to_string(),
                session,
            },
            usage: None,
        }
    }

    /// Create an error response
    pub fn error(request_id: &str, message: &str, code: &str) -> Self {
        Self {
            request_id: request_id.to_string(),
            result: AgentResult::Error {
                message: message.to_string(),
                code: code.to_string(),
            },
            usage: None,
        }
    }

    /// Attach a usage snapshot to this response.
    pub fn with_usage(mut self, usage: UsageSnapshot) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Format response with markers for reliable parsing from stdout
    pub fn to_marked_json(&self) -> String {
        format!(
            "{}\n{}\n{}",
            RESPONSE_START_MARKER,
            serde_json::to_string(self).unwrap_or_default(),
            RESPONSE_END_MARKER
        )
    }
}

/// Parse response from marked stdout output
///
/// Extracts the JSON response between the start and end markers.
pub fn parse_marked_response(stdout: &str) -> Option<AgentResponse> {
    let start = stdout.rfind(RESPONSE_START_MARKER)?;
    let json_start = start + RESPONSE_START_MARKER.len();
    let end = stdout[json_start..].find(RESPONSE_END_MARKER)? + json_start;
    let json = stdout.get(json_start..end)?.trim();
    serde_json::from_str(json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_markers() {
        let response = AgentResponse::success("req-123", "Hello!", None);
        let marked = response.to_marked_json();

        assert!(marked.contains(RESPONSE_START_MARKER));
        assert!(marked.contains(RESPONSE_END_MARKER));
        assert!(marked.contains("req-123"));
        assert!(marked.contains("Hello!"));
    }

    #[test]
    fn test_parse_marked_response() {
        let response = AgentResponse::success("req-456", "Test output", None);
        let marked = response.to_marked_json();

        let parsed = parse_marked_response(&marked).unwrap();
        assert_eq!(parsed.request_id, "req-456");

        match parsed.result {
            AgentResult::Success { content, .. } => {
                assert_eq!(content, "Test output");
            }
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_parse_marked_response_with_noise() {
        let response = AgentResponse::success("test", "OK", None);
        let marked = response.to_marked_json();
        let noisy = format!("Log line 1\nLog line 2\n{}\nMore output", marked);

        let parsed = parse_marked_response(&noisy).unwrap();
        assert_eq!(parsed.request_id, "test");
    }

    #[test]
    fn test_parse_marked_response_uses_last_start_marker() {
        let first = AgentResponse::success("first", "old", None).to_marked_json();
        let second = AgentResponse::success("second", "new", None).to_marked_json();
        let payload = format!("{}\n{}", first, second);

        let parsed = parse_marked_response(&payload).unwrap();
        assert_eq!(parsed.request_id, "second");
    }

    #[test]
    fn test_error_response() {
        let response = AgentResponse::error("req-err", "Something went wrong", "ERR_001");
        let marked = response.to_marked_json();
        let parsed = parse_marked_response(&marked).unwrap();

        match parsed.result {
            AgentResult::Error { message, code } => {
                assert_eq!(message, "Something went wrong");
                assert_eq!(code, "ERR_001");
            }
            _ => panic!("Expected Error result"),
        }
    }

    #[test]
    fn test_request_validate_ok_without_session() {
        let request = AgentRequest {
            request_id: "req-1".to_string(),
            message: InboundMessage::new("test", "user1", "chat1", "Hello"),
            agent_config: AgentDefaults::default(),
            session: None,
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_request_validate_ok_with_matching_session_key() {
        let mut session = Session::new("test:chat1");
        session.summary = Some("seed".to_string());

        let request = AgentRequest {
            request_id: "req-2".to_string(),
            message: InboundMessage::new("test", "user1", "chat1", "Hello"),
            agent_config: AgentDefaults::default(),
            session: Some(session),
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_request_validate_rejects_mismatched_session_key() {
        let request = AgentRequest {
            request_id: "req-3".to_string(),
            message: InboundMessage::new("test", "user1", "chat1", "Hello"),
            agent_config: AgentDefaults::default(),
            session: Some(Session::new("test:chat999")),
        };

        let error = request.validate().expect_err("request should be invalid");
        assert!(matches!(error, ZeptoError::Session(_)));
    }

    #[test]
    fn test_response_with_usage() {
        let usage = UsageSnapshot {
            input_tokens: 100,
            output_tokens: 50,
            tool_calls: 3,
            errors: 0,
        };
        let response = AgentResponse::success("req-u", "OK", None).with_usage(usage);
        let marked = response.to_marked_json();
        let parsed = parse_marked_response(&marked).unwrap();

        let u = parsed.usage.expect("usage should be present");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.tool_calls, 3);
        assert_eq!(u.errors, 0);
    }

    #[test]
    fn test_response_without_usage_backward_compat() {
        // Responses without "usage" field should still parse (backward compat)
        let json = r#"{"request_id":"old","result":{"Success":{"content":"hi","session":null}}}"#;
        let parsed: AgentResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.usage.is_none());
    }
}
