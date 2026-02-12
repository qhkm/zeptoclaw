//! Telegram Channel Implementation
//!
//! This module provides a Telegram bot channel for ZeptoClaw using the teloxide library.
//! It handles receiving messages from Telegram users and sending responses back.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────┐         ┌──────────────────┐
//! │   Telegram API   │ <────── │  TelegramChannel │
//! │   (Bot Father)   │ ──────> │   (teloxide)     │
//! └──────────────────┘         └────────┬─────────┘
//!                                       │
//!                                       │ InboundMessage
//!                                       ▼
//!                              ┌──────────────────┐
//!                              │    MessageBus    │
//!                              └──────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use zeptoclaw::bus::MessageBus;
//! use zeptoclaw::config::TelegramConfig;
//! use zeptoclaw::channels::TelegramChannel;
//!
//! let config = TelegramConfig {
//!     enabled: true,
//!     token: "BOT_TOKEN".to_string(),
//!     allow_from: vec![],
//! };
//! let bus = Arc::new(MessageBus::new());
//! let channel = TelegramChannel::new(config, bus);
//! ```

use async_trait::async_trait;
use futures::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::TelegramConfig;
use crate::error::{PicoError, Result};

use super::{BaseChannelConfig, Channel};

/// Telegram channel implementation using teloxide.
///
/// This channel connects to Telegram's Bot API to receive and send messages.
/// It supports:
/// - Receiving text messages from users
/// - Sending text responses
/// - Allowlist-based access control
/// - Graceful shutdown
///
/// # Configuration
///
/// The channel requires a valid bot token from BotFather and optionally
/// an allowlist of user IDs.
pub struct TelegramChannel {
    /// Telegram-specific configuration (token, allowlist, etc.)
    config: TelegramConfig,
    /// Base channel configuration (name, common settings)
    base_config: BaseChannelConfig,
    /// Reference to the message bus for publishing inbound messages
    bus: Arc<MessageBus>,
    /// Atomic flag indicating if the channel is currently running.
    /// Wrapped in Arc so the spawned polling task can update it.
    running: Arc<AtomicBool>,
    /// Sender to signal shutdown to the polling task
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Cached bot instance for sending messages (avoids rebuilding HTTP client)
    bot: Option<teloxide::Bot>,
}

