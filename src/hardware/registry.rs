//! Board registry -- maps USB VID/PID to known board names and architectures.
//!
//! This module provides a static lookup table of known development boards,
//! mapping their USB Vendor ID (VID) and Product ID (PID) to human-readable
//! board names and architecture descriptions. The registry is always compiled
//! (no feature gate) so that board lookups work in all builds.

use serde::Serialize;

/// Information about a known development board.
#[derive(Debug, Clone, Serialize)]
pub struct BoardInfo {
    /// USB Vendor ID
    pub vid: u16,
    /// USB Product ID
    pub pid: u16,
    /// Human-readable board name (e.g., "nucleo-f401re")
    pub name: &'static str,
    /// Architecture description (e.g., "ARM Cortex-M4")
    pub architecture: Option<&'static str>,
}

/// Known USB VID/PID to board mappings.
/// VID 0x0483 = STMicroelectronics, 0x2341 = Arduino, 0x10c4 = Silicon Labs.
const KNOWN_BOARDS: &[BoardInfo] = &[
    // STMicroelectronics Nucleo boards
    BoardInfo {
        vid: 0x0483,
        pid: 0x374b,
        name: "nucleo-f401re",
        architecture: Some("ARM Cortex-M4"),
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x3748,
        name: "nucleo-f411re",
        architecture: Some("ARM Cortex-M4"),
    },
    // Arduino boards
    BoardInfo {
        vid: 0x2341,
        pid: 0x0043,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P"),
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0078,
        name: "arduino-uno",
        architecture: Some("Arduino Uno Q / ATmega328P"),
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0042,
        name: "arduino-mega",
        architecture: Some("AVR ATmega2560"),
    },
    // Silicon Labs USB-UART bridges
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "cp2102",
        architecture: Some("USB-UART bridge"),
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea70,
        name: "cp2102n",
        architecture: Some("USB-UART bridge"),
    },
    // ESP32 dev boards (CH340 USB-UART)
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "esp32",
        architecture: Some("ESP32 (CH340)"),
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d4,
        name: "esp32",
        architecture: Some("ESP32 (CH340)"),
    },
];

/// Look up a board by USB Vendor ID and Product ID.
///
/// Returns `Some(&BoardInfo)` if the VID/PID pair matches a known board,
/// or `None` if the device is not recognized.
pub fn lookup_board(vid: u16, pid: u16) -> Option<&'static BoardInfo> {
    KNOWN_BOARDS.iter().find(|b| b.vid == vid && b.pid == pid)
}

/// Return all known board entries in the registry.
pub fn known_boards() -> &'static [BoardInfo] {
    KNOWN_BOARDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_nucleo_f401re() {
        let b = lookup_board(0x0483, 0x374b).unwrap();
        assert_eq!(b.name, "nucleo-f401re");
        assert_eq!(b.architecture, Some("ARM Cortex-M4"));
    }

    #[test]
    fn test_lookup_nucleo_f411re() {
        let b = lookup_board(0x0483, 0x3748).unwrap();
        assert_eq!(b.name, "nucleo-f411re");
        assert_eq!(b.architecture, Some("ARM Cortex-M4"));
    }

    #[test]
    fn test_lookup_arduino_uno() {
        let b = lookup_board(0x2341, 0x0043).unwrap();
        assert_eq!(b.name, "arduino-uno");
        assert!(b.architecture.unwrap().contains("ATmega328P"));
    }

    #[test]
    fn test_lookup_arduino_mega() {
        let b = lookup_board(0x2341, 0x0042).unwrap();
        assert_eq!(b.name, "arduino-mega");
        assert!(b.architecture.unwrap().contains("ATmega2560"));
    }

    #[test]
    fn test_lookup_esp32() {
        let b = lookup_board(0x1a86, 0x7523).unwrap();
        assert_eq!(b.name, "esp32");
        assert!(b.architecture.unwrap().contains("CH340"));
    }

    #[test]
    fn test_lookup_cp2102() {
        let b = lookup_board(0x10c4, 0xea60).unwrap();
        assert_eq!(b.name, "cp2102");
        assert_eq!(b.architecture, Some("USB-UART bridge"));
    }

    #[test]
    fn test_lookup_unknown_returns_none() {
        assert!(lookup_board(0x0000, 0x0000).is_none());
    }

    #[test]
    fn test_lookup_known_vid_unknown_pid() {
        // Known Arduino VID but unknown PID
        assert!(lookup_board(0x2341, 0xFFFF).is_none());
    }

    #[test]
    fn test_known_boards_not_empty() {
        assert!(!known_boards().is_empty());
    }

    #[test]
    fn test_known_boards_count() {
        assert_eq!(known_boards().len(), 9);
    }

    #[test]
    fn test_all_boards_have_names() {
        for board in known_boards() {
            assert!(
                !board.name.is_empty(),
                "Board with VID {:04x} has empty name",
                board.vid
            );
        }
    }

    #[test]
    fn test_board_info_serialize() {
        let board = lookup_board(0x0483, 0x374b).unwrap();
        let json = serde_json::to_value(board).unwrap();
        assert_eq!(json["name"], "nucleo-f401re");
        assert_eq!(json["vid"], 0x0483);
        assert_eq!(json["pid"], 0x374b);
    }
}
