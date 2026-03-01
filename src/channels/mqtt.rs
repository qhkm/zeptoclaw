//! MQTT channel for ZeptoClaw.
//!
//! Enables two-way agent communication over MQTT for IoT devices. Inbound messages
//! arrive on configurable subscribe topics as newline-delimited JSON.
//! Responses are published back to the device-specific outbox topic.
//!
//! This is the "Telegram but for networked IoT devices" channel — an ESP32 over WiFi,
//! a fleet of RPis, or any MQTT-capable device can talk to the agent wirelessly.
//!
//! # Feature Gate
//!
//! This entire module requires the `mqtt` feature:
//! ```bash
//! cargo build --features mqtt
//! ```
//!
//! # Topic Structure
//!
//! ```text
//! zeptoclaw/inbox/{device_id}    # Device → Agent (inbound messages)
//! zeptoclaw/outbox/{device_id}   # Agent → Device (responses)
//! ```
//!
//! # Protocol
//!
//! Inbound (device → agent):
//! ```json
//! {"type":"message","text":"Hello","sender":"esp32-node-17"}
//! ```
//!
//! Outbound (agent → device):
//! ```json
//! {"type":"response","text":"Hi!"}
//! ```

#[cfg(feature = "mqtt")]
mod inner {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
    use serde::{Deserialize, Serialize};
    use tracing::{debug, error, info, warn};

    use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
    use crate::channels::types::{BaseChannelConfig, Channel};
    use crate::config::MqttChannelConfig;
    use crate::error::{Result, ZeptoError};

    /// Inbound message format from an MQTT device.
    #[derive(Debug, Deserialize)]
    struct MqttInbound {
        #[serde(rename = "type")]
        msg_type: String,
        text: String,
        #[serde(default)]
        sender: String,
    }

    /// Outbound message format sent to an MQTT device.
    #[derive(Debug, Serialize)]
    struct MqttOutbound {
        #[serde(rename = "type")]
        msg_type: String,
        text: String,
    }

    /// Map a config QoS value (0, 1, 2) to `rumqttc::QoS`.
    fn config_qos(qos: u8) -> QoS {
        match qos {
            0 => QoS::AtMostOnce,
            2 => QoS::ExactlyOnce,
            _ => QoS::AtLeastOnce,
        }
    }

    /// Extract the device ID from an MQTT topic.
    ///
    /// Given `"zeptoclaw/inbox/node-17"`, returns `Some("node-17")`.
    fn extract_device_id(topic: &str) -> Option<&str> {
        topic.rsplit('/').next()
    }

    /// MQTT channel — lets IoT devices chat with the agent over WiFi/network.
    pub struct MqttChannel {
        config: MqttChannelConfig,
        base_config: BaseChannelConfig,
        bus: Arc<MessageBus>,
        /// Atomic running flag shared with the spawned event-loop task.
        running: Arc<AtomicBool>,
        /// MQTT async client handle for publishing.
        client: Option<AsyncClient>,
        /// Shutdown signal sender.
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl MqttChannel {
        /// Create a new `MqttChannel` from the given config and bus.
        pub fn new(config: MqttChannelConfig, bus: Arc<MessageBus>) -> Self {
            let base_config = BaseChannelConfig {
                name: "mqtt".to_string(),
                allowlist: config.allow_from.clone(),
                deny_by_default: config.deny_by_default,
            };
            Self {
                config,
                base_config,
                bus,
                running: Arc::new(AtomicBool::new(false)),
                client: None,
                shutdown_tx: None,
            }
        }
    }

    #[async_trait]
    impl Channel for MqttChannel {
        fn name(&self) -> &str {
            "mqtt"
        }

