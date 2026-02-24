//! Native Raspberry Pi I2C tools — scan, read, write via rppal.
//!
//! Only compiled when `peripheral-rpi` feature is enabled on Linux.
//! Uses `/dev/i2c-N` directly via rppal (no serial bridge needed).
//!
//! Three tools are provided:
//!
//! - [`RpiI2cScanTool`]  — probe all addresses 0x03–0x77 on a bus
//! - [`RpiI2cReadTool`]  — read N bytes from a register via SMBus block-read
//! - [`RpiI2cWriteTool`] — write hex-encoded bytes to a register via SMBus block-write

#![cfg(all(feature = "peripheral-rpi", target_os = "linux"))]

use crate::error::{Result, ZeptoError};
use crate::peripherals::board_profile::RPI_PROFILE;
use crate::peripherals::i2c::validate_hex;
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// RpiI2cScanTool
// ---------------------------------------------------------------------------

/// Tool: scan an I2C bus for connected devices using native rppal.
///
/// Probes every address in the standard 7-bit user range (0x03–0x77) by
/// attempting a 1-byte read. Addresses that acknowledge are returned as a
/// JSON array of hex strings.
pub struct RpiI2cScanTool;

#[async_trait]
impl Tool for RpiI2cScanTool {
    fn name(&self) -> &str {
        "rpi_i2c_scan"
    }

    fn description(&self) -> &str {
        "Scan an I2C bus on Raspberry Pi for connected devices. \
         Probes addresses 0x03–0x77 and returns a list of detected addresses."
    }

    fn compact_description(&self) -> &str {
        "Scan RPi I2C bus for devices"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bus": {
                    "type": "integer",
                    "description": "I2C bus number (default 1; RPi exposes bus 1 on GPIO2/3)",
                    "default": 1
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let bus = args.get("bus").and_then(|v| v.as_u64()).unwrap_or(1) as u8;

        // Validate bus against the RPi board profile.
        RPI_PROFILE.i2c_bus(bus).ok_or_else(|| {
            ZeptoError::Tool(format!(
                "I2C bus {bus} not found on board '{}' (valid buses: 1)",
                RPI_PROFILE.name
            ))
        })?;

        let found = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let mut detected = Vec::new();

            for addr in 0x03u16..=0x77 {
                // Open a fresh bus handle for each probe; rppal's I2c is not
                // Clone so we re-open per address.
                let mut i2c = rppal::i2c::I2c::with_bus(bus)
                    .map_err(|e| ZeptoError::Tool(format!("I2C open bus {bus}: {e}")))?;

                if i2c.set_slave_address(addr).is_err() {
                    continue;
                }

                // A 1-byte read is enough to detect device presence.
                let mut buf = [0u8; 1];
                if i2c.read(&mut buf).is_ok() {
                    detected.push(format!("0x{addr:02x}"));
                }
            }

            Ok(detected)
        })
        .await
        .map_err(|e| ZeptoError::Tool(format!("I2C scan join error: {e}")))??;

        Ok(ToolOutput::llm_only(
            serde_json::to_string(&found).unwrap_or_else(|_| "[]".into()),
        ))
    }
}

// ---------------------------------------------------------------------------
// RpiI2cReadTool
// ---------------------------------------------------------------------------

/// Tool: read bytes from an I2C device register on Raspberry Pi.
///
/// Uses SMBus block-read (`smbus_read_i2c_block_data`) to read up to 32 bytes
/// from a register. Returns the bytes as a lowercase hex string.
pub struct RpiI2cReadTool;

#[async_trait]
impl Tool for RpiI2cReadTool {
    fn name(&self) -> &str {
        "rpi_i2c_read"
    }

    fn description(&self) -> &str {
        "Read bytes from an I2C device register on Raspberry Pi. \
         Specify the bus, 7-bit device address, register, and byte count."
    }

