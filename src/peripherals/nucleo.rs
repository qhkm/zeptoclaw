//! STM32 Nucleo-specific serial peripheral.
//!
//! Wraps the generic `SerialPeripheral` with Nucleo-specific defaults
//! (115200 baud, board type "nucleo-f401re" or "nucleo-f411re").
//! Only compiled when the `hardware` feature is enabled.

#![cfg(feature = "hardware")]

use super::serial::SerialPeripheral;
use super::traits::Peripheral;
use crate::error::Result;
use crate::tools::Tool;
use async_trait::async_trait;

/// Default baud rate for Nucleo boards.
const NUCLEO_DEFAULT_BAUD: u32 = 115_200;

/// STM32 Nucleo peripheral -- wraps SerialPeripheral with Nucleo defaults.
pub struct NucleoPeripheral {
    inner: SerialPeripheral,
}

impl NucleoPeripheral {
    /// Create a new Nucleo peripheral connected to the given serial path.
    ///
    /// Uses 115200 baud by default (standard for STM32 Nucleo UART).
    ///
    /// # Arguments
    /// * `path` - Serial port path (e.g., "/dev/ttyACM0")
    /// * `board` - Board type (e.g., "nucleo-f401re", "nucleo-f411re")
    pub fn new(path: &str, board: &str) -> Result<Self> {
        let inner = SerialPeripheral::connect_to(path, board, NUCLEO_DEFAULT_BAUD)?;
        Ok(Self { inner })
    }

    /// Create a Nucleo-F401RE peripheral.
    pub fn f401re(path: &str) -> Result<Self> {
        Self::new(path, "nucleo-f401re")
    }

    /// Create a Nucleo-F411RE peripheral.
    pub fn f411re(path: &str) -> Result<Self> {
        Self::new(path, "nucleo-f411re")
    }
}

#[async_trait]
impl Peripheral for NucleoPeripheral {
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