impl TelegramChannel {
    /// Creates a new Telegram channel with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Telegram-specific configuration (token, allowlist)
    /// * `bus` - Reference to the message bus for publishing messages
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use zeptoclaw::bus::MessageBus;
    /// use zeptoclaw::config::TelegramConfig;
    /// use zeptoclaw::channels::TelegramChannel;
    ///
    /// let config = TelegramConfig {
    ///     enabled: true,
    ///     token: "BOT_TOKEN".to_string(),
    ///     allow_from: vec!["user123".to_string()],
    /// };
    /// let bus = Arc::new(MessageBus::new());
    /// let channel = TelegramChannel::new(config, bus);
    ///
    /// assert_eq!(channel.name(), "telegram");
    /// assert!(!channel.is_running());
    /// ```
    pub fn new(config: TelegramConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "telegram".to_string(),
            allowlist: config.allow_from.clone(),
        };
        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
            bot: None,
        }
    }

    /// Returns a reference to the Telegram configuration.
    pub fn telegram_config(&self) -> &TelegramConfig {
        &self.config
    }

    /// Returns whether the channel is enabled in configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Build a Telegram bot client with explicit proxy behavior.
    ///
    /// We disable automatic system proxy detection to avoid macOS dynamic-store
    /// crashes seen in some sandboxed/runtime environments.
    fn build_bot(token: &str) -> Result<teloxide::Bot> {
        let client = teloxide::net::default_reqwest_settings()
            .no_proxy()
            .build()
            .map_err(|e| {
                PicoError::Channel(format!("Failed to build Telegram HTTP client: {}", e))
            })?;
        Ok(teloxide::Bot::with_client(token.to_string(), client))
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    /// Returns the channel name ("telegram").
    fn name(&self) -> &str {
        "telegram"
    }

    /// Starts the Telegram bot polling loop.
    ///
    /// This method:
    /// 1. Creates a teloxide Bot instance with the configured token
    /// 2. Sets up a message handler that publishes to the message bus
    /// 3. Spawns a background task for polling
    /// 4. Returns immediately (non-blocking)
    ///
    /// # Errors
    ///
    /// Returns `Ok(())` if the bot starts successfully.
    /// The actual polling errors are logged but don't stop the channel.
    async fn start(&mut self) -> Result<()> {
        // Prevent double-start
        if self.running.swap(true, Ordering::SeqCst) {
            info!("Telegram channel already running");
            return Ok(());
        }

        if !self.config.enabled {
            warn!("Telegram channel is disabled in configuration");
            self.running.store(false, Ordering::SeqCst);
            return Ok(());
        }

        if self.config.token.is_empty() {
            error!("Telegram bot token is empty");
            self.running.store(false, Ordering::SeqCst);
            return Err(PicoError::Config("Telegram bot token is empty".into()));
        }

        info!("Starting Telegram channel");

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Clone values for the spawned task
        let token = self.config.token.clone();
        let bus = self.bus.clone();
        let allowlist = self.config.allow_from.clone();
        // Share the same running flag with the spawned task so state stays in sync
        let running_clone = Arc::clone(&self.running);

        let bot = match Self::build_bot(&token) {
            Ok(bot) => bot,
            Err(e) => {
                self.running.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };

        // Cache the bot for send() calls
        self.bot = Some(bot.clone());

        // Spawn the bot polling task
        tokio::spawn(async move {
            use teloxide::prelude::*;

            let task_result = std::panic::AssertUnwindSafe(async move {
                // Perform a startup check so connectivity/token errors are surfaced
                // as logged channel failures instead of dispatcher panics.
                if let Err(e) = bot.get_me().await {
                    error!("Telegram startup check failed: {}", e);
                    return;
                }

                // Create the handler for incoming messages
                // Note: dptree injects dependencies separately, not as tuples
                let handler =
                    Update::filter_message().endpoint(
                        |_bot: Bot,
                         msg: Message,
                         bus: Arc<MessageBus>,
                         allowlist: Vec<String>| async move {
                            // Extract user ID
                            let user_id = msg
                                .from()
                                .map(|u| u.id.0.to_string())
                                .unwrap_or_else(|| "unknown".to_string());

                            // Check allowlist (empty = allow all)
                            if !allowlist.is_empty() && !allowlist.contains(&user_id) {
                                info!(
                                    "Telegram: User {} not in allowlist, ignoring message",
                                    user_id
                                );
                                return Ok(());
                            }

                            // Only process text messages
                            if let Some(text) = msg.text() {
                                let chat_id = msg.chat.id.0.to_string();

                                info!(
                                    "Telegram: Received message from user {} in chat {}: {}",
                                    user_id,
                                    chat_id,
                                    if text.len() > 50 {
                                        format!("{}...", &text[..50])
                                    } else {
                                        text.to_string()
                                    }
                                );

                                // Create and publish the inbound message
                                let inbound =
                                    InboundMessage::new("telegram", &user_id, &chat_id, text);

                                if let Err(e) = bus.publish_inbound(inbound).await {
                                    error!("Failed to publish inbound message to bus: {}", e);
                                }
                            }

                            // Acknowledge the message (required by teloxide)
                            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                        },
                    );

                // Build the dispatcher with dependencies
                let mut dispatcher = Dispatcher::builder(bot, handler)
                    .dependencies(dptree::deps![bus, allowlist])
                    .build();

                info!("Telegram bot dispatcher started, waiting for messages...");

                // Run until shutdown signal
                tokio::select! {
                    _ = dispatcher.dispatch() => {
                        info!("Telegram dispatcher completed");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Telegram channel shutdown signal received");
                    }
                }
            })
            .catch_unwind()
            .await;

            if task_result.is_err() {
                error!("Telegram polling task panicked");
            }

            running_clone.store(false, Ordering::SeqCst);
            info!("Telegram polling task stopped");
        });

        Ok(())
    }

    /// Stops the Telegram bot polling loop.
    ///
    /// Sends a shutdown signal to the polling task and waits briefly
    /// for it to terminate.
    async fn stop(&mut self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            info!("Telegram channel already stopped");
            return Ok(());
        }

        info!("Stopping Telegram channel");

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            if tx.send(()).await.is_err() {
                warn!("Telegram shutdown channel already closed");
            }
        }

        // Clear cached bot
        self.bot = None;

        info!("Telegram channel stopped");
        Ok(())
    }

    /// Sends an outbound message to a Telegram chat.
    ///
    /// # Arguments
    ///
    /// * `msg` - The outbound message containing chat_id and content
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The chat_id cannot be parsed as an integer
    /// - The Telegram API request fails
    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        use teloxide::prelude::*;
        use teloxide::types::ChatId;

        if !self.running.load(Ordering::SeqCst) {
            warn!("Telegram channel not running, cannot send message");
            return Err(PicoError::Channel(
                "Telegram channel not running".to_string(),
            ));
        }

        // Parse the chat ID
        let chat_id: i64 = msg.chat_id.parse().map_err(|_| {
            PicoError::Channel(format!("Invalid Telegram chat ID: {}", msg.chat_id))
        })?;

        info!("Telegram: Sending message to chat {}", chat_id);

        // Use cached bot instance
        let bot = self
            .bot
            .as_ref()
            .ok_or_else(|| PicoError::Channel("Telegram bot not initialized".to_string()))?;

        bot.send_message(ChatId(chat_id), &msg.content)
            .await
            .map_err(|e| PicoError::Channel(format!("Failed to send Telegram message: {}", e)))?;

        info!("Telegram: Message sent successfully to chat {}", chat_id);
        Ok(())
    }

    /// Returns whether the channel is currently running.
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Checks if a user is allowed to use this channel.
    ///
    /// Uses the base configuration's allowlist logic.
    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telegram_channel_creation() {
        let config = TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: vec!["user1".to_string()],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        assert_eq!(channel.name(), "telegram");
        assert!(!channel.is_running());
        assert!(channel.is_allowed("user1"));
        assert!(!channel.is_allowed("user2"));
    }

    #[test]
    fn test_telegram_empty_allowlist() {
        let config = TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: vec![],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        // Empty allowlist should allow anyone
        assert!(channel.is_allowed("anyone"));
        assert!(channel.is_allowed("user1"));
        assert!(channel.is_allowed("random_user_123"));
    }

    #[test]
    fn test_telegram_config_access() {
        let config = TelegramConfig {
            enabled: true,
            token: "my-bot-token".to_string(),
            allow_from: vec!["admin".to_string()],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        assert!(channel.is_enabled());
        assert_eq!(channel.telegram_config().token, "my-bot-token");
        assert_eq!(channel.telegram_config().allow_from, vec!["admin"]);
    }

    #[test]
    fn test_telegram_disabled_channel() {
        let config = TelegramConfig {
            enabled: false,
            token: "test-token".to_string(),
            allow_from: vec![],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        assert!(!channel.is_enabled());
    }

    #[test]
    fn test_telegram_multiple_allowed_users() {
        let config = TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: vec![
                "user1".to_string(),
                "user2".to_string(),
                "admin".to_string(),
            ],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        assert!(channel.is_allowed("user1"));
        assert!(channel.is_allowed("user2"));
        assert!(channel.is_allowed("admin"));
        assert!(!channel.is_allowed("user3"));
        assert!(!channel.is_allowed("hacker"));
    }

    #[tokio::test]
    async fn test_telegram_start_without_token() {
        let config = TelegramConfig {
            enabled: true,
            token: String::new(), // Empty token
            allow_from: vec![],
        };
        let bus = Arc::new(MessageBus::new());
        let mut channel = TelegramChannel::new(config, bus);

        // Should fail with empty token
        let result = channel.start().await;
        assert!(result.is_err());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_telegram_start_disabled() {
        let config = TelegramConfig {
            enabled: false, // Disabled
            token: "test-token".to_string(),
            allow_from: vec![],
        };
        let bus = Arc::new(MessageBus::new());
        let mut channel = TelegramChannel::new(config, bus);

        // Should return Ok but not actually start
        let result = channel.start().await;
        assert!(result.is_ok());
        assert!(!channel.is_running());
    }

    #[tokio::test]
    async fn test_telegram_stop_not_running() {
        let config = TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: vec![],
        };
        let bus = Arc::new(MessageBus::new());
        let mut channel = TelegramChannel::new(config, bus);

        // Should be ok to stop when not running
        let result = channel.stop().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_telegram_send_not_running() {
        let config = TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: vec![],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        // Should fail when not running
        let msg = OutboundMessage::new("telegram", "12345", "Hello");
        let result = channel.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_telegram_base_config() {
        let config = TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: vec!["allowed_user".to_string()],
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus);

        // Verify base config is set correctly
        assert_eq!(channel.base_config.name, "telegram");
        assert_eq!(channel.base_config.allowlist, vec!["allowed_user"]);
    }
}