    fn compact_description(&self) -> &str {
        "Read RPi I2C device register"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bus": {
                    "type": "integer",
                    "description": "I2C bus number (default 1)",
                    "default": 1
                },
                "addr": {
                    "type": "integer",
                    "description": "7-bit I2C device address (0-127)"
                },
                "reg": {
                    "type": "integer",
                    "description": "Register address to read from (0-255)"
                },
                "len": {
                    "type": "integer",
                    "description": "Number of bytes to read (default 1, max 32)",
                    "default": 1
                }
            },
            "required": ["addr", "reg"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let bus = args.get("bus").and_then(|v| v.as_u64()).unwrap_or(1) as u8;

        let addr = args
            .get("addr")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'addr'".into()))?;

        let reg = args
            .get("reg")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'reg'".into()))?;

        let len = args.get("len").and_then(|v| v.as_u64()).unwrap_or(1);

        // Validate bus against RPi board profile.
        RPI_PROFILE.i2c_bus(bus).ok_or_else(|| {
            ZeptoError::Tool(format!(
                "I2C bus {bus} not found on board '{}' (valid buses: 1)",
                RPI_PROFILE.name
            ))
        })?;

        // Validate 7-bit address range.
        if addr > 127 {
            return Err(ZeptoError::Tool(format!(
                "I2C address {addr:#04x} is out of range; must be 0-127 (7-bit)"
            )));
        }

        // Clamp len to the SMBus block-read maximum of 32.
        let len = len.min(32) as usize;
        let reg_u8 = reg as u8;
        let addr_u16 = addr as u16;

        let hex = tokio::task::spawn_blocking(move || -> Result<String> {
            let mut i2c = rppal::i2c::I2c::with_bus(bus)
                .map_err(|e| ZeptoError::Tool(format!("I2C open bus {bus}: {e}")))?;

            i2c.set_slave_address(addr_u16)
                .map_err(|e| ZeptoError::Tool(format!("I2C set address {addr_u16:#04x}: {e}")))?;

            let mut buf = vec![0u8; len];
            i2c.smbus_read_i2c_block_data(reg_u8, &mut buf)
                .map_err(|e| {
                    ZeptoError::Tool(format!(
                        "I2C read addr={addr_u16:#04x} reg={reg_u8:#04x}: {e}"
                    ))
                })?;

            Ok(buf.iter().map(|b| format!("{b:02x}")).collect::<String>())
        })
        .await
        .map_err(|e| ZeptoError::Tool(format!("I2C read join error: {e}")))??;

        Ok(ToolOutput::llm_only(hex))
    }
}

// ---------------------------------------------------------------------------
// RpiI2cWriteTool
// ---------------------------------------------------------------------------

/// Tool: write bytes to an I2C device register on Raspberry Pi.
///
/// Accepts a hex-encoded data string (e.g. `"FF00AB"`) and writes it to the
/// specified register using SMBus block-write (`smbus_write_i2c_block_data`).
pub struct RpiI2cWriteTool;

#[async_trait]
impl Tool for RpiI2cWriteTool {
    fn name(&self) -> &str {
        "rpi_i2c_write"
    }

    fn description(&self) -> &str {
        "Write bytes to an I2C device register on Raspberry Pi. \
         Specify the bus, 7-bit device address, register, and data as a hex string \
         (e.g. \"FF00AB\")."
    }

