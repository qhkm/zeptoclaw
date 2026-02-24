//! Serial peripheral -- STM32 and similar boards over USB CDC/serial.
//!
//! Protocol: newline-delimited JSON.
//! Request:  `{"id":"1","cmd":"gpio_write","args":{"pin":13,"value":1}}`
//! Response: `{"id":"1","ok":true,"result":"done"}`
//!
//! This module is only compiled when the `hardware` feature is enabled.

use super::traits::Peripheral;
use crate::error::{Result, ZeptoError};
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

/// Allowed serial path patterns (security: deny arbitrary paths).
const ALLOWED_PATH_PREFIXES: &[&str] = &[
    "/dev/ttyACM",
    "/dev/ttyUSB",
    "/dev/tty.usbmodem",
    "/dev/cu.usbmodem",
    "/dev/tty.usbserial",
    "/dev/cu.usbserial",
    "COM",
];

/// Check if a serial path matches allowed patterns.
pub fn is_path_allowed(path: &str) -> bool {
    ALLOWED_PATH_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Timeout for serial request/response (seconds).
const SERIAL_TIMEOUT_SECS: u64 = 5;

/// Send a JSON request over serial and read the JSON response.
async fn send_request(port: &mut SerialStream, cmd: &str, args: Value) -> Result<Value> {
    static ID: AtomicU64 = AtomicU64::new(0);
    let id = ID.fetch_add(1, Ordering::Relaxed);
    let id_str = id.to_string();

    let req = json!({
        "id": id_str,
        "cmd": cmd,
        "args": args
    });
    let line = format!("{}\n", req);

    port.write_all(line.as_bytes())
        .await
        .map_err(|e| ZeptoError::Tool(format!("Serial write failed: {e}")))?;
    port.flush()
        .await
        .map_err(|e| ZeptoError::Tool(format!("Serial flush failed: {e}")))?;

    /// Maximum serial response size (64 KB) to prevent unbounded buffer growth.
    const MAX_RESPONSE_SIZE: usize = 64 * 1024;

    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    while port.read_exact(&mut b).await.is_ok() {
        if b[0] == b'\n' {
            break;
        }
        buf.push(b[0]);
        if buf.len() > MAX_RESPONSE_SIZE {
            return Err(ZeptoError::Tool(format!(
                "Serial response exceeded max size ({} bytes)",
                MAX_RESPONSE_SIZE
            )));
        }
    }

    let line_str = String::from_utf8_lossy(&buf);
    let resp: Value = serde_json::from_str(line_str.trim())
        .map_err(|e| ZeptoError::Tool(format!("Serial response parse error: {e}")))?;

    let resp_id = resp["id"].as_str().unwrap_or("");
    if resp_id != id_str {
        return Err(ZeptoError::Tool(format!(
            "Response id mismatch: expected {}, got {}",
            id_str, resp_id
        )));
    }

    Ok(resp)
}

/// Shared serial transport for tools.
pub(crate) struct SerialTransport {
    port: Mutex<SerialStream>,
}

impl SerialTransport {
    /// Send a request and parse the response.
    pub(crate) async fn request(&self, cmd: &str, args: Value) -> Result<String> {
        let mut port = self.port.lock().await;
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(SERIAL_TIMEOUT_SECS),
            send_request(&mut port, cmd, args),
        )
        .await
        .map_err(|_| {
            ZeptoError::Tool(format!(
                "Serial request timed out after {}s",
                SERIAL_TIMEOUT_SECS
            ))
        })??;

        let ok = resp["ok"].as_bool().unwrap_or(false);
        let result = resp["result"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| resp["result"].to_string());
        let error = resp["error"].as_str().map(String::from);

        if ok {
            Ok(result)
        } else {
            Err(ZeptoError::Tool(
                error.unwrap_or_else(|| "Unknown device error".to_string()),
            ))
        }
    }
}

/// Serial peripheral for STM32, Arduino, etc. over USB CDC.
pub struct SerialPeripheral {
    name: String,
    board_type: String,
    transport: Arc<SerialTransport>,
}

impl SerialPeripheral {
    /// Create and connect to a serial peripheral.
    pub fn connect_to(path: &str, board: &str, baud: u32) -> Result<Self> {
        if !is_path_allowed(path) {
            return Err(ZeptoError::Tool(format!(
                "Serial path not allowed: {}. Allowed: /dev/ttyACM*, /dev/ttyUSB*, /dev/tty.usbmodem*, /dev/cu.usbmodem*",
                path
            )));
        }

        let port = tokio_serial::new(path, baud)
            .open_native_async()
            .map_err(|e| ZeptoError::Tool(format!("Failed to open {}: {}", path, e)))?;

        let name = format!("{}-{}", board, path.replace('/', "_"));
        let transport = Arc::new(SerialTransport {
            port: Mutex::new(port),
        });

        Ok(Self {
            name,
            board_type: board.to_string(),
            transport,
        })
    }
}

#[async_trait]
impl Peripheral for SerialPeripheral {
    fn name(&self) -> &str {
        &self.name
    }

    fn board_type(&self) -> &str {
        &self.board_type
    }

    async fn connect(&mut self) -> Result<()> {
        // Connection established during connect_to()
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Serial port is closed when transport is dropped
        Ok(())
    }

    async fn health_check(&self) -> bool {
        self.transport.request("ping", json!({})).await.is_ok()
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(GpioReadTool {
                transport: self.transport.clone(),
            }),
            Box::new(GpioWriteTool {
                transport: self.transport.clone(),
            }),
        ]
    }
}

/// Tool: read GPIO pin value via serial.
struct GpioReadTool {
    transport: Arc<SerialTransport>,
}

#[async_trait]
impl Tool for GpioReadTool {
    fn name(&self) -> &str {
        "gpio_read"
    }

    fn description(&self) -> &str {
        "Read the value (0 or 1) of a GPIO pin on a connected peripheral (e.g. STM32 Nucleo)"
    }

    fn compact_description(&self) -> &str {
        "Read GPIO pin value"
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
                    "description": "GPIO pin number (e.g. 13 for LED on Nucleo)"
                }
            },
            "required": ["pin"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> crate::error::Result<ToolOutput> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing 'pin' parameter".into()))?;
        let result = self
            .transport
            .request("gpio_read", json!({ "pin": pin }))
            .await?;
        Ok(ToolOutput::llm_only(result))
    }
}

/// Tool: write GPIO pin value via serial.
struct GpioWriteTool {
    transport: Arc<SerialTransport>,
}

#[async_trait]
impl Tool for GpioWriteTool {
    fn name(&self) -> &str {
        "gpio_write"
    }

    fn description(&self) -> &str {
        "Set a GPIO pin high (1) or low (0) on a connected peripheral (e.g. turn on/off LED)"
    }

    fn compact_description(&self) -> &str {
        "Write GPIO pin value"
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
                    "description": "GPIO pin number"
                },
                "value": {
                    "type": "integer",
                    "description": "0 for low, 1 for high"
                }
            },
            "required": ["pin", "value"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> crate::error::Result<ToolOutput> {
        let pin = args
            .get("pin")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing 'pin' parameter".into()))?;
        let value = args
            .get("value")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ZeptoError::Tool("Missing 'value' parameter".into()))?;
        let result = self
            .transport
            .request("gpio_write", json!({ "pin": pin, "value": value }))
            .await?;
        Ok(ToolOutput::llm_only(result))
    }
}
