//! Generic NVS (Non-Volatile Storage) tools for ESP32.
//!
//! NVS is ESP-IDF's flash-based key-value store, used by any board with
//! `has_nvs: true` in its [`BoardProfile`].  These tools do **not** require
//! a `BoardProfile` reference — NVS operations do not need pin validation.
//!
//! # Protocol
//!
//! Commands are sent via [`SerialTransport::request`] as newline-delimited JSON:
//!
//! - `nvs_get`    — read a value by namespace + key
//! - `nvs_set`    — write a value by namespace + key
//! - `nvs_delete` — delete a key from a namespace
//!
//! # ESP-IDF constraints
//!
//! - Namespace and key names are limited to 15 bytes (NVS_KEY_NAME_MAX_SIZE).
//! - Names may contain only alphanumeric characters, `_`, and `-`.
//!
//! This module is only compiled when the `hardware` feature is enabled.

use super::serial::SerialTransport;
use crate::error::{Result, ZeptoError};
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Constants (ESP-IDF limits)
// ---------------------------------------------------------------------------

/// Maximum length of an NVS key or namespace name (ESP-IDF NVS_KEY_NAME_MAX_SIZE).
pub const MAX_KEY_LEN: usize = 15;

/// Maximum length of an NVS namespace name (same limit as key).
pub const MAX_NAMESPACE_LEN: usize = 15;

/// Maximum length of an NVS string value (ESP-IDF NVS blob limit for strings).
pub const MAX_VALUE_LEN: usize = 4000;

/// Default NVS namespace used when the caller omits the parameter.
const DEFAULT_NAMESPACE: &str = "config";

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

