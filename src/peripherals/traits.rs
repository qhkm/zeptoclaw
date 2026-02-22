//! Peripheral trait -- hardware boards (STM32, RPi GPIO) that expose tools.
//!
//! Peripherals are the agent's "arms and legs": remote devices that run minimal
//! firmware and expose capabilities (GPIO, sensors, actuators) as tools.
//! The trait is always compiled so it can be referenced in non-feature-gated code.

use async_trait::async_trait;

use crate::error::Result;
use crate::tools::Tool;

/// A hardware peripheral that exposes capabilities as agent tools.
///
/// Implement this trait for each supported board type (e.g., Nucleo-F401RE
/// over serial, Raspberry Pi GPIO via rppal). When the agent connects
/// to a peripheral, the tools returned by [`tools`](Peripheral::tools) are
/// merged into the agent's tool registry, making hardware capabilities
/// available to the LLM as callable functions.
///
/// The lifecycle follows a connect -> use -> disconnect pattern. Implementations
/// must be `Send + Sync` because the peripheral may be accessed from multiple
/// async tasks after connection.
#[async_trait]
pub trait Peripheral: Send + Sync {
    /// Return the human-readable instance name of this peripheral.
    ///
    /// Should uniquely identify a specific device instance, including an index
    /// or serial number when multiple boards of the same type are connected
    /// (e.g., `"nucleo-f401re-0"`, `"rpi-gpio-hat-1"`).
    fn name(&self) -> &str;

    /// Return the board type identifier for this peripheral.
    ///
    /// A stable, lowercase string used in configuration and factory registration
    /// (e.g., `"nucleo-f401re"`, `"rpi-gpio"`).
    fn board_type(&self) -> &str;

    /// Establish a connection to the peripheral hardware.
    ///
    /// Opens the underlying transport (serial port, GPIO bus, etc.) and
    /// performs any initialization handshake required by the firmware.
    async fn connect(&mut self) -> Result<()>;

    /// Disconnect from the peripheral and release all held resources.
    ///
    /// Closes serial ports, unexports GPIO pins, and performs cleanup.
    async fn disconnect(&mut self) -> Result<()>;

    /// Check whether the peripheral is reachable and responsive.
    ///
    /// Performs a lightweight probe without altering device state.
    async fn health_check(&self) -> bool;

    /// Return the tools this peripheral exposes to the agent.
    ///
    /// Each returned [`Tool`] delegates execution to the underlying hardware
    /// (e.g., `gpio_read`, `gpio_write`, `sensor_read`).
    fn tools(&self) -> Vec<Box<dyn Tool>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the Peripheral trait is object-safe (can be used as `dyn Peripheral`).
    #[test]
    fn test_peripheral_trait_object_safety() {
        fn _assert_object_safe(_p: &dyn Peripheral) {}
        // If this compiles, the trait is object-safe
    }
}