        async fn start(&mut self) -> Result<()> {
            // Parse broker URL — rumqttc expects host and port separately.
            let url = &self.config.broker_url;
            let (host, port) = parse_broker_url(url).map_err(|e| {
                ZeptoError::Config(format!("Invalid MQTT broker URL '{}': {}", url, e))
            })?;

            let mut mqtt_options = MqttOptions::new(&self.config.client_id, &host, port);
            mqtt_options.set_keep_alive(std::time::Duration::from_secs(30));

            if !self.config.username.is_empty() {
                mqtt_options.set_credentials(&self.config.username, &self.config.password);
            }

            let (client, mut eventloop) = AsyncClient::new(mqtt_options, 64);

            // Subscribe to configured topics.
            let qos = config_qos(self.config.qos);
            for topic in &self.config.subscribe_topics {
                client.subscribe(topic, qos).await.map_err(|e| {
                    ZeptoError::Config(format!("MQTT subscribe to '{}' failed: {}", topic, e))
                })?;
                info!("MQTT: subscribed to {}", topic);
            }

            self.client = Some(client.clone());
            self.running.store(true, Ordering::SeqCst);

            let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            self.shutdown_tx = Some(shutdown_tx);

            // Spawn the MQTT event loop in a background task.
            let bus = self.bus.clone();
            let running = self.running.clone();
            let channel_name = "mqtt".to_string();
            let publish_prefix = self.config.publish_prefix.clone();
            let allow_from = self.config.allow_from.clone();
            let deny_by_default = self.config.deny_by_default;

            tokio::spawn(async move {
                loop {
                    let event = tokio::select! {
                        event = eventloop.poll() => event,
                        _ = &mut shutdown_rx => {
                            info!("MQTT channel shutdown signal received");
                            break;
                        }
                    };

                    match event {
                        Ok(Event::Incoming(Packet::Publish(publish))) => {
                            let topic = publish.topic.clone();
                            let payload = match std::str::from_utf8(&publish.payload) {
                                Ok(s) => s.to_string(),
                                Err(e) => {
                                    warn!("MQTT: invalid UTF-8 payload on {}: {}", topic, e);
                                    continue;
                                }
                            };

                            let trimmed = payload.trim();
                            if trimmed.is_empty() {
                                continue;
                            }

                            let inbound: MqttInbound = match serde_json::from_str(trimmed) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!(
                                        "MQTT: failed to parse JSON from {}: {} — {:?}",
                                        topic, e, trimmed
                                    );
                                    continue;
                                }
                            };

                            if inbound.msg_type != "message" {
                                debug!("MQTT: ignoring non-message type '{}'", inbound.msg_type);
                                continue;
                            }

                            // Derive sender: prefer explicit sender field, fall back to topic device ID.
                            let sender = if !inbound.sender.is_empty() {
                                inbound.sender.clone()
                            } else if let Some(device_id) = extract_device_id(&topic) {
                                device_id.to_string()
                            } else {
                                "mqtt-device".to_string()
                            };

                            // Access control check.
                            let allowed = if allow_from.is_empty() {
                                !deny_by_default
                            } else {
                                allow_from.contains(&sender)
                            };

                            if !allowed {
                                warn!("MQTT: message from '{}' denied by allowlist", sender);
                                continue;
                            }

                            // Use publish_prefix/sender as chat_id for response routing.
                            let chat_id = format!("{}/{}", publish_prefix, sender);
                            let msg = InboundMessage::new(
                                &channel_name,
                                &sender,
                                &chat_id,
                                &inbound.text,
                            );

                            if let Err(e) = bus.publish_inbound(msg).await {
                                error!("MQTT: failed to publish inbound message: {}", e);
                            }
                        }
                        Ok(Event::Incoming(Packet::ConnAck(_))) => {
                            info!("MQTT: connected to broker");
                        }
                        Ok(_) => {
                            // Other events (PingResp, SubAck, etc.) — ignore.
                        }
                        Err(e) => {
                            error!("MQTT connection error: {}", e);
                            // rumqttc auto-reconnects, so we continue the loop.
                        }
                    }
                }

                running.store(false, Ordering::SeqCst);
                info!("MQTT channel stopped");
            });

            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::SeqCst);

