//! Hardware tool -- agent-facing tool for hardware discovery and peripheral interaction.
//!
//! The `HardwareTool` dispatches based on the `action` parameter:
//! - `list_devices` -- discover connected USB devices
//! - `device_info` -- get info about a specific device
//! - `connect` -- connect to a peripheral (placeholder)
//! - `send_command` -- send a command to a connected peripheral (placeholder)
//! - `read_data` -- read data from a connected peripheral (placeholder)
//! - `disconnect` -- disconnect a peripheral (placeholder)
//!
//! When compiled WITHOUT the `hardware` feature, the tool returns an informative
//! error directing the user to rebuild with the feature enabled.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{Result, ZeptoError};
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};

// ============================================================================
// Feature-gated implementation (with hardware feature)
// ============================================================================

#[cfg(feature = "hardware")]
use crate::hardware::HardwareManager;

/// Agent-facing hardware tool for USB discovery and peripheral interaction.
#[cfg(feature = "hardware")]
pub struct HardwareTool {
    manager: HardwareManager,
}

#[cfg(feature = "hardware")]
impl HardwareTool {
    /// Create a new HardwareTool.
    pub fn new() -> Self {
        Self {
            manager: HardwareManager::new(),
        }
    }
}

#[cfg(feature = "hardware")]
impl Default for HardwareTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "hardware")]
#[async_trait]
impl Tool for HardwareTool {
    fn name(&self) -> &str {
        "hardware"
    }

    fn description(&self) -> &str {
        "Discover and interact with connected hardware devices (USB, serial peripherals). \
         Actions: list_devices, device_info, connect, send_command, read_data, disconnect."
    }

    fn compact_description(&self) -> &str {
        "Hardware discovery and peripheral control"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_devices", "device_info", "connect", "send_command", "read_data", "disconnect"],
                    "description": "The hardware action to perform"
                },
                "device": {
                    "type": "string",
                    "description": "Device name or VID:PID (for device_info, connect, disconnect)"
                },
                "command": {
                    "type": "string",
                    "description": "Command to send (for send_command)"
                },
                "args": {
                    "type": "object",
                    "description": "Arguments for the command (for send_command)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".into()))?;

        match action {
            "list_devices" => {
                let devices = self.manager.discover_devices();
                if devices.is_empty() {
                    Ok(ToolOutput::llm_only("No hardware devices found. Connect a board (e.g., Nucleo, Arduino) via USB and try again.".to_string()))
                } else {
                    serde_json::to_string_pretty(&devices)
                        .map(ToolOutput::llm_only)
                        .map_err(|e| ZeptoError::Tool(format!("JSON serialize error: {e}")))
                }
            }
            "device_info" => {
                let device = args
                    .get("device")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Missing 'device' parameter for device_info action".into(),
                        )
                    })?;
                match self.manager.device_info(device) {
                    Some(info) => serde_json::to_string_pretty(&info)
                        .map(ToolOutput::llm_only)
                        .map_err(|e| ZeptoError::Tool(format!("JSON serialize error: {e}"))),
                    None => Ok(ToolOutput::llm_only(format!(
                        "Device '{}' not found in registry or connected devices.",
                        device
                    ))),
                }
            }
            "connect" => {
                let device = args
                    .get("device")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ZeptoError::Tool("Missing 'device' parameter for connect action".into())
                    })?;
                // TODO: Implement peripheral connection management
                Ok(ToolOutput::llm_only(format!(
                    "Connect to '{}' is not yet implemented. Use the peripheral-specific tools directly.",
                    device
                )))
            }
            "send_command" => {
                let _device = args.get("device").and_then(|v| v.as_str());
                let _command = args.get("command").and_then(|v| v.as_str());
                // TODO: Implement command dispatch to connected peripherals
                Ok(ToolOutput::llm_only("send_command is not yet implemented. Connect a peripheral first.".to_string()))
            }
            "read_data" => {
                let _device = args.get("device").and_then(|v| v.as_str());
                // TODO: Implement data reading from connected peripherals
                Ok(ToolOutput::llm_only("read_data is not yet implemented. Connect a peripheral first.".to_string()))
            }
            "disconnect" => {
                let device = args
                    .get("device")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ZeptoError::Tool(
                            "Missing 'device' parameter for disconnect action".into(),
                        )
                    })?;
                // TODO: Implement peripheral disconnection
                Ok(ToolOutput::llm_only(format!(
                    "Disconnect from '{}' is not yet implemented.",
                    device
                )))
            }
            other => Err(ZeptoError::Tool(format!(
                "Unknown hardware action: '{}'. Valid actions: list_devices, device_info, connect, send_command, read_data, disconnect",
                other
            ))),
        }
    }
}

