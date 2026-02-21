//! Message types for the ZeptoClaw message bus
//!
//! This module defines the core message types used for communication
//! between channels, agents, and the message bus.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents an incoming message from a channel (e.g., Telegram, Discord, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    /// The channel this message came from (e.g., "telegram", "discord")
    pub channel: String,
    /// Unique identifier of the sender
    pub sender_id: String,
    /// Unique identifier of the chat/conversation
    pub chat_id: String,
    /// The text content of the message
    pub content: String,
    /// Optional media attachment
    pub media: Option<MediaAttachment>,
    /// Session key for routing (format: "channel:chat_id")
    pub session_key: String,
    /// Additional metadata key-value pairs
    pub metadata: HashMap<String, String>,
}

/// Represents an outgoing message to be sent via a channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    /// The channel to send this message through
    pub channel: String,
    /// The chat/conversation to send to
    pub chat_id: String,
    /// The text content to send
    pub content: String,
    /// Optional message ID to reply to
    pub reply_to: Option<String>,
    /// Additional metadata key-value pairs for channel-specific delivery hints
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Represents a media attachment (image, audio, video, or document)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    /// The type of media
    pub media_type: MediaType,
    /// URL to the media (if hosted remotely)
    pub url: Option<String>,
    /// Raw binary data (if available locally)
    pub data: Option<Vec<u8>>,
    /// Original filename
    pub filename: Option<String>,
}

/// Types of media that can be attached to messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaType {
    /// Image files (PNG, JPG, GIF, etc.)
    Image,
    /// Audio files (MP3, WAV, OGG, etc.)
    Audio,
    /// Video files (MP4, WebM, etc.)
    Video,
    /// Document files (PDF, DOCX, etc.)
    Document,
}

impl InboundMessage {
    /// Creates a new inbound message with the required fields.
    ///
    /// The session key is automatically generated as "channel:chat_id".
    ///
    /// # Arguments
    /// * `channel` - The source channel (e.g., "telegram")
    /// * `sender_id` - Unique identifier of the message sender
    /// * `chat_id` - Unique identifier of the chat/conversation
    /// * `content` - The text content of the message
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::message::InboundMessage;
    ///
    /// let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello, bot!");
    /// assert_eq!(msg.session_key, "telegram:chat456");
    /// ```
    pub fn new(channel: &str, sender_id: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            sender_id: sender_id.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            media: None,
            session_key: format!("{}:{}", channel, chat_id),
            metadata: HashMap::new(),
        }
    }

    /// Attaches media to the message (builder pattern).
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::message::{InboundMessage, MediaAttachment, MediaType};
    ///
    /// let media = MediaAttachment::new(MediaType::Image).with_url("https://example.com/image.png");
    /// let msg = InboundMessage::new("telegram", "user123", "chat456", "Check this out!")
    ///     .with_media(media);
    /// assert!(msg.media.is_some());
    /// ```
    pub fn with_media(mut self, media: MediaAttachment) -> Self {
        self.media = Some(media);
        self
    }

    /// Adds a metadata key-value pair to the message (builder pattern).
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::message::InboundMessage;
    ///
    /// let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello")
    ///     .with_metadata("message_id", "12345")
    ///     .with_metadata("is_bot", "false");
    /// assert_eq!(msg.metadata.get("message_id"), Some(&"12345".to_string()));
    /// ```
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }

    /// Checks if this message has any media attached.
    pub fn has_media(&self) -> bool {
        self.media.is_some()
    }
}

impl OutboundMessage {
    /// Creates a new outbound message.
    ///
    /// # Arguments
    /// * `channel` - The target channel (e.g., "telegram")
    /// * `chat_id` - The chat/conversation to send to
    /// * `content` - The text content to send
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::message::OutboundMessage;
    ///
    /// let msg = OutboundMessage::new("telegram", "chat456", "Hello from the bot!");
    /// assert_eq!(msg.channel, "telegram");
    /// ```
    pub fn new(channel: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            reply_to: None,
            metadata: HashMap::new(),
        }
    }

    /// Sets the message ID to reply to (builder pattern).
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::message::OutboundMessage;
    ///
    /// let msg = OutboundMessage::new("telegram", "chat456", "This is a reply")
    ///     .with_reply("original_msg_123");
    /// assert_eq!(msg.reply_to, Some("original_msg_123".to_string()));
    /// ```
    pub fn with_reply(mut self, message_id: &str) -> Self {
        self.reply_to = Some(message_id.to_string());
        self
    }

    /// Adds a metadata key-value pair to the outbound message.
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }

    /// Creates an outbound message as a response to an inbound message.
    ///
    /// # Example
    /// ```
    /// use zeptoclaw::bus::message::{InboundMessage, OutboundMessage};
    ///
    /// let inbound = InboundMessage::new("telegram", "user123", "chat456", "Hello");
    /// let response = OutboundMessage::reply_to(&inbound, "Hello back!");
    /// assert_eq!(response.channel, "telegram");
    /// assert_eq!(response.chat_id, "chat456");
    /// ```
    pub fn reply_to(msg: &InboundMessage, content: &str) -> Self {
        Self::new(&msg.channel, &msg.chat_id, content)
    }
}

impl MediaAttachment {
    /// Creates a new media attachment of the specified type.
    pub fn new(media_type: MediaType) -> Self {
        Self {
            media_type,
            url: None,
            data: None,
            filename: None,
        }
    }

