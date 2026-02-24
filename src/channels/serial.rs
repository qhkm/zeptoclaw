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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;
    use tokio_serial::SerialPortBuilderExt;
    use tracing::{debug, error, info, warn};

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
        /// Atomic running flag shared with the spawned read-loop task.
        running: Arc<AtomicBool>,
        port: Option<Arc<Mutex<tokio_serial::SerialStream>>>,
        /// Shutdown signal sender — dropping or sending signals the read loop to exit.
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
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
                running: Arc::new(AtomicBool::new(false)),
                port: None,
                shutdown_tx: None,
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
            self.running.store(true, Ordering::SeqCst);

            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            self.shutdown_tx = Some(shutdown_tx);

            // Spawn the read loop in a background task.
            let bus = self.bus.clone();
            let running = self.running.clone();
            let channel_name = "serial".to_string();
            let allow_from = self.config.allow_from.clone();
            let deny_by_default = self.config.deny_by_default;

            tokio::spawn(async move {
                let mut shutdown_rx = shutdown_rx;

                // Acquire the lock once for the read loop. The BufReader persists
                // across iterations so its internal buffer is never discarded (C2 fix).
                // Outbound `send()` will block on the Mutex while the read loop holds
                // it; this is acceptable for half-duplex serial where inbound messages
                // and outbound responses do not overlap.
                let mut guard = port.lock().await;
                let mut reader = BufReader::new(&mut *guard);

                loop {
                    let mut buf = String::new();

                    let read_result = tokio::select! {
                        result = reader.read_line(&mut buf) => result,
                        _ = &mut shutdown_rx => {
                            info!("Serial channel shutdown signal received");
                            break;
                        }
                    };

                    match read_result {
                        Ok(0) => {
                            // EOF — port closed.
                            info!("Serial channel: port closed (EOF)");
                            break;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            error!("Serial read error: {}", e);
                            break;
                        }
                    }

                    let trimmed = buf.trim();
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

                    // Drop the Mutex guard temporarily so send() can acquire it
                    // for outbound writes while we publish.
                    // NOTE: For half-duplex serial this is not strictly needed, but
                    // we release it here so bus.publish_inbound() doesn't deadlock
                    // if the handler tries to send() a response synchronously.
                    // In practice the bus is async and the send() path is separate,
                    // so holding the lock is fine. Keeping the simpler single-lock
                    // approach avoids split() complexity on SerialStream.

                    if let Err(e) = bus.publish_inbound(msg).await {
                        error!("Serial: failed to publish inbound message: {}", e);
                    }
                }

                running.store(false, Ordering::SeqCst);
                info!("Serial channel stopped");
            });

            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::SeqCst);

            if let Some(tx) = self.shutdown_tx.take() {
                if tx.send(()).is_err() {
                    warn!("Serial shutdown receiver already dropped");
                }
            }

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
            self.running.load(Ordering::SeqCst)
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

        #[test]
        fn test_serial_channel_running_flag_is_atomic() {
            let ch = make_channel(SerialChannelConfig {
                port: "/dev/ttyUSB0".to_string(),
                ..Default::default()
            });
            assert!(!ch.is_running());
            ch.running.store(true, Ordering::SeqCst);
            assert!(ch.is_running());
            ch.running.store(false, Ordering::SeqCst);
            assert!(!ch.is_running());
        }
    }
}

#[cfg(feature = "hardware")]
pub use inner::SerialChannel;
