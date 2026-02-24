//! Arduino-specific serial peripheral.
//!
//! Wraps the generic `SerialPeripheral` with Arduino-specific defaults
//! (115200 baud, board type "arduino-uno" or "arduino-mega").
//! Only compiled when the `hardware` feature is enabled.

use super::serial::SerialPeripheral;
use super::traits::Peripheral;
use crate::error::Result;
use crate::tools::Tool;
use async_trait::async_trait;

/// Default baud rate for Arduino boards.
const ARDUINO_DEFAULT_BAUD: u32 = 115_200;

/// Arduino peripheral -- wraps SerialPeripheral with Arduino defaults.
pub struct ArduinoPeripheral {
    inner: SerialPeripheral,
}

impl ArduinoPeripheral {
    /// Create a new Arduino peripheral connected to the given serial path.
    ///
    /// Uses 115200 baud by default (standard for Arduino serial monitor).
    ///
    /// # Arguments
    /// * `path` - Serial port path (e.g., "/dev/ttyACM0", "/dev/cu.usbmodem*")
    /// * `board` - Board type (e.g., "arduino-uno", "arduino-mega")
    pub fn new(path: &str, board: &str) -> Result<Self> {
        let inner = SerialPeripheral::connect_to(path, board, ARDUINO_DEFAULT_BAUD)?;
        Ok(Self { inner })
    }

    /// Create an Arduino Uno peripheral.
    pub fn uno(path: &str) -> Result<Self> {
        Self::new(path, "arduino-uno")
    }

    /// Create an Arduino Mega peripheral.
    pub fn mega(path: &str) -> Result<Self> {
        Self::new(path, "arduino-mega")
    }
}

#[async_trait]
impl Peripheral for ArduinoPeripheral {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn board_type(&self) -> &str {
        self.inner.board_type()
    }

    async fn connect(&mut self) -> Result<()> {
        self.inner.connect().await
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.inner.disconnect().await
    }

    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.inner.tools()
    }
}
