//! WhatsApp Web native channel via wa-rs.
//!
//! Pairs via QR code like WhatsApp Desktop and persists session state to a
//! local SQLite database.

use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use wa_rs::bot::Bot;
use wa_rs::proto_helpers::MessageExt;
use wa_rs::store::SqliteStore;
use wa_rs::types::events::Event;
use wa_rs::wa_rs_proto::whatsapp as wa;
use wa_rs::{Client, Jid};
use wa_rs_tokio_transport::TokioWebSocketTransportFactory;
use wa_rs_ureq_http::UreqHttpClient;

use qrcode::QrCode;

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::channels::types::{BaseChannelConfig, Channel};
use crate::config::WhatsAppWebConfig;
use crate::error::{Result, ZeptoError};

/// Render a QR code string as unicode block characters in the terminal.
///
/// Uses 2x1 half-block characters for compact display:
/// - `█` (U+2588) = both pixels dark
/// - `▀` (U+2580) = top dark, bottom light
/// - `▄` (U+2584) = top light, bottom dark
/// - ` ` (space)  = both pixels light
fn render_qr_terminal(data: &str) -> Option<String> {
    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors: Vec<bool> = code
        .into_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();

    // Add 1-module quiet zone on each side
    let padded_width = width + 2;
    let padded_height = width + 2;

    let pixel = |row: usize, col: usize| -> bool {
        if row == 0 || row > width || col == 0 || col > width {
            false // quiet zone
        } else {
            colors[(row - 1) * width + (col - 1)]
        }
    };

    let mut output = String::new();
    let mut y = 0;
    while y < padded_height {
        for x in 0..padded_width {
            let top = pixel(y, x);
            let bottom = if y + 1 < padded_height {
                pixel(y + 1, x)
            } else {
                false
            };
            output.push(match (top, bottom) {
                (true, true) => '\u{2588}',
                (true, false) => '\u{2580}',
                (false, true) => '\u{2584}',
                (false, false) => ' ',
            });
        }
        output.push('\n');
        y += 2;
    }
    Some(output)
}

