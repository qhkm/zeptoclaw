//! WhatsApp Web native channel (wa-rs, feature: whatsapp-web).
//!
//! Pairs via QR code like WhatsApp Desktop — no Meta Business account required.
//! This module is a placeholder; the full implementation is in Task 10.

use crate::bus::MessageBus;
use crate::config::WhatsAppWebConfig;
use crate::error::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::types::{BaseChannelConfig, Channel};
use crate::bus::OutboundMessage;

/// WhatsApp Web channel using native wa-rs protocol.
pub struct WhatsAppWebChannel {
    config: WhatsAppWebConfig,
    base: BaseChannelConfig,
    _bus: Arc<MessageBus>,
    running: bool,
}

impl WhatsAppWebChannel {
    /// Create a new WhatsApp Web channel.
    pub fn new(config: WhatsAppWebConfig, bus: Arc<MessageBus>) -> Self {
        let base = BaseChannelConfig {
            name: "whatsapp_web".to_string(),
            allowlist: config.allow_from.clone(),
            deny_by_default: config.deny_by_default,
        };
        Self {
            config,
            base,
            _bus: bus,
            running: false,
        }
    }
}

#[async_trait]
impl Channel for WhatsAppWebChannel {
    fn name(&self) -> &str {
        &self.base.name
    }

    async fn start(&mut self) -> Result<()> {
        self.running = true;
        tracing::info!(
            auth_dir = %self.config.auth_dir,
            "WhatsApp Web channel started (stub — full implementation pending)"
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.running = false;
        Ok(())
    }

    async fn send(&self, _msg: OutboundMessage) -> Result<()> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base.is_allowed(user_id)
    }
}
