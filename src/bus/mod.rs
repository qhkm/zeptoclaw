//! Message Bus Module
//!
//! This module provides the core message bus infrastructure for ZeptoClaw.
//! The `MessageBus` handles routing of inbound messages (from channels to agents)
//! and outbound messages (from agents back to channels).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │   Channel   │────>│  MessageBus │────>│    Agent    │
//! │  (Telegram) │     │  (inbound)  │     │  (OpenAI)   │
//! └─────────────┘     └─────────────┘     └─────────────┘
//!                            │
//!                            │ outbound
//!                            ▼
//! ┌─────────────┐     ┌─────────────┐
//! │   Channel   │<────│  MessageBus │
//! │  (Telegram) │     │  (outbound) │
//! └─────────────┘     └─────────────┘
//! ```
//!
//! # Example
//!
//! ```
//! use zeptoclaw::bus::{MessageBus, InboundMessage, OutboundMessage};
//!
//! #[tokio::main]
//! async fn main() {
//!     let bus = MessageBus::new();
//!
//!     // Publish an inbound message
//!     let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");
//!     bus.publish_inbound(msg).await.unwrap();
//!
//!     // Consume it elsewhere
//!     if let Some(received) = bus.consume_inbound().await {
//!         println!("Received: {}", received.content);
//!     }
//! }
//! ```

pub mod message;

pub use message::{InboundMessage, MediaAttachment, MediaType, OutboundMessage};

use crate::error::{Result, ZeptoError};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

/// Default buffer size for message channels
const DEFAULT_BUFFER_SIZE: usize = 100;

/// The central message bus for routing messages between channels and agents.
///
/// The `MessageBus` maintains two separate channels:
/// - **Inbound**: Messages from channels (e.g., Telegram) to agents
/// - **Outbound**: Messages from agents back to channels
///
/// Both channels use async MPSC (multi-producer, single-consumer) queues
/// backed by Tokio, allowing for high-throughput message passing.
pub struct MessageBus {
    /// Sender for inbound messages
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Receiver for inbound messages (wrapped in Arc<Mutex> for shared access)
    inbound_rx: Arc<Mutex<mpsc::Receiver<InboundMessage>>>,
    /// Sender for outbound messages
    outbound_tx: mpsc::Sender<OutboundMessage>,
    /// Receiver for outbound messages (wrapped in Arc<Mutex> for shared access)
    outbound_rx: Arc<Mutex<mpsc::Receiver<OutboundMessage>>>,
}

impl MessageBus {
    /// Creates a new `MessageBus` with default buffer sizes.
    ///
    /// The default buffer size is 100 messages for both inbound and outbound channels.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::MessageBus;
    ///
    /// let bus = MessageBus::new();
    /// ```
    pub fn new() -> Self {
        Self::with_buffer_size(DEFAULT_BUFFER_SIZE)
    }

    /// Creates a new `MessageBus` with a custom buffer size.
    ///
    /// # Arguments
    /// * `buffer_size` - The maximum number of messages that can be buffered
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::MessageBus;
    ///
    /// let bus = MessageBus::with_buffer_size(500);
    /// ```
    pub fn with_buffer_size(buffer_size: usize) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(buffer_size);
        let (outbound_tx, outbound_rx) = mpsc::channel(buffer_size);