/// Validate an NVS namespace or key string.
///
/// Rules (mirror ESP-IDF constraints):
/// - Must not be empty.
/// - Must not exceed `max_len` bytes.
/// - May only contain ASCII alphanumeric characters, `_`, or `-`.
fn validate_nvs_string(value: &str, field: &str, max_len: usize) -> Result<()> {
    if value.is_empty() {
        return Err(ZeptoError::Tool(format!("NVS {field} must not be empty")));
    }
    if value.len() > max_len {
        return Err(ZeptoError::Tool(format!(
            "NVS {field} '{}' exceeds maximum length of {} bytes (got {})",
            value,
            max_len,
            value.len()
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ZeptoError::Tool(format!(
            "NVS {field} '{}' contains invalid characters; only alphanumeric, '_', and '-' are allowed",
            value
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// NvsGetTool
// ---------------------------------------------------------------------------

/// Tool: read a value from NVS by namespace and key.
pub struct NvsGetTool {
    pub(crate) transport: Arc<SerialTransport>,
}

#[async_trait]
impl Tool for NvsGetTool {
    fn name(&self) -> &str {
        "nvs_get"
    }

    fn description(&self) -> &str {
        "Read a value from ESP32 NVS (non-volatile flash storage) by namespace and key. \
         Returns the stored string value, or an error if the key does not exist."
    }

    fn compact_description(&self) -> &str {
        "Read NVS key value"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "NVS namespace (default: \"config\", max 15 chars, alphanumeric/_/-)"
                },
                "key": {
                    "type": "string",
                    "description": "NVS key name (required, max 15 chars, alphanumeric/_/-)"
                }
            },
            "required": ["key"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let namespace = args
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_NAMESPACE);
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'key'".into()))?;

        validate_nvs_string(namespace, "namespace", MAX_NAMESPACE_LEN)?;
        validate_nvs_string(key, "key", MAX_KEY_LEN)?;

        let result = self
            .transport
            .request("nvs_get", json!({ "namespace": namespace, "key": key }))
            .await?;

        Ok(ToolOutput::llm_only(result))
    }
}

// ---------------------------------------------------------------------------
// NvsSetTool
// ---------------------------------------------------------------------------

/// Tool: write a value to NVS by namespace and key.
pub struct NvsSetTool {
    pub(crate) transport: Arc<SerialTransport>,
}

#[async_trait]
impl Tool for NvsSetTool {
    fn name(&self) -> &str {
        "nvs_set"
    }

    fn description(&self) -> &str {
        "Write a string value to ESP32 NVS (non-volatile flash storage). \
         Creates the key if it does not exist; overwrites if it does."
    }

    fn compact_description(&self) -> &str {
        "Write NVS key value"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "NVS namespace (default: \"config\", max 15 chars, alphanumeric/_/-)"
                },
                "key": {
                    "type": "string",
                    "description": "NVS key name (required, max 15 chars, alphanumeric/_/-)"
                },
                "value": {
                    "type": "string",
                    "description": "String value to store in NVS"
                }
            },
            "required": ["key", "value"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let namespace = args
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_NAMESPACE);
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'key'".into()))?;
        let value = args
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'value'".into()))?;

        validate_nvs_string(namespace, "namespace", MAX_NAMESPACE_LEN)?;
        validate_nvs_string(key, "key", MAX_KEY_LEN)?;

        if value.len() > MAX_VALUE_LEN {
            return Err(ZeptoError::Tool(format!(
                "NVS value exceeds maximum length of {} bytes (got {})",
                MAX_VALUE_LEN,
                value.len()
            )));
        }

        let result = self
            .transport
            .request(
                "nvs_set",
                json!({ "namespace": namespace, "key": key, "value": value }),
            )
            .await?;

        Ok(ToolOutput::llm_only(result))
    }
}

// ---------------------------------------------------------------------------
// NvsDeleteTool
// ---------------------------------------------------------------------------

/// Tool: delete a key from NVS.
pub struct NvsDeleteTool {
    pub(crate) transport: Arc<SerialTransport>,
}

#[async_trait]
impl Tool for NvsDeleteTool {
    fn name(&self) -> &str {
        "nvs_delete"
    }

    fn description(&self) -> &str {
        "Delete a key from ESP32 NVS (non-volatile flash storage). \
         Returns an error if the namespace or key does not exist."
    }

    fn compact_description(&self) -> &str {
        "Delete NVS key"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "NVS namespace (default: \"config\", max 15 chars, alphanumeric/_/-)"
                },
                "key": {
                    "type": "string",
                    "description": "NVS key name to delete (required, max 15 chars, alphanumeric/_/-)"
                }
            },
            "required": ["key"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let namespace = args
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_NAMESPACE);
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'key'".into()))?;

        validate_nvs_string(namespace, "namespace", MAX_NAMESPACE_LEN)?;
        validate_nvs_string(key, "key", MAX_KEY_LEN)?;

        let result = self
            .transport
            .request("nvs_delete", json!({ "namespace": namespace, "key": key }))
            .await?;

        Ok(ToolOutput::llm_only(result))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // validate_nvs_string — valid inputs
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_nvs_string_valid_key() {
        assert!(validate_nvs_string("wifi_ssid", "key", MAX_KEY_LEN).is_ok());
        assert!(validate_nvs_string("k", "key", MAX_KEY_LEN).is_ok());
        assert!(validate_nvs_string("key-1", "key", MAX_KEY_LEN).is_ok());
        assert!(validate_nvs_string("KEY_123", "key", MAX_KEY_LEN).is_ok());
    }

    #[test]
    fn test_validate_nvs_string_exactly_max_len() {
        // 15 chars — at the limit, should pass.
        let at_limit = "a".repeat(MAX_KEY_LEN);
        assert_eq!(at_limit.len(), MAX_KEY_LEN);
        assert!(validate_nvs_string(&at_limit, "key", MAX_KEY_LEN).is_ok());
    }

    // -----------------------------------------------------------------------
    // validate_nvs_string — empty string
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_nvs_string_empty_key() {
        let err = validate_nvs_string("", "key", MAX_KEY_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty"),
            "expected 'empty' in error, got: {msg}"
        );
    }

    #[test]
    fn test_validate_nvs_string_empty_namespace() {
        let err = validate_nvs_string("", "namespace", MAX_NAMESPACE_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty"),
            "expected 'empty' in error, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // validate_nvs_string — too long
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_nvs_string_too_long_key() {
        // 16 chars — one over the limit.
        let too_long = "a".repeat(MAX_KEY_LEN + 1);
        let err = validate_nvs_string(&too_long, "key", MAX_KEY_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds") || msg.contains("maximum"),
            "expected length error, got: {msg}"
        );
    }

    #[test]
    fn test_validate_nvs_string_too_long_namespace() {
        let too_long = "n".repeat(MAX_NAMESPACE_LEN + 1);
        let err = validate_nvs_string(&too_long, "namespace", MAX_NAMESPACE_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds") || msg.contains("maximum"),
            "expected length error, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // validate_nvs_string — invalid characters
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_nvs_string_invalid_chars_space() {
        let err = validate_nvs_string("my key", "key", MAX_KEY_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid"),
            "expected 'invalid' in error, got: {msg}"
        );
    }

    #[test]
    fn test_validate_nvs_string_invalid_chars_dot() {
        let err = validate_nvs_string("my.key", "key", MAX_KEY_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid"),
            "expected 'invalid' in error, got: {msg}"
        );
    }

    #[test]
    fn test_validate_nvs_string_invalid_chars_slash() {
        let err = validate_nvs_string("my/key", "key", MAX_KEY_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid"),
            "expected 'invalid' in error, got: {msg}"
        );
    }

    #[test]
    fn test_validate_nvs_string_invalid_chars_at() {
        let err = validate_nvs_string("key@host", "key", MAX_KEY_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid"),
            "expected 'invalid' in error, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    #[test]
    fn test_max_key_len_is_15() {
        assert_eq!(MAX_KEY_LEN, 15);
    }

    #[test]
    fn test_max_namespace_len_is_15() {
        assert_eq!(MAX_NAMESPACE_LEN, 15);
    }

    #[test]
    fn test_max_value_len_is_4000() {
        assert_eq!(MAX_VALUE_LEN, 4000);
    }

    // -----------------------------------------------------------------------
    // Parameter schema tests (no SerialTransport needed)
    // -----------------------------------------------------------------------

    fn get_parameters() -> Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "NVS namespace (default: \"config\", max 15 chars, alphanumeric/_/-)"
                },
                "key": {
                    "type": "string",
                    "description": "NVS key name (required, max 15 chars, alphanumeric/_/-)"
                }
            },
            "required": ["key"]
        })
    }

    fn set_parameters() -> Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "NVS namespace (default: \"config\", max 15 chars, alphanumeric/_/-)"
                },
                "key": {
                    "type": "string",
                    "description": "NVS key name (required, max 15 chars, alphanumeric/_/-)"
                },
                "value": {
                    "type": "string",
                    "description": "String value to store in NVS"
                }
            },
            "required": ["key", "value"]
        })
    }

    fn delete_parameters() -> Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "NVS namespace (default: \"config\", max 15 chars, alphanumeric/_/-)"
                },
                "key": {
                    "type": "string",
                    "description": "NVS key name to delete (required, max 15 chars, alphanumeric/_/-)"
                }
            },
            "required": ["key"]
        })
    }

    #[test]
    fn test_nvs_get_parameter_schema() {
        let schema = get_parameters();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["namespace"].is_object());
        assert!(schema["properties"]["key"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("key")));
        assert!(
            !required.contains(&json!("namespace")),
            "namespace is optional"
        );
    }

    #[test]
    fn test_nvs_set_parameter_schema() {
        let schema = set_parameters();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["namespace"].is_object());
        assert!(schema["properties"]["key"].is_object());
        assert!(schema["properties"]["value"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("key")));
        assert!(required.contains(&json!("value")));
        assert!(
            !required.contains(&json!("namespace")),
            "namespace is optional"
        );
    }

    #[test]
    fn test_nvs_delete_parameter_schema() {
        let schema = delete_parameters();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["namespace"].is_object());
        assert!(schema["properties"]["key"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("key")));
        assert!(
            !required.contains(&json!("namespace")),
            "namespace is optional"
        );
    }

    // -----------------------------------------------------------------------
    // Underscore and hyphen are valid
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_nvs_string_allows_underscore_and_hyphen() {
        assert!(validate_nvs_string("my_key", "key", MAX_KEY_LEN).is_ok());
        assert!(validate_nvs_string("my-key", "key", MAX_KEY_LEN).is_ok());
        assert!(validate_nvs_string("a_b-c", "key", MAX_KEY_LEN).is_ok());
    }

    // -----------------------------------------------------------------------
    // Error message contains field name
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_nvs_string_error_mentions_field() {
        let err = validate_nvs_string("", "namespace", MAX_NAMESPACE_LEN).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("namespace"),
            "error should mention field name, got: {msg}"
        );

        let err2 = validate_nvs_string("", "key", MAX_KEY_LEN).unwrap_err();
        let msg2 = err2.to_string();
        assert!(
            msg2.contains("key"),
            "error should mention field name, got: {msg2}"
        );
    }
}
