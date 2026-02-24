//! Hardware peripherals -- STM32, Arduino, RPi GPIO, etc.
//!
//! Peripherals extend the agent with physical capabilities. Each peripheral
//! implements the [`Peripheral`] trait and exposes its capabilities as
//! agent tools (e.g., `gpio_read`, `gpio_write`).
//!
//! # Feature Gates
//!
//! - `hardware`: Enables serial peripherals (STM32, Arduino, ESP32)
//! - `peripheral-rpi`: Enables Raspberry Pi GPIO (Linux only, via rppal)
//!
//! Without feature flags, only the `Peripheral` trait and stub factory are compiled.

pub mod board_profile;
pub mod traits;

#[cfg(feature = "hardware")]
pub mod serial;

#[cfg(feature = "hardware")]
pub mod arduino;

#[cfg(feature = "hardware")]
pub mod nucleo;

#[cfg(feature = "hardware")]
pub mod i2c;

#[cfg(feature = "hardware")]
pub mod nvs;

#[cfg(all(feature = "peripheral-rpi", target_os = "linux"))]
pub mod rpi;

#[cfg(all(feature = "peripheral-rpi", target_os = "linux"))]
pub mod rpi_i2c;

#[cfg(feature = "peripheral-esp32")]
pub mod esp32;

pub use traits::Peripheral;

use crate::tools::Tool;

/// Create peripheral tools based on enabled features.
///
/// When the `hardware` feature is disabled, returns an empty vec.
/// When enabled, this serves as the factory entry point for creating
/// peripheral tools that get merged into the agent's tool registry.
pub fn create_peripheral_tools() -> Vec<Box<dyn Tool>> {
    // Peripheral tools are created on-demand when a connection is established
    // via the HardwareTool's "connect" action. This factory is a placeholder
    // for static/always-available peripheral tools.
    Vec::new()
}

/// Validate a serial port path for security.
///
/// Only allows known serial device path prefixes to prevent arbitrary
/// file access through the serial peripheral system.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_peripheral_tools_returns_empty() {
        let tools = create_peripheral_tools();
        assert!(tools.is_empty());
    }

    #[test]
    fn test_validate_serial_path_linux_acm() {
        assert!(validate_serial_path("/dev/ttyACM0").is_ok());
        assert!(validate_serial_path("/dev/ttyACM1").is_ok());
    }

    #[test]
    fn test_validate_serial_path_linux_usb() {
        assert!(validate_serial_path("/dev/ttyUSB0").is_ok());
    }

    #[test]
    fn test_validate_serial_path_macos_modem() {
        assert!(validate_serial_path("/dev/tty.usbmodem14201").is_ok());
        assert!(validate_serial_path("/dev/cu.usbmodem14201").is_ok());
    }

    #[test]
    fn test_validate_serial_path_macos_serial() {
        assert!(validate_serial_path("/dev/tty.usbserial-1420").is_ok());
        assert!(validate_serial_path("/dev/cu.usbserial-1420").is_ok());
    }

    #[test]
    fn test_validate_serial_path_windows() {
        assert!(validate_serial_path("COM3").is_ok());
        assert!(validate_serial_path("COM10").is_ok());
    }

    #[test]
    fn test_validate_serial_path_rejects_arbitrary() {
        assert!(validate_serial_path("/dev/sda1").is_err());
        assert!(validate_serial_path("/etc/passwd").is_err());
        assert!(validate_serial_path("/tmp/fake_serial").is_err());
        assert!(validate_serial_path("").is_err());
    }

    #[test]
    fn test_validate_serial_path_error_message() {
        let err = validate_serial_path("/etc/passwd").unwrap_err();
        assert!(err.contains("not allowed"));
        assert!(err.contains("/etc/passwd"));
    }
}
