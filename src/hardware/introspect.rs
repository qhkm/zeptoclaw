//! Device introspection -- correlate serial paths with USB device info.
//!
//! Provides the ability to introspect a device by its serial port path,
//! correlating it with USB enumeration data and the board registry to
//! produce enriched device metadata including architecture and memory map info.

#![cfg(all(
    feature = "hardware",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]

use super::discover;
use super::registry;
use crate::error::Result;
use serde::Serialize;

/// Result of introspecting a device by its serial path.
#[derive(Debug, Clone, Serialize)]
pub struct IntrospectResult {
    /// The serial port path queried
    pub path: String,
    /// USB Vendor ID (if correlated)
    pub vid: Option<u16>,
    /// USB Product ID (if correlated)
    pub pid: Option<u16>,
    /// Board name from registry (if recognized)
    pub board_name: Option<String>,
    /// Architecture from registry (if recognized)
    pub architecture: Option<String>,
    /// Memory map note (static or from probe)
    pub memory_map_note: String,
}

/// Introspect a device by its serial path (e.g., /dev/ttyACM0, /dev/tty.usbmodem*).
///
/// Attempts to correlate the serial path with USB devices from discovery.
/// Best-effort: if exactly one USB device is found, it is assumed to match.
/// With multiple devices, the first recognized board is preferred.
///
/// # Errors
///
/// Returns an error if USB enumeration fails.
pub fn introspect_device(path: &str) -> Result<IntrospectResult> {
    let devices = discover::list_usb_devices()?;

    // Try to correlate path with a discovered device.
    // Best-effort: if we have exactly one device, use it.
    // With multiple: prefer the first recognized board.
    let matched = if devices.len() == 1 {
        devices.first().cloned()
    } else if devices.is_empty() {
        None
    } else {
        devices
            .iter()
            .find(|d| d.board_name.is_some())
            .cloned()
            .or_else(|| devices.first().cloned())
    };

    let (vid, pid, board_name, architecture) = match matched {
        Some(d) => (Some(d.vid), Some(d.pid), d.board_name, d.architecture),
        None => (None, None, None, None),
    };

    // Enrich with registry if we have VID/PID
    let board_info = vid.and_then(|v| pid.and_then(|p| registry::lookup_board(v, p)));
    let architecture =
        architecture.or_else(|| board_info.and_then(|b| b.architecture.map(String::from)));
    let board_name = board_name.or_else(|| board_info.map(|b| b.name.to_string()));

    let memory_map_note = memory_map_for_board(board_name.as_deref());

    Ok(IntrospectResult {
        path: path.to_string(),
        vid,
        pid,
        board_name,
        architecture,
        memory_map_note,
    })
}

/// Get a memory map note for a board. Static data unless probe feature is enabled.
fn memory_map_for_board(board_name: Option<&str>) -> String {
    match board_name {
        Some("nucleo-f401re") => "Flash: 512 KB (0x08000000), RAM: 96 KB (0x20000000). \
             Build with --features probe for live memory map via USB."
            .to_string(),
        Some("nucleo-f411re") => "Flash: 512 KB (0x08000000), RAM: 128 KB (0x20000000). \
             Build with --features probe for live memory map via USB."
            .to_string(),
        Some("arduino-uno") => "Flash: 32 KB, SRAM: 2 KB, EEPROM: 1 KB".to_string(),
        Some("arduino-mega") => "Flash: 256 KB, SRAM: 8 KB, EEPROM: 4 KB".to_string(),
        Some(_) => "Build with --features probe for live memory map via USB".to_string(),
        None => "Unknown device. Connect a supported board and try again.".to_string(),
    }
}

/// Validate that a serial path matches allowed patterns.
///
/// Security: only allow known serial device path prefixes to prevent
/// arbitrary file access through the serial peripheral system.
pub fn validate_serial_path(path: &str) -> std::result::Result<(), String> {
    const ALLOWED_PATH_PREFIXES: &[&str] = &[
        "/dev/ttyACM",
        "/dev/ttyUSB",
        "/dev/tty.usbmodem",
        "/dev/cu.usbmodem",
        "/dev/tty.usbserial",
        "/dev/cu.usbserial",
        "COM",
    ];

    if ALLOWED_PATH_PREFIXES.iter().any(|p| path.starts_with(p)) {
        Ok(())
    } else {
        Err(format!(
            "Serial path not allowed: {}. Allowed prefixes: {}",
            path,
            ALLOWED_PATH_PREFIXES.join(", ")
        ))
    }
}