fn normalize_phone(phone: &str) -> String {
    phone
        .trim()
        .trim_start_matches('+')
        .split('@')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn expand_auth_dir(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

fn sqlite_store_path(path: &str) -> PathBuf {
    let expanded = PathBuf::from(expand_auth_dir(path));
    if expanded.extension().is_some() {
        expanded
    } else {
        expanded.join("session.sqlite3")
    }
}

/// Resolve a sender to a phone number for allowlist matching.
///
/// WhatsApp Web may deliver messages with a LID-based sender JID
/// (e.g. `78971563720736@lid`) instead of a phone number JID
/// (`60123456789@s.whatsapp.net`). The `sender_alt` field on
/// `MessageSource` contains the phone number JID when the primary
/// sender is a LID. This function checks `sender_alt` first,
/// falling back to the primary sender JID.
fn resolve_sender_phone(sender: &Jid, sender_alt: &Option<Jid>) -> String {
    // Prefer sender_alt (contains phone JID when sender is LID)
    if let Some(ref alt) = sender_alt {
        if alt.is_pn() {
            return normalize_phone(&alt.to_string());
        }
    }
    // If sender itself is a phone JID, use it directly
    if sender.is_pn() {
        return normalize_phone(&sender.to_string());
    }
    // Fallback: use sender as-is (LID user part)
    normalize_phone(&sender.to_string())
}

fn parse_chat_jid(chat_id: &str) -> Result<Jid> {
    let jid = if chat_id.contains('@') {
        chat_id.trim().to_string()
    } else {
        format!("{}@s.whatsapp.net", normalize_phone(chat_id))
    };

    Jid::from_str(&jid)
        .map_err(|e| ZeptoError::Channel(format!("WhatsApp Web: invalid recipient '{jid}': {e}")))
}

fn build_outbound_message(msg: &OutboundMessage) -> wa::Message {
    if let Some(reply_to) = msg.reply_to.as_deref() {
        wa::Message {
            extended_text_message: Some(Box::new(wa::message::ExtendedTextMessage {
                text: Some(msg.content.clone()),
                context_info: Some(Box::new(wa::ContextInfo {
                    stanza_id: Some(reply_to.to_string()),
                    ..Default::default()
                })),
                ..Default::default()
            })),
            ..Default::default()
        }
    } else {
        wa::Message {
            conversation: Some(msg.content.clone()),
            ..Default::default()
        }
    }
}

struct RuntimeState {
    client: Arc<Client>,
    task: JoinHandle<()>,
}

/// WhatsApp Web channel using the native wa-rs protocol.
pub struct WhatsAppWebChannel {
    config: WhatsAppWebConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    runtime: Arc<Mutex<Option<RuntimeState>>>,
}

impl WhatsAppWebChannel {
    pub fn new(config: WhatsAppWebConfig, bus: Arc<MessageBus>) -> Self {
        let normalized_allowlist: Vec<String> = config
            .allow_from
            .iter()
            .map(|p| normalize_phone(p))
            .collect();

        let base_config = BaseChannelConfig {
            name: "whatsapp_web".to_string(),
            allowlist: normalized_allowlist,
            deny_by_default: config.deny_by_default,
        };

        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            runtime: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Channel for WhatsAppWebChannel {
    fn name(&self) -> &str {
        &self.base_config.name
    }

    async fn start(&mut self) -> Result<()> {
        {
            let runtime = self.runtime.lock().await;
            if runtime.is_some() && self.running.load(Ordering::SeqCst) {
                info!("WhatsApp Web channel already running");
                return Ok(());
            }
        }

        let store_path = sqlite_store_path(&self.config.auth_dir);
        if let Some(parent) = store_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ZeptoError::Channel(format!(
                    "WhatsApp Web: failed to create auth directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let store = Arc::new(
            SqliteStore::new(store_path.to_str().ok_or_else(|| {
                ZeptoError::Channel(format!(
                    "WhatsApp Web: invalid auth path {}",
                    store_path.display()
                ))
            })?)
            .await
            .map_err(|e| {
                ZeptoError::Channel(format!(
                    "WhatsApp Web: failed to initialize SQLite store {}: {}",
                    store_path.display(),
                    e
                ))
            })?,
        );

        let bus = self.bus.clone();
        let base_config = self.base_config.clone();
        let running = self.running.clone();

        let mut bot = Bot::builder()
            .with_backend(store)
            .with_transport_factory(TokioWebSocketTransportFactory::new())
            .with_http_client(UreqHttpClient::new())
            .on_event(move |event, client| {
                let bus = bus.clone();
                let base_config = base_config.clone();
                let running = running.clone();
                async move {
                    match event {
                        Event::Connected(_) => {
                            running.store(true, Ordering::SeqCst);
                            info!("WhatsApp Web connected");
                        }
                        Event::PairingQrCode { code, timeout } => {
                            eprintln!();
                            eprintln!("╔══════════════════════════════════════════╗");
                            eprintln!("║   Scan this QR code with WhatsApp       ║");
                            eprintln!("║   Phone → Settings → Linked Devices     ║");
                            eprintln!(
                                "║   Valid for {}s                        ║",
                                timeout.as_secs()
                            );
                            eprintln!("╚══════════════════════════════════════════╝");
                            eprintln!();
                            match render_qr_terminal(&code) {
                                Some(qr) => eprint!("{}", qr),
                                None => {
                                    warn!("Failed to render QR code, raw token: {}", code);
                                }
                            }
                            eprintln!();
                        }
                        Event::PairingCode { code, timeout } => {
                            info!(
                                "WhatsApp Web pair code received (valid for {}s): {}",
                                timeout.as_secs(),
                                code
                            );
                        }
                        Event::LoggedOut(reason) => {
                            running.store(false, Ordering::SeqCst);
                            warn!("WhatsApp Web logged out: {:?}", reason.reason);
                        }
                        Event::Disconnected(_) => {
                            running.store(false, Ordering::SeqCst);
                            warn!("WhatsApp Web disconnected");
                        }
                        Event::Message(message, info) => {
                            if info.source.is_from_me {
                                return;
                            }

                            let sender_jid = info.source.sender.to_string();
                            let sender_id =
                                resolve_sender_phone(&info.source.sender, &info.source.sender_alt);
                            if !base_config.is_allowed(&sender_id) {
                                info!(
                                    "WhatsApp Web: sender {} (jid: {}) not in allowlist, ignoring",
                                    sender_id, sender_jid
                                );
                                return;
                            }

                            let content = message
                                .text_content()
                                .or_else(|| message.get_caption())
                                .map(str::trim)
                                .unwrap_or_default()
                                .to_string();

                            if content.is_empty() {
                                return;
                            }

                            let chat_id = info.source.chat.to_string();
                            let mut inbound =
                                InboundMessage::new("whatsapp_web", &sender_id, &chat_id, &content)
                                    .with_metadata("whatsapp_message_id", &info.id)
                                    .with_metadata("sender_jid", &sender_jid)
                                    .with_metadata("chat_jid", &chat_id);

                            if !info.push_name.is_empty() {
                                inbound = inbound.with_metadata("sender_name", &info.push_name);
                            }
                            if info.source.is_group {
                                inbound = inbound.with_metadata("is_group", "true");
                            }

                            if let Err(e) = bus.publish_inbound(inbound).await {
                                error!("WhatsApp Web: failed to publish inbound message: {}", e);
                            }
                        }
                        Event::PairError(err) => {
                            warn!("WhatsApp Web pairing failed: {}", err.error);
                        }
                        _ => {
                            let _ = client;
                        }
                    }
                }
            })
            .build()
            .await
            .map_err(|e| ZeptoError::Channel(format!("WhatsApp Web: bot build failed: {e}")))?;

        let client = bot.client();
        let run_handle = bot
            .run()
            .await
            .map_err(|e| ZeptoError::Channel(format!("WhatsApp Web: bot run failed: {e}")))?;

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_handle.await {
                error!("WhatsApp Web task failed: {}", e);
            }
            running.store(false, Ordering::SeqCst);
        });

        let mut runtime = self.runtime.lock().await;
        *runtime = Some(RuntimeState { client, task });

        info!(
            auth_db = %store_path.display(),
            "WhatsApp Web channel started"
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        let state = self.runtime.lock().await.take();
        let Some(state) = state else {
            self.running.store(false, Ordering::SeqCst);
            return Ok(());
        };

        state.client.disconnect().await;

        match tokio::time::timeout(std::time::Duration::from_secs(10), state.task).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("WhatsApp Web task join failed: {}", e),
            Err(_) => warn!("WhatsApp Web task did not stop within 10 seconds"),
        }

        self.running.store(false, Ordering::SeqCst);
        info!("WhatsApp Web channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if msg.content.trim().is_empty() {
            return Err(ZeptoError::Channel(
                "WhatsApp Web: outbound content cannot be empty".to_string(),
            ));
        }

        let client = {
            let runtime = self.runtime.lock().await;
            runtime
                .as_ref()
                .map(|state| state.client.clone())
                .ok_or_else(|| {
                    ZeptoError::Channel("WhatsApp Web: channel not started".to_string())
                })?
        };

        let jid = parse_chat_jid(&msg.chat_id)?;
        let wa_message = build_outbound_message(&msg);
        client
            .send_message(jid, wa_message)
            .await
            .map_err(|e| ZeptoError::Channel(format!("WhatsApp Web: send failed: {e}")))?;

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(&normalize_phone(user_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;

    fn make_channel(config: WhatsAppWebConfig) -> WhatsAppWebChannel {
        WhatsAppWebChannel::new(config, Arc::new(MessageBus::new()))
    }

    #[test]
    fn test_default_config() {
        let cfg = WhatsAppWebConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.auth_dir, "~/.zeptoclaw/state/whatsapp_web");
        assert!(cfg.allow_from.is_empty());
        assert!(!cfg.deny_by_default);
    }

    #[test]
    fn test_normalize_phone_with_plus() {
        assert_eq!(normalize_phone("+60123456789"), "60123456789");
    }

    #[test]
    fn test_normalize_phone_jid() {
        assert_eq!(normalize_phone("60123456789@s.whatsapp.net"), "60123456789");
    }

    #[test]
    fn test_expand_auth_dir_tilde() {
        let expanded = expand_auth_dir("~/.zeptoclaw/state/whatsapp_web");
        if dirs::home_dir().is_some() {
            assert!(!expanded.starts_with('~'));
        }
    }

    #[test]
    fn test_sqlite_store_path_from_directory() {
        let path = sqlite_store_path("/tmp/wa-state");
        assert_eq!(path, std::path::Path::new("/tmp/wa-state/session.sqlite3"));
    }

    #[test]
    fn test_sqlite_store_path_from_file() {
        let path = sqlite_store_path("/tmp/wa-state.sqlite");
        assert_eq!(path, std::path::Path::new("/tmp/wa-state.sqlite"));
    }

    #[test]
    fn test_is_allowed_normalized_match() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec!["+60123456789".to_string()],
            ..Default::default()
        });
        assert!(ch.is_allowed("60123456789@s.whatsapp.net"));
    }

    #[test]
    fn test_is_allowed_deny_by_default() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec![],
            deny_by_default: true,
            ..Default::default()
        });
        assert!(!ch.is_allowed("60999999999"));
    }

    #[test]
    fn test_parse_chat_jid_from_phone() {
        let jid = parse_chat_jid("+60123456789").unwrap();
        assert_eq!(jid.to_string(), "60123456789@s.whatsapp.net");
    }

    #[test]
    fn test_parse_chat_jid_from_jid() {
        let jid = parse_chat_jid("60123456789@s.whatsapp.net").unwrap();
        assert_eq!(jid.to_string(), "60123456789@s.whatsapp.net");
    }

    #[test]
    fn test_build_outbound_message_plain() {
        let message = build_outbound_message(&OutboundMessage::new(
            "whatsapp_web",
            "60123456789",
            "Hello",
        ));
        assert_eq!(message.conversation.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_build_outbound_message_reply() {
        let message = build_outbound_message(
            &OutboundMessage::new("whatsapp_web", "60123456789", "Hello").with_reply("abc123"),
        );
        let reply = message.extended_text_message.expect("reply message");
        assert_eq!(reply.text.as_deref(), Some("Hello"));
        assert_eq!(
            reply.context_info.and_then(|ctx| ctx.stanza_id).as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn test_channel_name() {
        let ch = make_channel(WhatsAppWebConfig::default());
        assert_eq!(ch.name(), "whatsapp_web");
    }

    #[test]
    fn test_channel_not_running_initially() {
        let ch = make_channel(WhatsAppWebConfig::default());
        assert!(!ch.is_running());
    }

    #[tokio::test]
    async fn test_send_errors_when_not_started() {
        let ch = make_channel(WhatsAppWebConfig::default());
        let msg = OutboundMessage::new("whatsapp_web", "60123456789", "Hello");
        let result = ch.send(msg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not started"));
    }

    #[test]
    fn test_resolve_sender_phone_from_pn_jid() {
        let sender = Jid::from_str("60123456789@s.whatsapp.net").unwrap();
        let result = resolve_sender_phone(&sender, &None);
        assert_eq!(result, "60123456789");
    }

    #[test]
    fn test_resolve_sender_phone_from_lid_with_alt() {
        let sender = Jid::from_str("78971563720736@lid").unwrap();
        let alt = Some(Jid::from_str("60123456789@s.whatsapp.net").unwrap());
        let result = resolve_sender_phone(&sender, &alt);
        assert_eq!(result, "60123456789");
    }

    #[test]
    fn test_resolve_sender_phone_from_lid_without_alt() {
        let sender = Jid::from_str("78971563720736@lid").unwrap();
        let result = resolve_sender_phone(&sender, &None);
        assert_eq!(result, "78971563720736");
    }

    #[test]
    fn test_resolve_sender_phone_lid_alt_is_also_lid() {
        let sender = Jid::from_str("111111@lid").unwrap();
        let alt = Some(Jid::from_str("222222@lid").unwrap());
        let result = resolve_sender_phone(&sender, &alt);
        // Neither is a PN, falls back to sender user part
        assert_eq!(result, "111111");
    }
}
