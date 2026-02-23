//! Channel Manager for ZeptoClaw
//!
//! This module provides the `ChannelManager` which is responsible for:
//! - Registering and managing multiple communication channels
//! - Starting and stopping all channels
//! - Dispatching outbound messages to the appropriate channels

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::bus::{MessageBus, OutboundMessage};
use crate::config::Config;
use crate::error::Result;

use super::Channel;

type SharedChannel = Arc<Mutex<Box<dyn Channel>>>;

/// The `ChannelManager` manages the lifecycle of all communication channels.
///
/// It provides methods to:
/// - Register new channels
/// - Start and stop all channels
/// - Route outbound messages to the correct channel
/// - List all registered channels
///
/// # Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │                     ChannelManager                          │
/// │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐       │
/// │  │Telegram │  │ Discord │  │  Slack  │  │WhatsApp │  ...  │
/// │  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘       │
/// │       │            │            │            │              │
/// │       └────────────┴─────┬──────┴────────────┘              │
/// │                          │                                  │
/// │                    ┌─────┴─────┐                           │
/// │                    │MessageBus │                           │
/// │                    └───────────┘                           │
/// └─────────────────────────────────────────────────────────────┘
/// ```
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use zeptoclaw::bus::MessageBus;
/// use zeptoclaw::config::Config;
/// use zeptoclaw::channels::ChannelManager;
///
/// #[tokio::main]
/// async fn main() {
///     let bus = Arc::new(MessageBus::new());
///     let config = Config::default();
///     let manager = ChannelManager::new(bus, config);
///
///     // Register channels
///     // manager.register(Box::new(TelegramChannel::new(...))).await;
///
///     // Start all channels
///     manager.start_all().await.unwrap();
/// }
/// ```
pub struct ChannelManager {
    /// Map of channel name to channel instance
    channels: Arc<RwLock<HashMap<String, SharedChannel>>>,
    /// Reference to the message bus for routing
    bus: Arc<MessageBus>,
    /// Global configuration
    #[allow(dead_code)]
    config: Config,
    /// Shutdown signal sender for dispatcher
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver (cloneable)
    shutdown_rx: watch::Receiver<bool>,
    /// Handle to the dispatcher task (if running)
    dispatcher_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl ChannelManager {
    /// Creates a new `ChannelManager` with the given message bus and configuration.
    ///
    /// # Arguments
    ///
    /// * `bus` - The message bus for routing messages
    /// * `config` - The global configuration
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use zeptoclaw::bus::MessageBus;
    /// use zeptoclaw::config::Config;
    /// use zeptoclaw::channels::ChannelManager;
    ///
    /// let bus = Arc::new(MessageBus::new());
    /// let config = Config::default();
    /// let manager = ChannelManager::new(bus, config);
    /// ```
    pub fn new(bus: Arc<MessageBus>, config: Config) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            bus,
            config,
            shutdown_tx,
            shutdown_rx,
            dispatcher_handle: Arc::new(RwLock::new(None)),
        }
    }

    /// Registers a new channel with the manager.
    ///
    /// The channel is stored by its name and can be started later with `start_all()`.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel to register
    ///
    /// # Example
    ///
    /// ```ignore
    /// let manager = ChannelManager::new(bus, config);
    /// manager.register(Box::new(telegram_channel)).await;
    /// ```
    pub async fn register(&self, channel: Box<dyn Channel>) {
        let name = channel.name().to_string();
        info!("Registering channel: {}", name);
        let mut channels = self.channels.write().await;
        channels.insert(name, Arc::new(Mutex::new(channel)));
    }

    /// Returns a list of all registered channel names.
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use zeptoclaw::bus::MessageBus;
    /// use zeptoclaw::config::Config;
    /// use zeptoclaw::channels::ChannelManager;
    ///
    /// # tokio_test::block_on(async {
    /// let bus = Arc::new(MessageBus::new());
    /// let config = Config::default();
    /// let manager = ChannelManager::new(bus, config);
    ///
    /// let channels = manager.channels().await;
    /// assert!(channels.is_empty());
    /// # })
    /// ```
    pub async fn channels(&self) -> Vec<String> {
        let channels = self.channels.read().await;
        channels.keys().cloned().collect()
    }

    /// Returns the number of registered channels.
    pub async fn channel_count(&self) -> usize {
        let channels = self.channels.read().await;
        channels.len()
    }

    /// Checks if a channel with the given name is registered.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the channel to check
    pub async fn has_channel(&self, name: &str) -> bool {
        let channels = self.channels.read().await;
        channels.contains_key(name)
    }

    /// Starts all registered channels.
    ///
    /// This method:
    /// 1. Iterates over all registered channels and starts each one
    /// 2. Spawns a background task to dispatch outbound messages
    ///
    /// Errors from individual channels are logged but do not prevent
    /// other channels from starting.
    ///
    /// # Errors
    ///
    /// Returns `Ok(())` even if individual channels fail to start.
    /// Check logs for channel-specific errors.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let manager = ChannelManager::new(bus, config);
    /// manager.register(Box::new(telegram_channel)).await;
    /// manager.start_all().await?;
    /// ```
    pub async fn start_all(&self) -> Result<()> {
        // Check if dispatcher is already running to prevent multiple dispatcher tasks
        {
            let dispatcher_handle = self.dispatcher_handle.read().await;
            if let Some(ref handle) = *dispatcher_handle {
                if !handle.is_finished() {
                    warn!("Dispatcher already running, skipping start");
                    return Ok(());
                }
            }
        }

        let channels_to_start = {
            let channels = self.channels.read().await;
            channels
                .iter()
                .map(|(name, channel)| (name.clone(), Arc::clone(channel)))
                .collect::<Vec<_>>()
        };

        for (name, channel) in channels_to_start {
            info!("Starting channel: {}", name);
            let mut channel = channel.lock().await;
            if let Err(e) = channel.start().await {
                error!("Failed to start channel {}: {}", name, e);
            }
        }

        // Reset shutdown signal for fresh start
        let _ = self.shutdown_tx.send(false);

        // Start outbound dispatcher
        let bus = self.bus.clone();
        let channels_ref = self.channels.clone();
        let shutdown_rx = self.shutdown_rx.clone();
        let handle = tokio::spawn(async move {
            dispatch_outbound(bus, channels_ref, shutdown_rx).await;
        });

        // Store the handle so we can wait for it to stop
        let mut dispatcher_handle = self.dispatcher_handle.write().await;
        *dispatcher_handle = Some(handle);

        Ok(())
    }

    /// Stops all registered channels.
    ///
    /// Errors from individual channels are logged but do not prevent
    /// other channels from stopping.
    ///
    /// # Errors
    ///
    /// Returns `Ok(())` even if individual channels fail to stop.
    /// Check logs for channel-specific errors.
    ///
    /// # Example
    ///
    /// ```ignore
    /// manager.stop_all().await?;
    /// ```
    pub async fn stop_all(&self) -> Result<()> {
        // Signal the dispatcher to stop
        info!("Signaling dispatcher to stop");
        let _ = self.shutdown_tx.send(true);

        // Wait for dispatcher to finish (with timeout)
        let mut dispatcher_handle = self.dispatcher_handle.write().await;
        if let Some(handle) = dispatcher_handle.take() {
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(_) => info!("Dispatcher stopped cleanly"),
                Err(_) => warn!("Dispatcher did not stop within timeout"),
            }
        }

        // Stop all channels
        let channels_to_stop = {
            let channels = self.channels.read().await;
            channels
                .iter()
                .map(|(name, channel)| (name.clone(), Arc::clone(channel)))
                .collect::<Vec<_>>()
        };

        for (name, channel) in channels_to_stop {
            info!("Stopping channel: {}", name);
            let mut channel = channel.lock().await;
            if let Err(e) = channel.stop().await {
                error!("Failed to stop channel {}: {}", name, e);
            }
        }
        Ok(())
    }

    /// Sends a message to a specific channel.
    ///
    /// # Arguments
    ///
    /// * `channel_name` - The name of the channel to send to
    /// * `msg` - The outbound message to send
    ///
    /// # Errors
    ///
    /// Returns an error if the channel fails to send the message.
    /// If the channel is not found, a warning is logged and `Ok(())` is returned.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let msg = OutboundMessage::new("telegram", "chat123", "Hello!");
    /// manager.send("telegram", msg).await?;
    /// ```
    pub async fn send(&self, channel_name: &str, msg: OutboundMessage) -> Result<()> {
        let channel = {
            let channels = self.channels.read().await;
            channels.get(channel_name).cloned()
        };

        if let Some(channel) = channel {
            let channel = channel.lock().await;
            channel.send(msg).await
        } else {
            // Pseudo-channels (e.g. "heartbeat") have no outbound handler — debug-level only
            debug!(
                "Channel not found: {} (may be a pseudo-channel like 'heartbeat')",
                channel_name
            );
            Ok(())
        }
    }

    /// Returns a reference to the message bus.
    pub fn bus(&self) -> Arc<MessageBus> {
        self.bus.clone()
    }
}

