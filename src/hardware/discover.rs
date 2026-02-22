//! USB device discovery -- enumerate devices and enrich with board registry.
//!
//! USB enumeration via `nusb` is only supported on Linux, macOS, and Windows.
//! This module is conditionally compiled only on those platforms when the
//! `hardware` feature is enabled. On unsupported platforms, callers in
//! `hardware/mod.rs` fall back to an empty result.

#![cfg(all(
    feature = "hardware",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]

use super::registry;
use crate::error::{Result, ZeptoError};
use nusb::MaybeFuture;
use serde::Serialize;

/// Information about a discovered USB device.
#[derive(Debug, Clone, Serialize)]
pub struct UsbDeviceInfo {
    /// Bus identifier (platform-specific)
    pub bus_id: String,
    /// Device address on the bus
    pub device_address: u8,
    /// USB Vendor ID
    pub vid: u16,
    /// USB Product ID
    pub pid: u16,
    /// Product string from USB descriptor
    pub product_string: Option<String>,
    /// Matched board name from registry (if recognized)
    pub board_name: Option<String>,
    /// Architecture from registry (if recognized)
    pub architecture: Option<String>,
}

/// Enumerate all connected USB devices and enrich with board registry lookup.
///
/// Iterates over all USB devices visible to the system, performs a VID/PID
/// lookup in the board registry, and returns enriched device information.
///
/// # Errors
///
/// Returns an error if USB enumeration fails (e.g., insufficient permissions).
pub fn list_usb_devices() -> Result<Vec<UsbDeviceInfo>> {
    let mut devices = Vec::new();

    let iter = nusb::list_devices()
        .wait()
        .map_err(|e| ZeptoError::Tool(format!("USB enumeration failed: {e}")))?;

    for dev in iter {
        let vid = dev.vendor_id();
        let pid = dev.product_id();
        let board = registry::lookup_board(vid, pid);

        devices.push(UsbDeviceInfo {
            bus_id: dev.bus_id().to_string(),
            device_address: dev.device_address(),
            vid,
            pid,
            product_string: dev.product_string().map(String::from),
            board_name: board.map(|b| b.name.to_string()),
            architecture: board.and_then(|b| b.architecture.map(String::from)),
        });
    }

    Ok(devices)
}
