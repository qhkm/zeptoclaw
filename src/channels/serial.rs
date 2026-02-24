//! Serial (UART) channel for ZeptoClaw.
//!
//! Enables two-way agent communication over a serial port. Inbound messages
//! arrive as newline-delimited JSON (`{"type":"message","text":"...","sender":"..."}`).
//! Responses are written back as `{"type":"response","text":"..."}`.
//!
//! This is the "Telegram but for embedded devices" channel — an ESP32, Arduino,
//! or STM32 can talk to the agent over USB-serial.
//!
//! # Feature Gate
//!
//! This entire module requires the `hardware` feature:
//! ```bash
//! cargo build --features hardware
//! ```
//!
//! # Protocol
//!
//! Inbound (device → agent):
//! ```json
//! {"type":"message","text":"Hello","sender":"esp32-0"}
//! ```
//!
//! Outbound (agent → device):
//! ```json
//! {"type":"response","text":"Hi!"}
//! ```

#[cfg(feature = "hardware")]
mod inner {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;
    use tokio_serial::SerialPortBuilderExt;
    use tracing::{debug, error, warn};

    use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
    use crate::channels::types::{BaseChannelConfig, Channel};
    use crate::config::SerialChannelConfig;
    use crate::error::{Result, ZeptoError};
    use crate::peripherals::validate_serial_path;

    /// Inbound message format from the serial device.
    #[derive(Debug, Deserialize)]
    struct SerialInbound {
        #[serde(rename = "type")]
        msg_type: String,
        text: String,
        #[serde(default)]
        sender: String,
    }

    /// Outbound message format sent to the serial device.
    #[derive(Debug, Serialize)]
    struct SerialOutbound {
        #[serde(rename = "type")]
        msg_type: String,
        text: String,
    }

    /// Serial (UART) channel — lets embedded devices chat with the agent.
    pub struct SerialChannel {
        config: SerialChannelConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
        running: bool,
        port: Option<Arc<Mutex<tokio_serial::SerialStream>>>,
    }

    impl SerialChannel {
        /// Create a new `SerialChannel` from the given config and bus.
        pub fn new(config: SerialChannelConfig, bus: Arc<MessageBus>) -> Self {
            let base_config = BaseChannelConfig {
                name: "serial".to_string(),
                allowlist: config.allow_from.clone(),
                deny_by_default: config.deny_by_default,
            };
            Self {
                config,
                base_config,
                bus,
                running: false,
                port: None,
            }
        }
    }

    #[async_trait]
    impl Channel for SerialChannel {
        fn name(&self) -> &str {
            "serial"
        }