/// Background task that dispatches outbound messages from the bus to channels.
///
/// This function runs in a loop, consuming outbound messages from the bus
/// and routing them to the appropriate channel based on the message's
/// `channel` field. It stops when the shutdown signal is received.
///
/// # Arguments
///
/// * `bus` - The message bus to consume from
/// * `channels` - The shared map of channels
/// * `shutdown_rx` - Receiver for shutdown signals
async fn dispatch_outbound(
    bus: Arc<MessageBus>,
    channels: Arc<RwLock<HashMap<String, SharedChannel>>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    info!("Outbound dispatcher started");
    loop {
        tokio::select! {
            // Check for shutdown signal
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Outbound dispatcher received shutdown signal");
                    break;
                }
            }
            // Wait for outbound messages
            msg = bus.consume_outbound() => {
                if let Some(msg) = msg {
                    let channel_name = msg.channel.clone();
                    let channel = {
                        let channels = channels.read().await;
                        channels.get(&channel_name).cloned()
                    };

                    if let Some(channel) = channel {
                        let channel = channel.lock().await;
                        if let Err(e) = channel.send(msg).await {
                            error!("Failed to send message to {}: {}", channel_name, e);
                        }
                    } else {
                        // Pseudo-channels (e.g. "heartbeat") have no outbound handler — debug-level only
                        debug!("Unknown channel for outbound message: {} (may be a pseudo-channel like 'heartbeat')", channel_name);
                    }
                } else {
                    // Channel closed
                    info!("Outbound channel closed");
                    break;
                }
            }
        }
    }
    info!("Outbound dispatcher stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A mock channel for testing
    struct MockChannel {
        name: String,
        running: Arc<AtomicBool>,
        allowlist: Vec<String>,
    }

    impl MockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                running: Arc::new(AtomicBool::new(false)),
                allowlist: Vec::new(),
            }
        }

        fn with_allowlist(name: &str, allowlist: Vec<String>) -> Self {
            Self {
                name: name.to_string(),
                running: Arc::new(AtomicBool::new(false)),
                allowlist,
            }
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn start(&mut self) -> Result<()> {
            self.running.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::SeqCst);
            Ok(())
        }

        async fn send(&self, _msg: OutboundMessage) -> Result<()> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }

        fn is_allowed(&self, user_id: &str) -> bool {
            self.allowlist.is_empty() || self.allowlist.contains(&user_id.to_string())
        }
    }

    #[tokio::test]
    async fn test_channel_manager_creation() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);
        assert!(manager.channels().await.is_empty());
    }

    #[tokio::test]
    async fn test_register_channel() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        let channel = MockChannel::new("test");
        manager.register(Box::new(channel)).await;

        let channels = manager.channels().await;
        assert_eq!(channels.len(), 1);
        assert!(channels.contains(&"test".to_string()));
    }

    #[tokio::test]
    async fn test_register_multiple_channels() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        manager
            .register(Box::new(MockChannel::new("telegram")))
            .await;
        manager
            .register(Box::new(MockChannel::new("discord")))
            .await;
        manager.register(Box::new(MockChannel::new("slack"))).await;

        assert_eq!(manager.channel_count().await, 3);
        assert!(manager.has_channel("telegram").await);
        assert!(manager.has_channel("discord").await);
        assert!(manager.has_channel("slack").await);
        assert!(!manager.has_channel("whatsapp").await);
    }

    #[tokio::test]
    async fn test_start_all() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        let channel = MockChannel::new("test");
        manager.register(Box::new(channel)).await;

        manager.start_all().await.unwrap();

        // Give the dispatcher task time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_stop_all() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        manager.register(Box::new(MockChannel::new("test"))).await;
        manager.start_all().await.unwrap();
        manager.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_double_start_prevented() {
        // Regression test: calling start_all() twice should not spawn multiple dispatchers
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        manager.register(Box::new(MockChannel::new("test"))).await;

        // First start
        manager.start_all().await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Second start should be a no-op (dispatcher already running)
        manager.start_all().await.unwrap();

        // Clean shutdown
        manager.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_send_to_unknown_channel() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        let msg = OutboundMessage::new("unknown", "chat123", "Hello");
        let result = manager.send("unknown", msg).await;
        assert!(result.is_ok()); // Should not error, just warn
    }

    #[tokio::test]
    async fn test_send_to_registered_channel() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus, config);

        manager.register(Box::new(MockChannel::new("test"))).await;

        let msg = OutboundMessage::new("test", "chat123", "Hello");
        let result = manager.send("test", msg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_channel_allowlist() {
        let channel = MockChannel::with_allowlist("test", vec!["user1".to_string()]);
        assert!(channel.is_allowed("user1"));
        assert!(!channel.is_allowed("user2"));
    }

    #[tokio::test]
    async fn test_channel_empty_allowlist() {
        let channel = MockChannel::new("test");
        assert!(channel.is_allowed("anyone"));
    }

    #[tokio::test]
    async fn test_bus_reference() {
        let bus = Arc::new(MessageBus::new());
        let config = Config::default();
        let manager = ChannelManager::new(bus.clone(), config);

        // The bus reference should be the same
        assert!(Arc::ptr_eq(&bus, &manager.bus()));
    }
}