// ============================================================================
// Stub implementation (without hardware feature)
// ============================================================================

/// Stub HardwareTool when the `hardware` feature is not enabled.
///
/// Returns an informative error directing the user to rebuild with features.
#[cfg(not(feature = "hardware"))]
pub struct HardwareTool;

#[cfg(not(feature = "hardware"))]
impl HardwareTool {
    /// Create a new stub HardwareTool.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(feature = "hardware"))]
impl Default for HardwareTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(feature = "hardware"))]
#[async_trait]
impl Tool for HardwareTool {
    fn name(&self) -> &str {
        "hardware"
    }

    fn description(&self) -> &str {
        "Hardware tool (requires 'hardware' build feature). \
         Rebuild with: cargo build --features hardware"
    }

    fn compact_description(&self) -> &str {
        "Hardware (feature not enabled)"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The hardware action to perform"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Err(ZeptoError::Tool(
            "Hardware tool requires 'hardware' build feature. \
             Rebuild with: cargo build --features hardware"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_tool_name() {
        let tool = HardwareTool::new();
        assert_eq!(tool.name(), "hardware");
    }

    #[test]
    fn test_hardware_tool_category() {
        let tool = HardwareTool::new();
        assert_eq!(tool.category(), ToolCategory::Hardware);
    }

    #[test]
    fn test_hardware_tool_description_not_empty() {
        let tool = HardwareTool::new();
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_hardware_tool_compact_description() {
        let tool = HardwareTool::new();
        assert!(!tool.compact_description().is_empty());
    }

    #[test]
    fn test_hardware_tool_parameters_schema() {
        let tool = HardwareTool::new();
        let params = tool.parameters();
        assert!(params.is_object());
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert_eq!(params["properties"]["action"]["type"], "string");
    }

    #[test]
    fn test_hardware_tool_default() {
        let tool = HardwareTool::default();
        assert_eq!(tool.name(), "hardware");
    }

    // In default build (no hardware feature), execute should return error
    #[cfg(not(feature = "hardware"))]
    #[tokio::test]
    async fn test_hardware_tool_stub_returns_error() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"action": "list_devices"}), &ctx)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("hardware"));
        assert!(err.contains("feature"));
    }

    #[cfg(not(feature = "hardware"))]
    #[tokio::test]
    async fn test_hardware_tool_stub_error_mentions_rebuild() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"action": "list_devices"}), &ctx)
            .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cargo build --features hardware"));
    }

    // Feature-gated tests (only run with --features hardware)
    #[cfg(feature = "hardware")]
    #[tokio::test]
    async fn test_hardware_tool_list_devices() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"action": "list_devices"}), &ctx)
            .await;
        // Should succeed (may return empty list or devices)
        assert!(result.is_ok());
    }

    #[cfg(feature = "hardware")]
    #[tokio::test]
    async fn test_hardware_tool_device_info_known() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool
            .execute(
                serde_json::json!({"action": "device_info", "device": "nucleo-f401re"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("nucleo-f401re"));
    }

    #[cfg(feature = "hardware")]
    #[tokio::test]
    async fn test_hardware_tool_device_info_unknown() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool
            .execute(
                serde_json::json!({"action": "device_info", "device": "nonexistent"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("not found"));
    }

    #[cfg(feature = "hardware")]
    #[tokio::test]
    async fn test_hardware_tool_unknown_action() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool
            .execute(serde_json::json!({"action": "invalid_action"}), &ctx)
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown hardware action"));
    }

    #[cfg(feature = "hardware")]
    #[tokio::test]
    async fn test_hardware_tool_missing_action() {
        let tool = HardwareTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(serde_json::json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("action"));
    }
}
