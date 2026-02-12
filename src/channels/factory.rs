//! Channel factory/registration helpers.

use std::sync::Arc;

use tracing::{info, warn};

use crate::bus::MessageBus;
use crate::config::Config;

use super::{ChannelManager, TelegramChannel};

/// Register all configured channels that currently have implementations.
///
/// Returns the number of registered channels.
pub async fn register_configured_channels(
    manager: &ChannelManager,
    bus: Arc<MessageBus>,
    config: &Config,
) -> usize {
    // Telegram
    if let Some(ref telegram_config) = config.channels.telegram {
        if telegram_config.enabled {
            if telegram_config.token.is_empty() {
                warn!("Telegram channel enabled but token is empty");
            } else {
                manager
                    .register(Box::new(TelegramChannel::new(
                        telegram_config.clone(),
                        bus.clone(),
                    )))
                    .await;
                info!("Registered Telegram channel");
            }
        }
    }

    // Enabled in config but not implemented in runtime wiring yet.
    if config
        .channels
        .discord
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("Discord channel is enabled but not implemented");
    }
    if config
        .channels
        .slack
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("Slack channel is enabled but not implemented");
    }
    if config
        .channels
        .whatsapp
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("WhatsApp channel is enabled but not implemented");
    }
    if config
        .channels
        .feishu
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("Feishu channel is enabled but not implemented");
    }
    if config
        .channels
        .maixcam
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("MaixCam channel is enabled but not implemented");
    }
    if config
        .channels
        .qq
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("QQ channel is enabled but not implemented");
    }
    if config
        .channels
        .dingtalk
        .as_ref()
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        warn!("DingTalk channel is enabled but not implemented");
    }

    manager.channel_count().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;
    use crate::config::{Config, TelegramConfig};

    #[tokio::test]
    async fn test_register_configured_channels_registers_telegram() {
        let bus = Arc::new(MessageBus::new());
        let mut config = Config::default();
        config.channels.telegram = Some(TelegramConfig {
            enabled: true,
            token: "test-token".to_string(),
            allow_from: Vec::new(),
        });

        let manager = ChannelManager::new(bus.clone(), config.clone());
        let count = register_configured_channels(&manager, bus, &config).await;

        assert_eq!(count, 1);
        assert!(manager.has_channel("telegram").await);
    }
}
