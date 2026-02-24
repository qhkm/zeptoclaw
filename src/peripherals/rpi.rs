//! Raspberry Pi GPIO peripheral -- native rppal access.
//!
//! Only compiled when `peripheral-rpi` feature is enabled and target is Linux.
//! Uses BCM pin numbering (e.g., GPIO 17, 27).

#![cfg(all(feature = "peripheral-rpi", target_os = "linux"))]

use super::traits::Peripheral;
use crate::error::{Result, ZeptoError};
use crate::peripherals::board_profile::RPI_PROFILE;
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

/// RPi GPIO peripheral -- direct access via rppal.
pub struct RpiGpioPeripheral {
    name: String,
}

impl RpiGpioPeripheral {
    /// Create a new RPi GPIO peripheral.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Peripheral for RpiGpioPeripheral {
    fn name(&self) -> &str {
        &self.name
    }

    fn board_type(&self) -> &str {
        "rpi-gpio"
    }

    async fn connect(&mut self) -> Result<()> {
        // Verify GPIO is accessible by doing a no-op init
        tokio::task::spawn_blocking(|| rppal::gpio::Gpio::new())
            .await
            .map_err(|e| ZeptoError::Tool(format!("GPIO init join error: {e}")))?
            .map_err(|e| ZeptoError::Tool(format!("GPIO init failed: {e}")))?;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        tokio::task::spawn_blocking(|| rppal::gpio::Gpio::new().is_ok())
            .await
            .unwrap_or(false)
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(RpiGpioReadTool),
            Box::new(RpiGpioWriteTool),
            Box::new(super::rpi_i2c::RpiI2cScanTool),
            Box::new(super::rpi_i2c::RpiI2cReadTool),
            Box::new(super::rpi_i2c::RpiI2cWriteTool),
        ]
    }
}

/// Tool: read GPIO pin value (BCM numbering).
struct RpiGpioReadTool;

#[async_trait]
impl Tool for RpiGpioReadTool {
    fn name(&self) -> &str {
        "rpi_gpio_read"
    }

    fn description(&self) -> &str {
        "Read the value (0 or 1) of a GPIO pin on Raspberry Pi. Uses BCM pin numbers (e.g. 17, 27)."
    }

    fn compact_description(&self) -> &str {
        "Read RPi GPIO pin"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "BCM GPIO pin number (e.g. 17, 27)"
                }
            },
            "required": ["pin"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing 'pin' parameter".into()))?;
        let pin_u8 = pin as u8;

        if !RPI_PROFILE.is_valid_gpio(pin_u8) {
            return Err(ZeptoError::Tool(format!(
                "GPIO pin {} is not a valid user-accessible BCM pin on Raspberry Pi (valid: 2-27)",
                pin
            )));
        }

        let value = tokio::task::spawn_blocking(move || {
            let gpio = rppal::gpio::Gpio::new()
                .map_err(|e| ZeptoError::Tool(format!("GPIO init: {e}")))?;
            let pin = gpio
                .get(pin_u8)
                .map_err(|e| ZeptoError::Tool(format!("GPIO get pin {}: {}", pin_u8, e)))?
                .into_input();
            Ok::<_, ZeptoError>(match pin.read() {
                rppal::gpio::Level::Low => 0,
                rppal::gpio::Level::High => 1,
            })
        })
        .await
        .map_err(|e| ZeptoError::Tool(format!("GPIO read join error: {e}")))??;

        Ok(ToolOutput::llm_only(format!("pin {} = {}", pin, value)))
    }
}

/// Tool: write GPIO pin value (BCM numbering).
struct RpiGpioWriteTool;

#[async_trait]
impl Tool for RpiGpioWriteTool {
    fn name(&self) -> &str {
        "rpi_gpio_write"
    }

    fn description(&self) -> &str {
        "Set a GPIO pin high (1) or low (0) on Raspberry Pi. Uses BCM pin numbers."
    }

    fn compact_description(&self) -> &str {
        "Write RPi GPIO pin"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "BCM GPIO pin number"
                },
                "value": {
                    "type": "integer",
                    "description": "0 for low, 1 for high"
                }
            },
            "required": ["pin", "value"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing 'pin' parameter".into()))?;
        let value = args
            .get("value")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing 'value' parameter".into()))?;
        let pin_u8 = pin as u8;

        if !RPI_PROFILE.is_valid_gpio(pin_u8) {
            return Err(ZeptoError::Tool(format!(
                "GPIO pin {} is not a valid user-accessible BCM pin on Raspberry Pi (valid: 2-27)",
                pin
            )));
        }

        let level = match value {
            0 => rppal::gpio::Level::Low,
            _ => rppal::gpio::Level::High,
        };

        tokio::task::spawn_blocking(move || {
            let gpio = rppal::gpio::Gpio::new()
                .map_err(|e| ZeptoError::Tool(format!("GPIO init: {e}")))?;
            let mut pin = gpio
                .get(pin_u8)
                .map_err(|e| ZeptoError::Tool(format!("GPIO get pin {}: {}", pin_u8, e)))?
                .into_output();
            pin.write(level);
            Ok::<_, ZeptoError>(())
        })
        .await
        .map_err(|e| ZeptoError::Tool(format!("GPIO write join error: {e}")))??;

        Ok(ToolOutput::llm_only(format!("pin {} = {}", pin, value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::board_profile::RPI_PROFILE;

    #[test]
    fn test_validate_gpio_pin_valid() {
        for pin in 2..=27u8 {
            assert!(RPI_PROFILE.is_valid_gpio(pin));
        }
    }

    #[test]
    fn test_validate_gpio_pin_invalid() {
        assert!(!RPI_PROFILE.is_valid_gpio(0));
        assert!(!RPI_PROFILE.is_valid_gpio(1));
        assert!(!RPI_PROFILE.is_valid_gpio(28));
    }

    #[test]
    fn test_gpio_read_tool_metadata() {
        let tool = RpiGpioReadTool;
        assert_eq!(tool.name(), "rpi_gpio_read");
        assert_eq!(tool.category(), ToolCategory::Hardware);
        let params = tool.parameters();
        assert!(params["properties"]["pin"].is_object());
    }

    #[test]
    fn test_gpio_write_tool_metadata() {
        let tool = RpiGpioWriteTool;
        assert_eq!(tool.name(), "rpi_gpio_write");
        assert_eq!(tool.category(), ToolCategory::Hardware);
        let params = tool.parameters();
        assert!(params["properties"]["pin"].is_object());
        assert!(params["properties"]["value"].is_object());
    }
}