        Self {
            inbound_tx,
            inbound_rx: Arc::new(Mutex::new(inbound_rx)),
            outbound_tx,
            outbound_rx: Arc::new(Mutex::new(outbound_rx)),
        }
    }

    /// Publishes an inbound message to the bus.
    ///
    /// This is typically called by channel adapters (e.g., Telegram, Discord)
    /// when they receive a message from a user.
    ///
    /// # Arguments
    /// * `msg` - The inbound message to publish
    ///
    /// # Errors
    /// Returns `ZeptoError::BusClosed` if the receiver has been dropped.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::{MessageBus, InboundMessage};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let bus = MessageBus::new();
    ///     let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");
    ///     bus.publish_inbound(msg).await.unwrap();
    /// }
    /// ```
    pub async fn publish_inbound(&self, msg: InboundMessage) -> Result<()> {
        self.inbound_tx
            .send(msg)
            .await
            .map_err(|_| ZeptoError::BusClosed)
    }

    /// Consumes the next inbound message from the bus.
    ///
    /// This is typically called by agents waiting for new messages to process.
    ///
    /// # Returns
    /// - `Some(InboundMessage)` if a message is available
    /// - `None` if the channel is closed (all senders dropped)
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::{MessageBus, InboundMessage};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let bus = MessageBus::new();
    ///
    ///     // In a real scenario, messages would be published by channels
    ///     let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");
    ///     bus.publish_inbound(msg).await.unwrap();
    ///
    ///     // Agent consumes the message
    ///     if let Some(received) = bus.consume_inbound().await {
    ///         println!("Processing: {}", received.content);
    ///     }
    /// }
    /// ```
    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        self.inbound_rx.lock().await.recv().await
    }

    /// Publishes an outbound message to the bus.
    ///
    /// This is typically called by agents when they have a response
    /// to send back to a user via a channel.
    ///
    /// # Arguments
    /// * `msg` - The outbound message to publish
    ///
    /// # Errors
    /// Returns `ZeptoError::BusClosed` if the receiver has been dropped.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::{MessageBus, OutboundMessage};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let bus = MessageBus::new();
    ///     let msg = OutboundMessage::new("telegram", "chat456", "Hello back!");
    ///     bus.publish_outbound(msg).await.unwrap();
    /// }
    /// ```
    pub async fn publish_outbound(&self, msg: OutboundMessage) -> Result<()> {
        self.outbound_tx
            .send(msg)
            .await
            .map_err(|_| ZeptoError::BusClosed)
    }

    /// Consumes the next outbound message from the bus.
    ///
    /// This is typically called by channel adapters waiting for
    /// responses to send to users.
    ///
    /// # Returns
    /// - `Some(OutboundMessage)` if a message is available
    /// - `None` if the channel is closed (all senders dropped)
    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        self.outbound_rx.lock().await.recv().await
    }

    /// Returns a clone of the inbound message sender.
    ///
    /// This is useful for giving multiple channels their own sender
    /// to publish messages to the bus.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::{MessageBus, InboundMessage};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let bus = MessageBus::new();
    ///     let sender = bus.inbound_sender();
    ///
    ///     // The sender can be moved to another task
    ///     let handle = tokio::spawn(async move {
    ///         let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");
    ///         sender.send(msg).await.unwrap();
    ///     });
    ///     handle.await.unwrap();
    /// }
    /// ```
    pub fn inbound_sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Returns a clone of the outbound message sender.
    ///
    /// This is useful for giving multiple agents their own sender
    /// to publish responses to the bus.
    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Tries to publish an inbound message without blocking.
    ///
    /// This is useful in non-async contexts or when you want to
    /// avoid blocking if the buffer is full.
    ///
    /// # Returns
    /// - `Ok(())` if the message was successfully queued
    /// - `Err(ZeptoError::BusClosed)` if the channel is closed
    /// - `Err(ZeptoError::Channel)` if the buffer is full
    pub fn try_publish_inbound(&self, msg: InboundMessage) -> Result<()> {
        self.inbound_tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                ZeptoError::Channel("inbound buffer full".to_string())
            }
            mpsc::error::TrySendError::Closed(_) => ZeptoError::BusClosed,
        })
    }

    /// Tries to publish an outbound message without blocking.
    pub fn try_publish_outbound(&self, msg: OutboundMessage) -> Result<()> {
        self.outbound_tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                ZeptoError::Channel("outbound buffer full".to_string())
            }
            mpsc::error::TrySendError::Closed(_) => ZeptoError::BusClosed,
        })
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MessageBus {
    /// Clones the message bus, sharing the same underlying channels.
    ///
    /// This allows multiple components to share access to the same bus.
    fn clone(&self) -> Self {
        Self {
            inbound_tx: self.inbound_tx.clone(),
            inbound_rx: Arc::clone(&self.inbound_rx),
            outbound_tx: self.outbound_tx.clone(),
            outbound_rx: Arc::clone(&self.outbound_rx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inbound_message_creation() {
        let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");
        assert_eq!(msg.channel, "telegram");
        assert_eq!(msg.content, "Hello");
        assert_eq!(msg.session_key, "telegram:chat456");
    }

    #[test]
    fn test_message_bus_creation() {
        let bus = MessageBus::new();
        // Just verify it can be created without panicking
        drop(bus);
    }

    #[test]
    fn test_message_bus_with_custom_buffer() {
        let bus = MessageBus::with_buffer_size(50);
        drop(bus);
    }

    #[test]
    fn test_message_bus_default() {
        let bus = MessageBus::default();
        drop(bus);
    }

    #[test]
    fn test_message_bus_clone() {
        let bus1 = MessageBus::new();
        let bus2 = bus1.clone();
        // Both should share the same underlying channels
        drop(bus1);
        drop(bus2);
    }

    #[tokio::test]
    async fn test_bus_inbound_flow() {
        let bus = MessageBus::new();
        let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");

        bus.publish_inbound(msg.clone()).await.unwrap();
        let received = bus.consume_inbound().await.unwrap();

        assert_eq!(received.content, "Hello");
        assert_eq!(received.channel, "telegram");
        assert_eq!(received.sender_id, "user123");
        assert_eq!(received.chat_id, "chat456");
    }

    #[tokio::test]
    async fn test_bus_outbound_flow() {
        let bus = MessageBus::new();
        let msg = OutboundMessage::new("telegram", "chat456", "Response");

        bus.publish_outbound(msg).await.unwrap();
        let received = bus.consume_outbound().await.unwrap();

        assert_eq!(received.content, "Response");
        assert_eq!(received.channel, "telegram");
        assert_eq!(received.chat_id, "chat456");
    }

    #[tokio::test]
    async fn test_bus_multiple_messages() {
        let bus = MessageBus::new();

        // Publish multiple messages
        for i in 0..5 {
            let msg = InboundMessage::new("telegram", "user", "chat", &format!("Message {}", i));
            bus.publish_inbound(msg).await.unwrap();
        }

        // Consume all messages in order
        for i in 0..5 {
            let received = bus.consume_inbound().await.unwrap();
            assert_eq!(received.content, format!("Message {}", i));
        }
    }

    #[tokio::test]
    async fn test_bus_sender_clones() {
        let bus = MessageBus::new();
        let sender1 = bus.inbound_sender();
        let sender2 = bus.inbound_sender();

        // Both senders should work
        let msg1 = InboundMessage::new("telegram", "user1", "chat1", "From sender 1");
        let msg2 = InboundMessage::new("discord", "user2", "chat2", "From sender 2");

        sender1.send(msg1).await.unwrap();
        sender2.send(msg2).await.unwrap();

        let received1 = bus.consume_inbound().await.unwrap();
        let received2 = bus.consume_inbound().await.unwrap();

        assert_eq!(received1.content, "From sender 1");
        assert_eq!(received2.content, "From sender 2");
    }

    #[tokio::test]
    async fn test_bus_concurrent_access() {
        let bus = Arc::new(MessageBus::new());
        let bus_clone = Arc::clone(&bus);

        // Spawn a producer task
        let producer = tokio::spawn(async move {
            for i in 0..10 {
                let msg = InboundMessage::new("test", "user", "chat", &format!("Msg {}", i));
                bus_clone.publish_inbound(msg).await.unwrap();
            }
        });

        // Spawn a consumer task
        let bus_clone2 = Arc::clone(&bus);
        let consumer = tokio::spawn(async move {
            let mut count = 0;
            while count < 10 {
                if let Some(_msg) = bus_clone2.consume_inbound().await {
                    count += 1;
                }
            }
            count
        });

        producer.await.unwrap();
        let consumed = consumer.await.unwrap();
        assert_eq!(consumed, 10);
    }

    #[tokio::test]
    async fn test_try_publish_inbound() {
        let bus = MessageBus::with_buffer_size(2);

        // Fill the buffer
        let msg1 = InboundMessage::new("test", "user", "chat", "Msg 1");
        let msg2 = InboundMessage::new("test", "user", "chat", "Msg 2");
        bus.try_publish_inbound(msg1).unwrap();
        bus.try_publish_inbound(msg2).unwrap();

        // Third message should fail with buffer full
        let msg3 = InboundMessage::new("test", "user", "chat", "Msg 3");
        let result = bus.try_publish_inbound(msg3);
        assert!(matches!(result, Err(ZeptoError::Channel(_))));
    }

    #[tokio::test]
    async fn test_try_publish_outbound() {
        let bus = MessageBus::with_buffer_size(2);

        let msg1 = OutboundMessage::new("test", "chat", "Msg 1");
        let msg2 = OutboundMessage::new("test", "chat", "Msg 2");
        bus.try_publish_outbound(msg1).unwrap();
        bus.try_publish_outbound(msg2).unwrap();

        // Third message should fail
        let msg3 = OutboundMessage::new("test", "chat", "Msg 3");
        let result = bus.try_publish_outbound(msg3);
        assert!(matches!(result, Err(ZeptoError::Channel(_))));
    }

    #[tokio::test]
    async fn test_outbound_with_reply() {
        let bus = MessageBus::new();
        let msg = OutboundMessage::new("telegram", "chat456", "This is a reply")
            .with_reply("original_msg_123");

        bus.publish_outbound(msg).await.unwrap();
        let received = bus.consume_outbound().await.unwrap();

        assert_eq!(received.reply_to, Some("original_msg_123".to_string()));
    }

    #[tokio::test]
    async fn test_inbound_with_media() {
        let bus = MessageBus::new();

        let media = MediaAttachment::new(MediaType::Image)
            .with_url("https://example.com/image.png")
            .with_filename("photo.png");

        let msg = InboundMessage::new("telegram", "user123", "chat456", "Check this out!")
            .with_media(media);

        bus.publish_inbound(msg).await.unwrap();
        let received = bus.consume_inbound().await.unwrap();

        assert!(received.has_media());
        let attachment = received.media.first().unwrap();
        assert_eq!(attachment.media_type, MediaType::Image);
        assert!(attachment.has_url());
    }

    #[tokio::test]
    async fn test_bus_reply_to_inbound() {
        let bus = MessageBus::new();

        // Simulate receiving an inbound message
        let inbound = InboundMessage::new("telegram", "user123", "chat456", "Hello bot!");
        bus.publish_inbound(inbound).await.unwrap();

        // Agent receives and processes
        let received = bus.consume_inbound().await.unwrap();

        // Agent creates a response
        let response = OutboundMessage::reply_to(&received, "Hello human!");
        bus.publish_outbound(response).await.unwrap();

        // Channel receives the response
        let outgoing = bus.consume_outbound().await.unwrap();
        assert_eq!(outgoing.channel, "telegram");
        assert_eq!(outgoing.chat_id, "chat456");
        assert_eq!(outgoing.content, "Hello human!");
    }
}