    fn compact_description(&self) -> &str {
        "Write RPi I2C device register"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bus": {
                    "type": "integer",
                    "description": "I2C bus number (default 1)",
                    "default": 1
                },
                "addr": {
                    "type": "integer",
                    "description": "7-bit I2C device address (0-127)"
                },
                "reg": {
                    "type": "integer",
                    "description": "Register address to write to (0-255)"
                },
                "data": {
                    "type": "string",
                    "description": "Data bytes as an even-length hex string (e.g. \"FF00AB\")"
                }
            },
            "required": ["addr", "reg", "data"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let bus = args.get("bus").and_then(|v| v.as_u64()).unwrap_or(1) as u8;

        let addr = args
            .get("addr")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'addr'".into()))?;

        let reg = args
            .get("reg")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'reg'".into()))?;

        let data = args
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing required parameter 'data'".into()))?;

        // Validate bus against RPi board profile.
        RPI_PROFILE.i2c_bus(bus).ok_or_else(|| {
            ZeptoError::Tool(format!(
                "I2C bus {bus} not found on board '{}' (valid buses: 1)",
                RPI_PROFILE.name
            ))
        })?;

        // Validate 7-bit address range.
        if addr > 127 {
            return Err(ZeptoError::Tool(format!(
                "I2C address {addr:#04x} is out of range; must be 0-127 (7-bit)"
            )));
        }

        // Validate hex data string.
        validate_hex(data).map_err(ZeptoError::Tool)?;

        // Decode hex string to bytes.
        let data_bytes: Vec<u8> = data
            .as_bytes()
            .chunks(2)
            .map(|chunk| {
                u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or("00"), 16).unwrap_or(0)
            })
            .collect();

        let reg_u8 = reg as u8;
        let addr_u16 = addr as u16;
        let byte_count = data_bytes.len();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut i2c = rppal::i2c::I2c::with_bus(bus)
                .map_err(|e| ZeptoError::Tool(format!("I2C open bus {bus}: {e}")))?;

            i2c.set_slave_address(addr_u16)
                .map_err(|e| ZeptoError::Tool(format!("I2C set address {addr_u16:#04x}: {e}")))?;

            i2c.smbus_write_i2c_block_data(reg_u8, &data_bytes)
                .map_err(|e| {
                    ZeptoError::Tool(format!(
                        "I2C write addr={addr_u16:#04x} reg={reg_u8:#04x}: {e}"
                    ))
                })?;

            Ok(())
        })
        .await
        .map_err(|e| ZeptoError::Tool(format!("I2C write join error: {e}")))??;

        Ok(ToolOutput::llm_only(format!(
            "wrote {byte_count} byte(s) to addr={addr:#04x} reg={reg:#04x} on bus {bus}"
        )))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpi_i2c_scan_tool_metadata() {
        let tool = RpiI2cScanTool;
        assert_eq!(tool.name(), "rpi_i2c_scan");
        assert_eq!(tool.category(), ToolCategory::Hardware);
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["bus"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.is_empty(), "bus should be optional for scan");
    }

    #[test]
    fn test_rpi_i2c_read_tool_metadata() {
        let tool = RpiI2cReadTool;
        assert_eq!(tool.name(), "rpi_i2c_read");
        assert_eq!(tool.category(), ToolCategory::Hardware);
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("addr")));
        assert!(required.contains(&json!("reg")));
        assert!(!required.contains(&json!("bus")), "bus should be optional");
        assert!(!required.contains(&json!("len")), "len should be optional");
    }

    #[test]
    fn test_rpi_i2c_write_tool_metadata() {
        let tool = RpiI2cWriteTool;
        assert_eq!(tool.name(), "rpi_i2c_write");
        assert_eq!(tool.category(), ToolCategory::Hardware);
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("addr")));
        assert!(required.contains(&json!("reg")));
        assert!(required.contains(&json!("data")));
        assert!(!required.contains(&json!("bus")), "bus should be optional");
    }

    #[test]
    fn test_rpi_i2c_bus_validation() {
        // RPi exposes bus 1 only.
        assert!(RPI_PROFILE.i2c_bus(1).is_some());
        assert!(RPI_PROFILE.i2c_bus(0).is_none());
        assert!(RPI_PROFILE.i2c_bus(2).is_none());
    }

    #[test]
    fn test_rpi_i2c_bus_1_pins() {
        let bus = RPI_PROFILE.i2c_bus(1).unwrap();
        assert_eq!(bus.sda_pin, 2, "SDA should be GPIO2");
        assert_eq!(bus.scl_pin, 3, "SCL should be GPIO3");
    }

    #[test]
    fn test_i2c_address_range() {
        // Valid probe range for i2cscan: 0x03..=0x77.
        for addr in [0x03u64, 0x48, 0x68, 0x77] {
            assert!(
                addr >= 3 && addr <= 0x77,
                "address {addr:#04x} should be in probe range"
            );
        }
        // Addresses outside 0x03..=0x77 are reserved or out of range.
        for addr in [0u64, 1, 2, 0x78, 128, 255] {
            assert!(
                addr < 3 || addr > 0x77,
                "address {addr:#04x} should be outside probe range"
            );
        }
    }

    #[test]
    fn test_scan_compact_description() {
        let tool = RpiI2cScanTool;
        assert!(!tool.compact_description().is_empty());
    }

    #[test]
    fn test_read_compact_description() {
        let tool = RpiI2cReadTool;
        assert!(!tool.compact_description().is_empty());
    }

    #[test]
    fn test_write_compact_description() {
        let tool = RpiI2cWriteTool;
        assert!(!tool.compact_description().is_empty());
    }

    #[test]
    fn test_scan_parameter_schema_structure() {
        let params = RpiI2cScanTool.parameters();
        assert_eq!(params["type"].as_str().unwrap(), "object");
        assert!(params["properties"]["bus"]["default"] == 1);
    }

    #[test]
    fn test_read_parameter_schema_structure() {
        let params = RpiI2cReadTool.parameters();
        assert_eq!(params["type"].as_str().unwrap(), "object");
        assert!(params["properties"]["bus"]["default"] == 1);
        assert!(params["properties"]["len"]["default"] == 1);
        assert!(params["properties"]["addr"].is_object());
        assert!(params["properties"]["reg"].is_object());
    }

    #[test]
    fn test_write_parameter_schema_structure() {
        let params = RpiI2cWriteTool.parameters();
        assert_eq!(params["type"].as_str().unwrap(), "object");
        assert!(params["properties"]["data"].is_object());
        assert_eq!(
            params["properties"]["data"]["type"].as_str().unwrap(),
            "string"
        );
    }

    #[test]
    fn test_hex_decode_roundtrip() {
        // Verify the hex-decode logic used in RpiI2cWriteTool.execute() is correct.
        let data = "DEADBEEF";
        let bytes: Vec<u8> = data
            .as_bytes()
            .chunks(2)
            .map(|chunk| {
                u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or("00"), 16).unwrap_or(0)
            })
            .collect();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_hex_decode_lowercase() {
        let data = "ff00ab";
        let bytes: Vec<u8> = data
            .as_bytes()
            .chunks(2)
            .map(|chunk| {
                u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or("00"), 16).unwrap_or(0)
            })
            .collect();
        assert_eq!(bytes, vec![0xFF, 0x00, 0xAB]);
    }

    #[test]
    fn test_i2c_address_7bit_boundary() {
        // 127 (0x7F) is the max valid 7-bit address.
        assert!(127u64 <= 127, "127 should be valid");
        // 128 and above are out of 7-bit range.
        assert!(128u64 > 127, "128 should be invalid");
    }
}
