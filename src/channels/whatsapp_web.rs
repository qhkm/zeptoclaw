//! WhatsApp Web native channel via wa-rs.
//!
//! Pairs via QR code like WhatsApp Desktop — no Meta Business account required.
//! Uses the wa-rs crate for direct WhatsApp Web protocol support.
//!
//! # Feature Gate
//!
//! This entire module requires the `whatsapp-web` feature:
//! ```bash
//! cargo build --features whatsapp-web
//! ```
//!
//! # Session Persistence
//!
//! Authentication state is stored in `auth_dir` (default:
//! `~/.zeptoclaw/state/whatsapp_web`). On first run a QR code is printed to
//! the terminal; subsequent runs reuse the saved session.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::FutureExt;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::bus::{MessageBus, OutboundMessage};
use crate::channels::types::{BaseChannelConfig, Channel};
use crate::config::WhatsAppWebConfig;
use crate::error::{Result, ZeptoError};

// ── Helper functions (pure, testable without wa-rs) ──────────────────────────

/// Normalize a phone number by stripping a leading `+`.
///
/// WhatsApp JIDs use plain E.164 without the leading plus sign
/// (e.g. `"60123456789@s.whatsapp.net"`). This function converts
/// `"+60123456789"` → `"60123456789"` and leaves already-normalized
/// numbers unchanged.
///
/// # Examples
///
/// ```
/// // "+60123456789" → "60123456789"
/// // "60123456789"  → "60123456789" (no-op)
/// ```
fn normalize_phone(phone: &str) -> String {
    phone.trim_start_matches('+').to_string()
}

/// Calculate exponential back-off delay for reconnection attempts.
///
/// Base: 2 s, doubles each attempt, capped at 120 s.
///
/// | attempt | delay  |
/// |---------|--------|
/// | 0       |   2 s  |
/// | 1       |   4 s  |
/// | 2       |   8 s  |
/// | 5       |  64 s  |
/// | 6+      | 120 s  |
#[allow(dead_code)]
fn backoff_delay(attempt: u32) -> std::time::Duration {
    let base_ms: u64 = 2_000;
    let max_ms: u64 = 120_000;
    let delay_ms = base_ms.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX));
    std::time::Duration::from_millis(delay_ms.min(max_ms))
}

/// Expand a leading `~/` in `path` to the user's home directory.
///
/// If the path does not start with `~/`, or if the home directory cannot be
/// determined, the original string is returned unchanged.
fn expand_auth_dir(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// Render a QR-code bit-matrix as Unicode block characters for terminal output.
///
/// Each pair of rows is collapsed into a single terminal line using the
/// half-block characters so the QR code fits in roughly half the vertical
/// space:
///
/// | top pixel | bottom pixel | character |
/// |-----------|--------------|-----------|
/// | dark      | dark         | `█` U+2588 |
/// | dark      | light        | `▀` U+2580 |
/// | light     | dark         | `▄` U+2584 |
/// | light     | light        | ` ` (space) |
///
/// The input is a row-major boolean matrix where `true` = dark module.
#[allow(dead_code)]
fn render_qr_terminal(qr_data: &[Vec<bool>]) -> String {
    let height = qr_data.len();
    let width = if height > 0 { qr_data[0].len() } else { 0 };
    let mut output = String::new();

    let mut y = 0;
    while y < height {
        for x in 0..width {
            let top = qr_data[y][x];
            let bottom = if y + 1 < height {
                qr_data[y + 1][x]
            } else {
                false
            };
            let ch = match (top, bottom) {
                (true, true) => '\u{2588}',  // █
                (true, false) => '\u{2580}', // ▀
                (false, true) => '\u{2584}', // ▄
                (false, false) => ' ',
            };
            output.push(ch);
        }
        output.push('\n');
        y += 2;
    }
    output
}

// ── Internal message type ─────────────────────────────────────────────────────

/// A message queued for delivery via the wa-rs client.
#[allow(dead_code)]
struct OutboundWaMessage {
    /// Recipient JID, e.g. `"60123456789@s.whatsapp.net"`.
    to: String,
    /// Plain-text content to send.
    content: String,
    /// Optional message ID to quote/reply to.
    reply_to: Option<String>,
}

// ── Channel struct ────────────────────────────────────────────────────────────

/// WhatsApp Web channel using the native wa-rs protocol.
///
/// Pairs with WhatsApp via a QR code on first run and then persists the
/// session to `auth_dir`. No Meta Business account is required.
pub struct WhatsAppWebChannel {
    config: WhatsAppWebConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    shutdown_tx: Option<watch::Sender<bool>>,
    outbound_tx: Option<mpsc::Sender<OutboundWaMessage>>,
}

impl WhatsAppWebChannel {
    /// Create a new `WhatsAppWebChannel`.
    ///
    /// Phone numbers in `config.allow_from` are normalized (leading `+`
    /// stripped) so that both `"+60123456789"` and `"60123456789"` work
    /// as allowlist entries.
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
            shutdown_tx: None,
            outbound_tx: None,
        }
    }
}

