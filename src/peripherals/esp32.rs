//! ESP32-specific serial peripheral.
//!
//! Wraps the generic `SerialPeripheral` with ESP32-specific defaults
//! (115200 baud, board type "esp32") and extends it with I2C and NVS tools
//! based on the ESP32 [`BoardProfile`] capabilities.
//!
//! Only compiled when the `peripheral-esp32` feature is enabled.

#[cfg(feature = "peripheral-esp32")]
use super::board_profile::ESP32_PROFILE;
#[cfg(feature = "peripheral-esp32")]
use super::i2c::{I2cReadTool, I2cScanTool, I2cWriteTool};
#[cfg(feature = "peripheral-esp32")]
use super::nvs::{NvsDeleteTool, NvsGetTool, NvsSetTool};
#[cfg(feature = "peripheral-esp32")]
use super::serial::{SerialPeripheral, SerialTransport};
#[cfg(feature = "peripheral-esp32")]
use super::traits::Peripheral;
#[cfg(feature = "peripheral-esp32")]
use crate::error::Result;
#[cfg(feature = "peripheral-esp32")]
use crate::tools::Tool;
#[cfg(feature = "peripheral-esp32")]
use async_trait::async_trait;
#[cfg(feature = "peripheral-esp32")]
use std::sync::Arc;

/// Default baud rate for ESP32 boards.
#[cfg(feature = "peripheral-esp32")]
const ESP32_DEFAULT_BAUD: u32 = 115_200;

/// ESP32 peripheral -- wraps SerialPeripheral with ESP32 defaults and
/// adds I2C and NVS tools based on the ESP32 board profile.
#[cfg(feature = "peripheral-esp32")]
pub struct Esp32Peripheral {
    inner: SerialPeripheral,
    transport: Arc<SerialTransport>,
}

#[cfg(feature = "peripheral-esp32")]
impl Esp32Peripheral {
    /// Create a new ESP32 peripheral connected to the given serial path.
    ///
    /// Uses 115200 baud (standard for ESP32 USB-CDC/UART).
    ///
    /// # Arguments
    /// * `path` - Serial port path (e.g., "/dev/ttyUSB0", "/dev/cu.usbserial-*")
    ///
    /// # Errors
    ///
    /// Returns an error if `path` is not an allowed serial path or if the
    /// serial port cannot be opened.
    pub fn new(path: &str) -> Result<Self> {
        let inner = SerialPeripheral::connect_to(path, "esp32", ESP32_DEFAULT_BAUD)?;
        let transport = inner.transport();
        Ok(Self { inner, transport })
    }
}

#[cfg(feature = "peripheral-esp32")]
#[async_trait]
impl Peripheral for Esp32Peripheral {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn board_type(&self) -> &str {
        "esp32"
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
        let mut tools: Vec<Box<dyn Tool>> = self.inner.tools();

        // Add I2C tools if the profile has I2C buses.
        if !ESP32_PROFILE.i2c_buses.is_empty() {
            tools.push(Box::new(I2cScanTool {
                transport: self.transport.clone(),
                profile: &ESP32_PROFILE,
            }));
            tools.push(Box::new(I2cReadTool {
                transport: self.transport.clone(),
                profile: &ESP32_PROFILE,
            }));
            tools.push(Box::new(I2cWriteTool {
                transport: self.transport.clone(),
                profile: &ESP32_PROFILE,
            }));
        }

        // Add NVS tools if the profile has NVS support.
        if ESP32_PROFILE.has_nvs {
            tools.push(Box::new(NvsGetTool {
                transport: self.transport.clone(),
            }));
            tools.push(Box::new(NvsSetTool {
                transport: self.transport.clone(),
            }));
            tools.push(Box::new(NvsDeleteTool {
                transport: self.transport.clone(),
            }));
        }

        tools
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "peripheral-esp32"))]
mod tests {
    use super::*;
    use crate::peripherals::board_profile::ESP32_PROFILE;

    #[test]
    fn test_esp32_default_baud() {
        assert_eq!(ESP32_DEFAULT_BAUD, 115_200);
    }

    #[test]
    fn test_esp32_profile_has_i2c() {
        // ESP32 profile must expose at least one I2C bus.
        assert!(
            !ESP32_PROFILE.i2c_buses.is_empty(),
            "ESP32 profile should have I2C buses"
        );
        // The standard DevKit exposes bus 0 on SDA=21, SCL=22.
        let bus = ESP32_PROFILE.i2c_bus(0).expect("bus 0 should exist");
        assert_eq!(bus.sda_pin, 21);
        assert_eq!(bus.scl_pin, 22);
    }

    #[test]
    fn test_esp32_profile_has_nvs() {
        assert!(
            ESP32_PROFILE.has_nvs,
            "ESP32 profile should have NVS support"
        );
    }

    #[test]
    fn test_esp32_new_rejects_invalid_path() {
        match Esp32Peripheral::new("/etc/passwd") {
            Ok(_) => panic!("should reject disallowed path"),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("not allowed"),
                    "error should mention 'not allowed', got: {msg}"
                );
            }
        }
    }
}