            if let Some(client) = &self.client {
                // Attempt graceful disconnect.
                let _ = client.disconnect().await;
            }

            if let Some(tx) = self.shutdown_tx.take() {
                if tx.send(()).is_err() {
                    warn!("MQTT shutdown receiver already dropped");
                }
            }

            self.client = None;
            Ok(())
        }

        async fn send(&self, msg: OutboundMessage) -> Result<()> {
            let client = match &self.client {
                Some(c) => c,
                None => return Err(ZeptoError::Config("MQTT channel not started".to_string())),
            };

            let outbound = MqttOutbound {
                msg_type: "response".to_string(),
                text: msg.content,
            };
            let payload = serde_json::to_string(&outbound)
                .map_err(|e| ZeptoError::Tool(format!("MQTT serialize error: {e}")))?;

            // Publish to the chat_id topic (which is already "prefix/device_id").
            let topic = &msg.chat_id;
            let qos = QoS::AtLeastOnce;

            client
                .publish(topic, qos, false, payload.as_bytes())
                .await
                .map_err(|e| ZeptoError::Tool(format!("MQTT publish error: {e}")))?;

            Ok(())
        }

        fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }

        fn is_allowed(&self, user_id: &str) -> bool {
            self.base_config.is_allowed(user_id)
        }
    }

    /// Parse an MQTT broker URL into (host, port).
    ///
    /// Accepts formats: `mqtt://host:port`, `host:port`, `host` (default port 1883).
    fn parse_broker_url(url: &str) -> std::result::Result<(String, u16), String> {
        let stripped = url
            .strip_prefix("mqtt://")
            .or_else(|| url.strip_prefix("mqtts://"))
            .or_else(|| url.strip_prefix("tcp://"))
            .unwrap_or(url);

        if let Some((host, port_str)) = stripped.rsplit_once(':') {
            let port: u16 = port_str
                .parse()
                .map_err(|_| format!("invalid port: {}", port_str))?;
            Ok((host.to_string(), port))
        } else {
            Ok((stripped.to_string(), 1883))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::bus::MessageBus;
        use crate::config::MqttChannelConfig;

        fn make_channel(config: MqttChannelConfig) -> MqttChannel {
            let bus = Arc::new(MessageBus::new());
            MqttChannel::new(config, bus)
        }

        #[test]
        fn test_mqtt_channel_name() {
            let ch = make_channel(MqttChannelConfig::default());
            assert_eq!(ch.name(), "mqtt");
        }

        #[test]
        fn test_mqtt_channel_not_running_initially() {
            let ch = make_channel(MqttChannelConfig::default());
            assert!(!ch.is_running());
        }

        #[test]
        fn test_mqtt_channel_allowlist() {
            let ch = make_channel(MqttChannelConfig {
                allow_from: vec!["esp32-node-17".to_string()],
                ..Default::default()
            });
            assert!(ch.is_allowed("esp32-node-17"));
            assert!(!ch.is_allowed("esp32-node-99"));
        }

        #[test]
        fn test_mqtt_channel_deny_by_default() {
            let ch = make_channel(MqttChannelConfig {
                allow_from: vec![],
                deny_by_default: true,
                ..Default::default()
            });
            assert!(!ch.is_allowed("anyone"));
        }

        #[test]
        fn test_mqtt_channel_open_by_default() {
            let ch = make_channel(MqttChannelConfig::default());
            assert!(ch.is_allowed("anyone"));
        }

        #[test]
        fn test_mqtt_outbound_serialization() {
            let outbound = MqttOutbound {
                msg_type: "response".to_string(),
                text: "Restarting I2C bus".to_string(),
            };
            let json = serde_json::to_string(&outbound).unwrap();
            assert!(json.contains("\"type\":\"response\""));
            assert!(json.contains("\"text\":\"Restarting I2C bus\""));
        }

        #[test]
        fn test_mqtt_inbound_deserialization() {
            let raw = r#"{"type":"message","text":"I2C timeout","sender":"esp32-node-17"}"#;
            let inbound: MqttInbound = serde_json::from_str(raw).unwrap();
            assert_eq!(inbound.msg_type, "message");
            assert_eq!(inbound.text, "I2C timeout");
            assert_eq!(inbound.sender, "esp32-node-17");
        }

        #[test]
        fn test_mqtt_inbound_deserialization_no_sender() {
            let raw = r#"{"type":"message","text":"Hello"}"#;
            let inbound: MqttInbound = serde_json::from_str(raw).unwrap();
            assert_eq!(inbound.msg_type, "message");
            assert_eq!(inbound.text, "Hello");
            assert_eq!(inbound.sender, "");
        }

        #[test]
        fn test_mqtt_channel_running_flag_is_atomic() {
            let ch = make_channel(MqttChannelConfig::default());
            assert!(!ch.is_running());
            ch.running.store(true, Ordering::SeqCst);
            assert!(ch.is_running());
            ch.running.store(false, Ordering::SeqCst);
            assert!(!ch.is_running());
        }

        #[test]
        fn test_extract_device_id() {
            assert_eq!(
                extract_device_id("zeptoclaw/inbox/node-17"),
                Some("node-17")
            );
            assert_eq!(
                extract_device_id("zeptoclaw/inbox/esp32-0"),
                Some("esp32-0")
            );
            assert_eq!(extract_device_id("single"), Some("single"));
            assert_eq!(extract_device_id(""), Some(""));
        }

        #[test]
        fn test_parse_broker_url_full() {
            let (host, port) = parse_broker_url("mqtt://192.168.1.100:1883").unwrap();
            assert_eq!(host, "192.168.1.100");
            assert_eq!(port, 1883);
        }

        #[test]
        fn test_parse_broker_url_no_scheme() {
            let (host, port) = parse_broker_url("broker.example.com:8883").unwrap();
            assert_eq!(host, "broker.example.com");
            assert_eq!(port, 8883);
        }

        #[test]
        fn test_parse_broker_url_default_port() {
            let (host, port) = parse_broker_url("mqtt://localhost").unwrap();
            assert_eq!(host, "localhost");
            assert_eq!(port, 1883);
        }

        #[test]
        fn test_parse_broker_url_host_only() {
            let (host, port) = parse_broker_url("broker.local").unwrap();
            assert_eq!(host, "broker.local");
            assert_eq!(port, 1883);
        }

        #[test]
        fn test_parse_broker_url_tcp_scheme() {
            let (host, port) = parse_broker_url("tcp://10.0.0.1:1883").unwrap();
            assert_eq!(host, "10.0.0.1");
            assert_eq!(port, 1883);
        }

        #[test]
        fn test_parse_broker_url_invalid_port() {
            assert!(parse_broker_url("mqtt://host:notaport").is_err());
        }

        #[test]
        fn test_config_qos_mapping() {
            assert_eq!(config_qos(0), QoS::AtMostOnce);
            assert_eq!(config_qos(1), QoS::AtLeastOnce);
            assert_eq!(config_qos(2), QoS::ExactlyOnce);
            assert_eq!(config_qos(3), QoS::AtLeastOnce); // default
        }

        #[test]
        fn test_mqtt_config_defaults() {
            let config = MqttChannelConfig::default();
            assert!(!config.enabled);
            assert_eq!(config.broker_url, "mqtt://localhost:1883");
            assert_eq!(config.client_id, "zeptoclaw-agent");
            assert_eq!(config.subscribe_topics, vec!["zeptoclaw/inbox/#"]);
            assert_eq!(config.publish_prefix, "zeptoclaw/outbox");
            assert_eq!(config.qos, 1);
            assert!(config.username.is_empty());
            assert!(config.password.is_empty());
        }
    }
}

#[cfg(feature = "mqtt")]
pub use inner::MqttChannel;