    /// Sets the URL for the media (builder pattern).
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = Some(url.to_string());
        self
    }

    /// Sets the raw binary data (builder pattern).
    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.data = Some(data);
        self
    }

    /// Sets the filename (builder pattern).
    pub fn with_filename(mut self, filename: &str) -> Self {
        self.filename = Some(filename.to_string());
        self
    }

    /// Checks if the media has a URL.
    pub fn has_url(&self) -> bool {
        self.url.is_some()
    }

    /// Checks if the media has binary data.
    pub fn has_data(&self) -> bool {
        self.data.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inbound_message_creation() {
        let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello");
        assert_eq!(msg.channel, "telegram");
        assert_eq!(msg.sender_id, "user123");
        assert_eq!(msg.chat_id, "chat456");
        assert_eq!(msg.content, "Hello");
        assert_eq!(msg.session_key, "telegram:chat456");
        assert!(msg.media.is_none());
        assert!(msg.metadata.is_empty());
    }

    #[test]
    fn test_inbound_message_with_media() {
        let media = MediaAttachment::new(MediaType::Image)
            .with_url("https://example.com/image.png")
            .with_filename("image.png");

        let msg =
            InboundMessage::new("discord", "user1", "channel1", "Check this").with_media(media);

        assert!(msg.has_media());
        let attachment = msg.media.unwrap();
        assert_eq!(attachment.media_type, MediaType::Image);
        assert_eq!(
            attachment.url,
            Some("https://example.com/image.png".to_string())
        );
        assert_eq!(attachment.filename, Some("image.png".to_string()));
    }

    #[test]
    fn test_inbound_message_with_metadata() {
        let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello")
            .with_metadata("message_id", "12345")
            .with_metadata("timestamp", "2024-01-01T00:00:00Z");

        assert_eq!(msg.metadata.len(), 2);
        assert_eq!(msg.metadata.get("message_id"), Some(&"12345".to_string()));
        assert_eq!(
            msg.metadata.get("timestamp"),
            Some(&"2024-01-01T00:00:00Z".to_string())
        );
    }

    #[test]
    fn test_outbound_message_creation() {
        let msg = OutboundMessage::new("telegram", "chat456", "Response");
        assert_eq!(msg.channel, "telegram");
        assert_eq!(msg.chat_id, "chat456");
        assert_eq!(msg.content, "Response");
        assert!(msg.reply_to.is_none());
        assert!(msg.metadata.is_empty());
    }

    #[test]
    fn test_outbound_message_with_reply() {
        let msg = OutboundMessage::new("telegram", "chat456", "This is a reply")
            .with_reply("original_msg_123");

        assert_eq!(msg.reply_to, Some("original_msg_123".to_string()));
    }

    #[test]
    fn test_outbound_message_with_metadata() {
        let msg = OutboundMessage::new("discord", "channel1", "Hello")
            .with_metadata("discord_thread_name", "Daily Updates")
            .with_metadata("discord_thread_auto_archive_minutes", "60");

        assert_eq!(
            msg.metadata.get("discord_thread_name"),
            Some(&"Daily Updates".to_string())
        );
        assert_eq!(
            msg.metadata.get("discord_thread_auto_archive_minutes"),
            Some(&"60".to_string())
        );
    }

    #[test]
    fn test_outbound_reply_to_inbound() {
        let inbound = InboundMessage::new("telegram", "user123", "chat456", "Hello");
        let response = OutboundMessage::reply_to(&inbound, "Hello back!");

        assert_eq!(response.channel, "telegram");
        assert_eq!(response.chat_id, "chat456");
        assert_eq!(response.content, "Hello back!");
    }

    #[test]
    fn test_media_attachment_creation() {
        let media = MediaAttachment::new(MediaType::Audio)
            .with_url("https://example.com/audio.mp3")
            .with_data(vec![1, 2, 3, 4])
            .with_filename("audio.mp3");

        assert_eq!(media.media_type, MediaType::Audio);
        assert!(media.has_url());
        assert!(media.has_data());
        assert_eq!(media.filename, Some("audio.mp3".to_string()));
    }

    #[test]
    fn test_media_type_equality() {
        assert_eq!(MediaType::Image, MediaType::Image);
        assert_ne!(MediaType::Image, MediaType::Audio);
        assert_ne!(MediaType::Video, MediaType::Document);
    }

    #[test]
    fn test_message_serialization() {
        let msg = InboundMessage::new("telegram", "user123", "chat456", "Hello")
            .with_metadata("key", "value");

        let json = serde_json::to_string(&msg).expect("Failed to serialize");
        let deserialized: InboundMessage =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.channel, "telegram");
        assert_eq!(deserialized.content, "Hello");
        assert_eq!(deserialized.metadata.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_outbound_message_serialization() {
        let msg = OutboundMessage::new("discord", "channel1", "Hello Discord!")
            .with_reply("msg_123")
            .with_metadata("discord_thread_name", "ops-thread");

        let json = serde_json::to_string(&msg).expect("Failed to serialize");
        let deserialized: OutboundMessage =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.channel, "discord");
        assert_eq!(deserialized.reply_to, Some("msg_123".to_string()));
        assert_eq!(
            deserialized.metadata.get("discord_thread_name"),
            Some(&"ops-thread".to_string())
        );
    }
}