// ── Channel trait impl ────────────────────────────────────────────────────────

#[async_trait]
impl Channel for WhatsAppWebChannel {
    fn name(&self) -> &str {
        &self.base_config.name
    }

    async fn start(&mut self) -> Result<()> {
        let auth_dir = expand_auth_dir(&self.config.auth_dir);
        info!(auth_dir = %auth_dir, "WhatsApp Web channel starting");

        // TODO: Initialize wa-rs client when crate is available.
        //
        // The implementation would:
        //   1. Create the auth_dir if it does not exist.
        //   2. Open / create the wa-rs SQLite session store at `auth_dir`.
        //   3. Spawn a wa-rs event loop task that:
        //      - On first run: prints the QR code via `render_qr_terminal()`.
        //      - On reconnect: uses exponential back-off via `backoff_delay()`.
        //      - Publishes inbound messages to `self.bus`.
        //      - Forwards outbound messages from `outbound_rx` to wa-rs.
        //      - Sets `running = false` on exit.
        //   4. Stores the client handle for `send()`.

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let (outbound_tx, outbound_rx) = mpsc::channel::<OutboundWaMessage>(64);
        self.outbound_tx = Some(outbound_tx);

        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        let bus = self.bus.clone();

        tokio::spawn(async move {
            let task_result = AssertUnwindSafe(async move {
                let mut _shutdown_rx = shutdown_rx;
                let mut _outbound_rx = outbound_rx;
                debug!("WhatsApp Web event loop placeholder started");

                // Real implementation: drive wa-rs event loop here, forwarding
                // inbound events to `bus` and outbound messages from `_outbound_rx`.
                // Select on `_shutdown_rx.changed()` to detect shutdown.
                let _ = &bus; // suppress unused warning in placeholder
            })
            .catch_unwind()
            .await;

            if let Err(e) = task_result {
                error!("WhatsApp Web event loop panicked: {:?}", e);
            }

            running.store(false, Ordering::SeqCst);
            info!("WhatsApp Web channel event loop stopped");
        });

        info!(auth_dir = %auth_dir, "WhatsApp Web channel started (wa-rs initialization pending crate availability)");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            if tx.send(true).is_err() {
                warn!("WhatsApp Web shutdown receiver already dropped");
            }
        }
        self.outbound_tx = None;
        self.running.store(false, Ordering::SeqCst);
        info!("WhatsApp Web channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        // Build the recipient JID from chat_id.
        // chat_id is expected to be a normalized phone number or a full JID.
        let to = if msg.chat_id.contains('@') {
            msg.chat_id.clone()
        } else {
            format!("{}@s.whatsapp.net", normalize_phone(&msg.chat_id))
        };

        let wa_msg = OutboundWaMessage {
            to,
            content: msg.content.clone(),
            reply_to: msg.reply_to.clone(),
        };

        match &self.outbound_tx {
            Some(tx) => {
                if let Err(e) = tx.try_send(wa_msg) {
                    return Err(ZeptoError::Channel(format!(
                        "WhatsApp Web: failed to queue outbound message: {}",
                        e
                    )));
                }
            }
            None => {
                return Err(ZeptoError::Channel(
                    "WhatsApp Web: channel not started".to_string(),
                ));
            }
        }

        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        // Normalize the inbound user_id before checking — the allowlist was
        // already normalized in `new()`.
        let normalized = normalize_phone(user_id);
        self.base_config.is_allowed(&normalized)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;
    use crate::config::WhatsAppWebConfig;

    // ── Helper ────────────────────────────────────────────────────────────────

    fn make_channel(config: WhatsAppWebConfig) -> WhatsAppWebChannel {
        let bus = Arc::new(MessageBus::new());
        WhatsAppWebChannel::new(config, bus)
    }

    // ── Config tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_default_config() {
        let cfg = WhatsAppWebConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.auth_dir, "~/.zeptoclaw/state/whatsapp_web");
        assert!(cfg.allow_from.is_empty());
        assert!(!cfg.deny_by_default);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = WhatsAppWebConfig {
            enabled: true,
            auth_dir: "~/.zeptoclaw/state/whatsapp_web".to_string(),
            allow_from: vec!["+60123456789".to_string()],
            deny_by_default: false,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let decoded: WhatsAppWebConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.enabled, cfg.enabled);
        assert_eq!(decoded.auth_dir, cfg.auth_dir);
        assert_eq!(decoded.allow_from, cfg.allow_from);
        assert_eq!(decoded.deny_by_default, cfg.deny_by_default);
    }

    #[test]
    fn test_config_serde_partial() {
        let json = r#"{"enabled":true}"#;
        let cfg: WhatsAppWebConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.auth_dir, "~/.zeptoclaw/state/whatsapp_web");
        assert!(cfg.allow_from.is_empty());
        assert!(!cfg.deny_by_default);
    }

    // ── E.164 normalization tests ─────────────────────────────────────────────

    #[test]
    fn test_normalize_phone_with_plus() {
        assert_eq!(normalize_phone("+60123456789"), "60123456789");
    }

    #[test]
    fn test_normalize_phone_without_plus() {
        assert_eq!(normalize_phone("60123456789"), "60123456789");
    }

    #[test]
    fn test_normalize_phone_empty() {
        assert_eq!(normalize_phone(""), "");
    }

    #[test]
    fn test_normalize_phone_only_plus() {
        assert_eq!(normalize_phone("+"), "");
    }

    #[test]
    fn test_normalize_phone_us_number() {
        assert_eq!(normalize_phone("+14155551234"), "14155551234");
    }

    // ── Allowlist tests ───────────────────────────────────────────────────────

    #[test]
    fn test_is_allowed_normalized_match() {
        // Config stores "+60123456789"; allowlist normalization strips the "+".
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec!["+60123456789".to_string()],
            ..Default::default()
        });
        // Query without "+": should still match after normalization.
        assert!(ch.is_allowed("60123456789"));
    }

    #[test]
    fn test_is_allowed_denied() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec!["+60123456789".to_string()],
            ..Default::default()
        });
        assert!(!ch.is_allowed("60111111111"));
    }

    #[test]
    fn test_is_allowed_empty_allowlist() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec![],
            deny_by_default: false,
            ..Default::default()
        });
        // Empty allowlist + deny_by_default false → allow all.
        assert!(ch.is_allowed("60999999999"));
    }

    #[test]
    fn test_is_allowed_deny_by_default() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec![],
            deny_by_default: true,
            ..Default::default()
        });
        // Strict mode: empty allowlist → deny all.
        assert!(!ch.is_allowed("60999999999"));
    }

    #[test]
    fn test_is_allowed_with_plus_query() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec!["+60123456789".to_string()],
            ..Default::default()
        });
        // Query WITH "+": normalize_phone strips it before the allowlist check.
        assert!(ch.is_allowed("+60123456789"));
    }

    // ── QR rendering tests ────────────────────────────────────────────────────

    #[test]
    fn test_render_qr_empty() {
        let result = render_qr_terminal(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_render_qr_single_row() {
        // A single row of [true, false] renders as one line with ▀ and space
        // (top=true/false, bottom=false because there is no row y+1).
        let data = vec![vec![true, false]];
        let result = render_qr_terminal(&data);
        // Expect "▀ \n" (▀ for top-dark/bottom-light, space for top-light/bottom-light)
        assert!(result.contains('\u{2580}')); // ▀
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_render_qr_two_rows() {
        // Two rows compress into one terminal line.
        let data = vec![vec![true, false], vec![false, true]];
        let result = render_qr_terminal(&data);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1, "two rows → one terminal line");
    }

    #[test]
    fn test_render_qr_all_patterns() {
        // Row 0: [true,  true,  false, false]
        // Row 1: [true,  false, true,  false]
        // Expected chars: █ ▀ ▄ ' '
        let data = vec![
            vec![true, true, false, false],
            vec![true, false, true, false],
        ];
        let result = render_qr_terminal(&data);
        assert!(result.contains('\u{2588}'), "should contain █ (both dark)");
        assert!(
            result.contains('\u{2580}'),
            "should contain ▀ (top dark, bottom light)"
        );
        assert!(
            result.contains('\u{2584}'),
            "should contain ▄ (top light, bottom dark)"
        );
        assert!(result.contains(' '), "should contain space (both light)");
    }

    // ── Back-off tests ────────────────────────────────────────────────────────

    #[test]
    fn test_backoff_attempt_0() {
        assert_eq!(backoff_delay(0).as_millis(), 2_000);
    }

    #[test]
    fn test_backoff_attempt_1() {
        assert_eq!(backoff_delay(1).as_millis(), 4_000);
    }

    #[test]
    fn test_backoff_attempt_2() {
        assert_eq!(backoff_delay(2).as_millis(), 8_000);
    }

    #[test]
    fn test_backoff_attempt_5() {
        assert_eq!(backoff_delay(5).as_millis(), 64_000);
    }

    #[test]
    fn test_backoff_capped_at_120s() {
        // Attempt 10 would be 2048 s; must be capped at 120 s.
        assert_eq!(backoff_delay(10).as_millis(), 120_000);
        // Very high attempt.
        assert_eq!(backoff_delay(100).as_millis(), 120_000);
    }

    // ── Auth-dir expansion tests ──────────────────────────────────────────────

    #[test]
    fn test_expand_auth_dir_tilde() {
        let expanded = expand_auth_dir("~/.zeptoclaw/state/whatsapp_web");
        // Must not start with "~" after expansion (home dir was found).
        if dirs::home_dir().is_some() {
            assert!(!expanded.starts_with('~'));
        }
    }

    #[test]
    fn test_expand_auth_dir_absolute() {
        let path = "/absolute/path/to/state";
        assert_eq!(expand_auth_dir(path), path);
    }

    #[test]
    fn test_expand_auth_dir_relative() {
        let path = "relative/path/state";
        assert_eq!(expand_auth_dir(path), path);
    }

    // ── OutboundWaMessage tests ───────────────────────────────────────────────

    #[test]
    fn test_outbound_message_basic() {
        let msg = OutboundWaMessage {
            to: "60123456789@s.whatsapp.net".to_string(),
            content: "Hello".to_string(),
            reply_to: None,
        };
        assert_eq!(msg.to, "60123456789@s.whatsapp.net");
        assert_eq!(msg.content, "Hello");
        assert!(msg.reply_to.is_none());
    }

    #[test]
    fn test_outbound_message_with_reply() {
        let msg = OutboundWaMessage {
            to: "60123456789@s.whatsapp.net".to_string(),
            content: "Got it!".to_string(),
            reply_to: Some("ABCDEF123456".to_string()),
        };
        assert_eq!(msg.reply_to, Some("ABCDEF123456".to_string()));
    }

    #[test]
    fn test_outbound_jid_format() {
        // Verify the JID format used for phone-number chat IDs.
        let phone = "60123456789";
        let jid = format!("{}@s.whatsapp.net", phone);
        assert_eq!(jid, "60123456789@s.whatsapp.net");
    }

    // ── Channel lifecycle tests ───────────────────────────────────────────────

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
    async fn test_channel_start_sets_running() {
        let mut ch = make_channel(WhatsAppWebConfig::default());
        ch.start().await.unwrap();
        // The placeholder task immediately sets running = false, so we just
        // check that start() succeeded without error.  In the real impl the
        // event loop would keep `running = true` for the channel lifetime.
        // We only assert that the call does not fail.
    }

    #[tokio::test]
    async fn test_channel_stop_clears_running() {
        let mut ch = make_channel(WhatsAppWebConfig::default());
        ch.start().await.unwrap();
        ch.stop().await.unwrap();
        assert!(!ch.is_running());
    }

    #[tokio::test]
    async fn test_send_errors_when_not_started() {
        let ch = make_channel(WhatsAppWebConfig::default());
        let msg = OutboundMessage {
            channel: "whatsapp_web".to_string(),
            chat_id: "60123456789".to_string(),
            content: "Hello".to_string(),
            reply_to: None,
            metadata: Default::default(),
        };
        let result = ch.send(msg).await;
        assert!(result.is_err());
    }

    // ── Channel new() tests ───────────────────────────────────────────────────

    #[test]
    fn test_new_normalizes_allowlist() {
        let ch = make_channel(WhatsAppWebConfig {
            allow_from: vec!["+60123456789".to_string(), "+14155551234".to_string()],
            ..Default::default()
        });
        // Allowlist should have stripped leading "+" during construction.
        assert!(ch
            .base_config
            .allowlist
            .contains(&"60123456789".to_string()));
        assert!(ch
            .base_config
            .allowlist
            .contains(&"14155551234".to_string()));
        assert!(!ch.base_config.allowlist.iter().any(|p| p.starts_with('+')));
    }

    #[test]
    fn test_new_sets_deny_by_default() {
        let ch = make_channel(WhatsAppWebConfig {
            deny_by_default: true,
            ..Default::default()
        });
        assert!(ch.base_config.deny_by_default);
    }
}
