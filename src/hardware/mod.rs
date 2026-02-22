//! Hardware discovery -- USB device enumeration, board registry, and introspection.
//!
//! This module provides hardware discovery capabilities for ZeptoClaw:
//!
//! - **Board registry** (`registry`): Static VID/PID to board name mapping (always compiled)
//! - **USB discovery** (`discover`): Enumerate connected USB devices (feature-gated: `hardware`)
//! - **Introspection** (`introspect`): Correlate serial paths with USB devices (feature-gated: `hardware`)
//!
//! The `HardwareManager` orchestrator ties these together for the agent tool and CLI.

pub mod registry;

#[cfg(all(
    feature = "hardware",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub mod discover;

#[cfg(all(
    feature = "hardware",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub mod introspect;

use serde::Serialize;

/// A hardware device discovered during auto-scan.
///
/// This is the unified device representation used by the CLI and agent tool.
/// It is always available (no feature gate) so that stub code can reference it.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredDevice {
    /// Human-readable device name (board name or "VID:PID" fallback)
    pub name: String,
    /// Product description from USB descriptor
    pub detail: Option<String>,
    /// Serial port path (if correlated, e.g., "/dev/ttyACM0")
    pub device_path: Option<String>,
    /// USB Vendor ID
    pub vid: u16,
    /// USB Product ID
    pub pid: u16,
    /// Architecture description (e.g., "ARM Cortex-M4")
    pub architecture: Option<String>,
}

/// Orchestrator for hardware discovery operations.
///
/// Provides a high-level API for the agent tool and CLI commands.
/// When the `hardware` feature is disabled, all methods return empty results
/// or informative error messages.
pub struct HardwareManager;

impl HardwareManager {
    /// Create a new HardwareManager.
    pub fn new() -> Self {
        Self
    }

    /// Discover all connected hardware devices.
    ///
    /// Returns a list of discovered devices enriched with board registry data.
    /// Returns an empty list when the `hardware` feature is not enabled or
    /// on unsupported platforms.
    pub fn discover_devices(&self) -> Vec<DiscoveredDevice> {
        #[cfg(all(
            feature = "hardware",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        {
            match discover::list_usb_devices() {
                Ok(devices) => devices
                    .into_iter()
                    .map(|d| DiscoveredDevice {
                        name: d
                            .board_name
                            .unwrap_or_else(|| format!("{:04x}:{:04x}", d.vid, d.pid)),
                        detail: d.product_string,
                        device_path: None,
                        vid: d.vid,
                        pid: d.pid,
                        architecture: d.architecture,
                    })
                    .collect(),
                Err(_) => Vec::new(),
            }
        }

        #[cfg(not(all(
            feature = "hardware",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        )))]
        {
            Vec::new()
        }
    }

    /// Get info about a specific device by name or "VID:PID" string.
    ///
    /// Searches the board registry first, then discovered devices.
    pub fn device_info(&self, query: &str) -> Option<DiscoveredDevice> {
        // Try board registry by name
        for board in registry::known_boards() {
            if board.name == query {
                return Some(DiscoveredDevice {
                    name: board.name.to_string(),
                    detail: None,
                    device_path: None,
                    vid: board.vid,
                    pid: board.pid,
                    architecture: board.architecture.map(String::from),
                });
            }
        }

        // Try VID:PID format (e.g., "0483:374b")
        if let Some((vid_str, pid_str)) = query.split_once(':') {
            if let (Ok(vid), Ok(pid)) = (
                u16::from_str_radix(vid_str, 16),
                u16::from_str_radix(pid_str, 16),
            ) {
                if let Some(board) = registry::lookup_board(vid, pid) {
                    return Some(DiscoveredDevice {
                        name: board.name.to_string(),
                        detail: None,
                        device_path: None,
                        vid: board.vid,
                        pid: board.pid,
                        architecture: board.architecture.map(String::from),
                    });
                }
            }
        }

        // Try discovered devices
        let devices = self.discover_devices();
        devices.into_iter().find(|d| d.name == query)
    }
}

impl Default for HardwareManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_device_construction() {
        let dev = DiscoveredDevice {
            name: "nucleo-f401re".to_string(),
            detail: Some("STM32 Nucleo".to_string()),
            device_path: Some("/dev/ttyACM0".to_string()),
            vid: 0x0483,
            pid: 0x374b,
            architecture: Some("ARM Cortex-M4".to_string()),
        };
        assert_eq!(dev.name, "nucleo-f401re");
        assert_eq!(dev.vid, 0x0483);
        assert_eq!(dev.pid, 0x374b);
        assert!(dev.architecture.is_some());
    }

    #[test]
    fn test_discovered_device_serialize() {
        let dev = DiscoveredDevice {
            name: "arduino-uno".to_string(),
            detail: None,
            device_path: None,
            vid: 0x2341,
            pid: 0x0043,
            architecture: Some("AVR ATmega328P".to_string()),
        };
        let json = serde_json::to_value(&dev).unwrap();
        assert_eq!(json["name"], "arduino-uno");
        assert_eq!(json["vid"], 0x2341);
    }

    #[test]
    fn test_hardware_manager_default() {
        let mgr = HardwareManager::default();
        // In default build (no hardware feature), discover returns empty
        let devices = mgr.discover_devices();
        // We cannot assert non-empty without real hardware; just check it does not panic
        let _ = devices;
    }

    #[test]
    fn test_hardware_manager_device_info_by_name() {
        let mgr = HardwareManager::new();
        let info = mgr.device_info("nucleo-f401re");
        assert!(info.is_some());
        let dev = info.unwrap();
        assert_eq!(dev.name, "nucleo-f401re");
        assert_eq!(dev.vid, 0x0483);
        assert_eq!(dev.pid, 0x374b);
    }

    #[test]
    fn test_hardware_manager_device_info_by_vid_pid() {
        let mgr = HardwareManager::new();
        let info = mgr.device_info("0483:374b");
        assert!(info.is_some());
        assert_eq!(info.unwrap().name, "nucleo-f401re");
    }

    #[test]
    fn test_hardware_manager_device_info_unknown() {
        let mgr = HardwareManager::new();
        let info = mgr.device_info("nonexistent-board");
        assert!(info.is_none());
    }

    #[test]
    fn test_hardware_manager_device_info_invalid_vid_pid() {
        let mgr = HardwareManager::new();
        let info = mgr.device_info("ZZZZ:YYYY");
        assert!(info.is_none());
    }
}
