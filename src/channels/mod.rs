//! Channels module - Communication channels (Telegram, Discord, etc.)
//!
//! This module provides the infrastructure for managing communication channels
//! in ZeptoClaw. Channels are responsible for receiving messages from users
//! and sending responses back.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     ChannelManager                          │
//! │                                                             │
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐       │
//! │  │Telegram │  │ Discord │  │  Slack  │  │WhatsApp │  ...  │
//! │  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘       │
//! │       │            │            │            │              │
//! │       │            │ implements │            │              │
//! │       │            │  Channel   │            │              │
//! │       │            │   trait    │            │              │
//! │       └────────────┴─────┬──────┴────────────┘              │
//! │                          │                                  │
//! │                    ┌─────┴─────┐                           │
//! │                    │MessageBus │                           │
//! │                    │ (inbound/ │                           │
//! │                    │ outbound) │                           │
//! │                    └───────────┘                           │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Implementing a New Channel
//!
//! To implement a new channel, create a struct that implements the `Channel` trait:
//!
//! ```ignore
//! use async_trait::async_trait;
//! use zeptoclaw::channels::{Channel, BaseChannelConfig};
//! use zeptoclaw::bus::{MessageBus, OutboundMessage};
//! use zeptoclaw::error::Result;
//! use std::sync::Arc;
//!
//! pub struct MyChannel {
//!     config: BaseChannelConfig,
//!     running: bool,
//!     bus: Arc<MessageBus>,
//! }
//!
//! impl MyChannel {
//!     pub fn new(name: &str, bus: Arc<MessageBus>) -> Self {
//!         Self {
//!             config: BaseChannelConfig::new(name),
//!             running: false,
//!             bus,
//!         }
//!     }
//! }
//!
//! #[async_trait]
//! impl Channel for MyChannel {
//!     fn name(&self) -> &str {
//!         &self.config.name
//!     }
//!
//!     async fn start(&mut self) -> Result<()> {
//!         self.running = true;
//!         // Start listening for messages...
//!         Ok(())
//!     }
//!
//!     async fn stop(&mut self) -> Result<()> {
//!         self.running = false;
//!         Ok(())
//!     }
//!
//!     async fn send(&self, msg: OutboundMessage) -> Result<()> {
//!         // Send message via your channel's API...
//!         Ok(())
//!     }
//!
//!     fn is_running(&self) -> bool {
//!         self.running
//!     }
//!
//!     fn is_allowed(&self, user_id: &str) -> bool {
//!         self.config.is_allowed(user_id)
//!     }
//! }
//! ```
//!
//! # Usage
//!
//! ```
//! use std::sync::Arc;
//! use zeptoclaw::bus::MessageBus;
//! use zeptoclaw::config::Config;
//! use zeptoclaw::channels::ChannelManager;
//!
//! # tokio_test::block_on(async {
//! let bus = Arc::new(MessageBus::new());
//! let config = Config::default();
//! let manager = ChannelManager::new(bus, config);
//!
//! // Register channels
//! // manager.register(Box::new(telegram_channel)).await;
//! // manager.register(Box::new(discord_channel)).await;
//!
//! // Start all channels
//! // manager.start_all().await?;
//! # })
//! ```

pub mod discord;
mod factory;
mod manager;
pub mod slack;
pub mod telegram;
mod types;
pub mod webhook;
pub mod whatsapp;
pub mod whatsapp_cloud;

pub use discord::DiscordChannel;
pub use factory::register_configured_channels;
pub use manager::ChannelManager;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use types::{BaseChannelConfig, Channel};
pub use webhook::{WebhookChannel, WebhookChannelConfig};
pub use whatsapp::WhatsAppChannel;
pub use whatsapp_cloud::WhatsAppCloudChannel;