        async fn start(&mut self) -> Result<()> {
            validate_serial_path(&self.config.port).map_err(ZeptoError::Config)?;

            let stream = tokio_serial::new(&self.config.port, self.config.baud_rate)
                .open_native_async()
                .map_err(|e| {
                    ZeptoError::Config(format!(
                        "Failed to open serial port {}: {}",
                        self.config.port, e
                    ))
                })?;

            let port = Arc::new(Mutex::new(stream));
            self.port = Some(port.clone());
            self.running = true;

            // Spawn the read loop in a background task.
            let bus = self.bus.clone();
            let channel_name = "serial".to_string();
            let allow_from = self.config.allow_from.clone();
            let deny_by_default = self.config.deny_by_default;

            tokio::spawn(async move {
                loop {
                    // Acquire lock, read one line, then release.
                    let line = {
                        let mut guard = port.lock().await;
                        let mut reader = BufReader::new(&mut *guard);
                        let mut buf = String::new();
                        match reader.read_line(&mut buf).await {
                            Ok(0) => {
                                // EOF — port closed.
                                break;
                            }
                            Ok(_) => buf,
                            Err(e) => {
                                error!("Serial read error: {}", e);
                                break;
                            }
                        }
                    };

                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let inbound: SerialInbound = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(
                                "Serial: failed to parse inbound JSON: {} — {:?}",
                                e, trimmed
                            );
                            continue;
                        }
                    };

                    if inbound.msg_type != "message" {
                        debug!("Serial: ignoring non-message type '{}'", inbound.msg_type);
                        continue;
                    }

                    let sender = if inbound.sender.is_empty() {
                        "serial-device".to_string()
                    } else {
                        inbound.sender.clone()
                    };

                    // Access control check.
                    let allowed = if allow_from.is_empty() {
                        !deny_by_default
                    } else {
                        allow_from.contains(&sender)
                    };

                    if !allowed {
                        warn!("Serial: message from '{}' denied by allowlist", sender);
                        continue;
                    }

                    let msg =
                        InboundMessage::new(&channel_name, &sender, &channel_name, &inbound.text);

                    if let Err(e) = bus.publish_inbound(msg).await {
                        error!("Serial: failed to publish inbound message: {}", e);
                    }
                }
            });

            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.running = false;
            self.port = None;
            Ok(())
        }

        async fn send(&self, msg: OutboundMessage) -> Result<()> {
            let port = match &self.port {
                Some(p) => p.clone(),
                None => return Err(ZeptoError::Config("Serial channel not started".to_string())),
            };

            let outbound = SerialOutbound {
                msg_type: "response".to_string(),
                text: msg.content,
            };
            let mut line = serde_json::to_string(&outbound)
                .map_err(|e| ZeptoError::Tool(format!("Serial serialize error: {e}")))?;
            line.push('\n');

            let mut guard = port.lock().await;
            guard
                .write_all(line.as_bytes())
                .await
                .map_err(|e| ZeptoError::Tool(format!("Serial write error: {e}")))?;
            guard
                .flush()
                .await
                .map_err(|e| ZeptoError::Tool(format!("Serial flush error: {e}")))?;
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn is_allowed(&self, user_id: &str) -> bool {
            self.base_config.is_allowed(user_id)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::bus::MessageBus;
        use crate::config::SerialChannelConfig;

        fn make_channel(config: SerialChannelConfig) -> SerialChannel {
            let bus = Arc::new(MessageBus::new());
            SerialChannel::new(config, bus)
        }

        #[test]
        fn test_serial_channel_name() {
            let ch = make_channel(SerialChannelConfig {
                port: "/dev/ttyUSB0".to_string(),
                ..Default::default()
            });
            assert_eq!(ch.name(), "serial");
        }

        #[test]
        fn test_serial_channel_not_running_initially() {
            let ch = make_channel(SerialChannelConfig {
                port: "/dev/ttyUSB0".to_string(),
                ..Default::default()
            });
            assert!(!ch.is_running());
        }

        #[test]
        fn test_serial_channel_allowlist() {
            let ch = make_channel(SerialChannelConfig {
                port: "/dev/ttyUSB0".to_string(),
                allow_from: vec!["esp32-0".to_string()],
                ..Default::default()
            });
            assert!(ch.is_allowed("esp32-0"));
            assert!(!ch.is_allowed("esp32-1"));
        }

        #[test]
        fn test_serial_channel_deny_by_default() {
            let ch = make_channel(SerialChannelConfig {
                port: "/dev/ttyUSB0".to_string(),
                allow_from: vec![],
                deny_by_default: true,
                ..Default::default()
            });
            assert!(!ch.is_allowed("anyone"));
        }

        #[test]
        fn test_serial_outbound_serialization() {
            let outbound = SerialOutbound {
                msg_type: "response".to_string(),
                text: "Hi!".to_string(),
            };
            let json = serde_json::to_string(&outbound).unwrap();
            assert!(json.contains("\"type\":\"response\""));
            assert!(json.contains("\"text\":\"Hi!\""));
        }

        #[test]
        fn test_serial_inbound_deserialization() {
            let raw = r#"{"type":"message","text":"Hello","sender":"esp32-0"}"#;
            let inbound: SerialInbound = serde_json::from_str(raw).unwrap();
            assert_eq!(inbound.msg_type, "message");
            assert_eq!(inbound.text, "Hello");
            assert_eq!(inbound.sender, "esp32-0");
        }
    }
}

#[cfg(feature = "hardware")]
pub use inner::SerialChannel;
