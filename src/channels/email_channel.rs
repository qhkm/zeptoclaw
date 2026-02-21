//! Email channel implementation — IMAP IDLE (inbound) + SMTP (outbound).
//!
//! Feature-gated behind `channel-email`:
//! - Without the feature: channel compiles but `start()` / `send()` return a
//!   clear error telling the user to rebuild.
//! - With the feature: full IMAP IDLE loop with automatic reconnect and SMTP
//!   send via STARTTLS.
//!
//! # Example configuration
//!
//! ```json
//! {
//!   "channels": {
//!     "email": {
//!       "imap_host": "imap.gmail.com",
//!       "smtp_host": "smtp.gmail.com",
//!       "username": "bot@gmail.com",
//!       "password": "app-password",
//!       "allowed_senders": ["@mycompany.com"]
//!     }
//!   }
//! }
//! ```

#[cfg(feature = "channel-email")]
use tracing::{error, info, warn};

use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Mutex;

#[cfg(feature = "channel-email")]
use crate::bus::InboundMessage;
use crate::bus::{MessageBus, OutboundMessage};
use crate::config::EmailConfig;
use crate::error::{Result, ZeptoError};

use super::{BaseChannelConfig, Channel};

// ---------------------------------------------------------------------------
// Channel struct (always compiled)
// ---------------------------------------------------------------------------

/// Email channel: IMAP IDLE for inbound push, SMTP for outbound.
///
/// This struct always compiles regardless of the `channel-email` feature.
/// Without the feature, `start()` and `send()` return a clear rebuild error.
#[cfg_attr(not(feature = "channel-email"), allow(dead_code))]
pub struct EmailChannel {
    config: EmailConfig,
    base_config: BaseChannelConfig,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    /// Tracks Message-IDs seen in the current session to avoid reprocessing on
    /// reconnect.
    seen_ids: Arc<Mutex<HashSet<String>>>,
}

impl EmailChannel {
    /// Create a new `EmailChannel` from configuration.
    pub fn new(config: EmailConfig, bus: Arc<MessageBus>) -> Self {
        let base_config = BaseChannelConfig {
            name: "email".to_string(),
            allowlist: config.allowed_senders.clone(),
            deny_by_default: config.deny_by_default,
        };
        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            seen_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Returns the channel name.
    pub fn channel_name(&self) -> &str {
        "email"
    }

    /// Check whether the given sender email address is permitted.
    ///
    /// Rules (applied in order):
    /// 1. `"*"` in allowlist → allow all.
    /// 2. Exact address match (case-insensitive).
    /// 3. Domain with `@` prefix (`"@example.com"`) matches any address at that domain.
    /// 4. Domain without `@` prefix (`"example.com"`) also matches any address at that domain.
    /// 5. Empty allowlist + `deny_by_default = false` → allow all.
    /// 6. Empty allowlist + `deny_by_default = true` → deny all.
    pub fn is_sender_allowed(&self, from: &str) -> bool {
        let list = &self.config.allowed_senders;

        if list.is_empty() {
            return !self.config.deny_by_default;
        }

        if list.iter().any(|a| a == "*") {
            return true;
        }

        let from_lower = from.to_lowercase();
        list.iter().any(|allowed| {
            if allowed.starts_with('@') {
                from_lower.ends_with(&allowed.to_lowercase())
            } else if allowed.contains('@') {
                allowed.eq_ignore_ascii_case(from)
            } else {
                from_lower.ends_with(&format!("@{}", allowed.to_lowercase()))
            }
        })
    }

    /// Extract plain text body from a `mail_parser::Message`.
    ///
    /// Only available when the `channel-email` feature is enabled.
    #[cfg(feature = "channel-email")]
    pub fn extract_plain_text(msg: &mail_parser::Message) -> String {
        msg.body_text(0)
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                msg.body_html(0)
                    .map(|h| Self::strip_html(h.as_ref()))
                    .unwrap_or_default()
            })
    }

    /// Naive HTML tag stripper (no external dep required).
    pub fn strip_html(html: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // ------------------------------------------------------------------
    // Feature-gated internals
    // ------------------------------------------------------------------

    /// Connect to the IMAP server with implicit TLS (port 993) and authenticate.
    #[cfg(feature = "channel-email")]
    async fn connect_imap(
        &self,
    ) -> std::result::Result<
        async_imap::Session<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
        ZeptoError,
    > {
        use rustls::{ClientConfig as RustlsClientConfig, RootCertStore};
        use tokio::net::TcpStream;
        use tokio_rustls::TlsConnector;

        let addr = format!("{}:{}", self.config.imap_host, self.config.imap_port);

        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| ZeptoError::Channel(format!("IMAP TCP connect failed: {e}")))?;

        let cert_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        let tls_cfg = RustlsClientConfig::builder()
            .with_root_certificates(cert_store)
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(tls_cfg));
        let sni = rustls::pki_types::ServerName::try_from(self.config.imap_host.clone())
            .map_err(|e| ZeptoError::Channel(format!("Invalid IMAP hostname: {e}")))?;

        let tls_stream = connector
            .connect(sni, tcp)
            .await
            .map_err(|e| ZeptoError::Channel(format!("IMAP TLS handshake failed: {e}")))?;

        let client = async_imap::Client::new(tls_stream);
        let session = client
            .login(&self.config.username, &self.config.password)
            .await
            .map_err(|(e, _)| ZeptoError::Channel(format!("IMAP login failed: {e}")))?;

        Ok(session)
    }

    /// Run a single IMAP IDLE session: connect → select mailbox → process
    /// existing unseen messages → IDLE loop.
    #[cfg(feature = "channel-email")]
    async fn run_idle_session(&self) -> std::result::Result<(), ZeptoError> {
        use async_imap::extensions::idle::IdleResponse;
        use std::time::Duration;

        let mut session = self.connect_imap().await?;

        session
            .select(&self.config.imap_folder)
            .await
            .map_err(|e| ZeptoError::Channel(format!("IMAP SELECT failed: {e}")))?;

        info!(
            "Email IMAP IDLE listening on {} / {}",
            self.config.imap_host, self.config.imap_folder
        );

        self.process_unseen(&mut session).await?;

        loop {
            if !self.running.load(Ordering::Relaxed) {
                break;
            }

            let idle_timeout = Duration::from_secs(self.config.idle_timeout_secs);
            let mut idle = session.idle();

            idle.init()
                .await
                .map_err(|e| ZeptoError::Channel(format!("IDLE init failed: {e}")))?;

            let (wait_fut, _stop) = idle.wait();
            let idle_result = tokio::time::timeout(idle_timeout, wait_fut).await;

            session = idle
                .done()
                .await
                .map_err(|e| ZeptoError::Channel(format!("IDLE done failed: {e}")))?;

            match idle_result {
                Ok(Ok(IdleResponse::NewData(_))) => {
                    // New mail — process immediately.
                }
                Ok(Ok(IdleResponse::Timeout)) | Err(_) => {
                    // Server timeout or local timeout — defensive fetch then re-IDLE.
                }
                Ok(Ok(IdleResponse::ManualInterrupt)) => {
                    info!("IMAP IDLE interrupted — stopping email channel");
                    break;
                }
                Ok(Err(e)) => {
                    return Err(ZeptoError::Channel(format!("IDLE wait error: {e}")));
                }
            }

            self.process_unseen(&mut session).await?;
        }

        let _ = session.logout().await;
        Ok(())
    }

    /// Fetch UNSEEN messages, deduplicate, filter by allowlist, and publish to bus.
    #[cfg(feature = "channel-email")]
    async fn process_unseen(
        &self,
        session: &mut async_imap::Session<
            tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
        >,
    ) -> std::result::Result<(), ZeptoError> {
        use futures::TryStreamExt;
        use mail_parser::MessageParser;

        let uids = session
            .uid_search("UNSEEN")
            .await
            .map_err(|e| ZeptoError::Channel(format!("IMAP SEARCH UNSEEN failed: {e}")))?;

        if uids.is_empty() {
            return Ok(());
        }

        let uid_set = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let raw_messages: Vec<async_imap::types::Fetch> = session
            .uid_fetch(&uid_set, "RFC822")
            .await
            .map_err(|e| ZeptoError::Channel(format!("IMAP FETCH failed: {e}")))?
            .try_collect()
            .await
            .map_err(|e| ZeptoError::Channel(format!("IMAP FETCH stream failed: {e}")))?;

        let parser = MessageParser::default();
        let inbound_tx = self.bus.inbound_sender();

        for raw in &raw_messages {
            let body = match raw.body() {
                Some(b) => b,
                None => continue,
            };

            let parsed = match parser.parse(body) {
                Some(m) => m,
                None => {
                    warn!("Failed to parse email body");
                    continue;
                }
            };

            let msg_id = parsed
                .message_id()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("gen-{}", uuid::Uuid::new_v4()));

            let from = parsed
                .from()
                .and_then(|a| a.first())
                .and_then(|a| a.address())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".into());

            if !self.is_sender_allowed(&from) {
                warn!("Blocked email from {from}");
                continue;
            }

            let is_new = {
                let mut seen = self.seen_ids.lock().await;
                seen.insert(msg_id.clone())
            };
            if !is_new {
                continue;
            }

            let subject = parsed.subject().unwrap_or("(no subject)").to_string();
            let body_text = Self::extract_plain_text(&parsed);
            let content = format!("Subject: {subject}\n\n{body_text}");

            let inbound = InboundMessage::new("email", &from, &from, &content)
                .with_metadata("message_id", &msg_id)
                .with_metadata("subject", &subject);

            if inbound_tx.send(inbound).await.is_err() {
                return Ok(());
            }
        }

        // Mark fetched messages as \Seen.
        if !raw_messages.is_empty() {
            let _ = session
                .uid_store(&uid_set, "+FLAGS (\\Seen)")
                .await;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Channel trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &str {
        "email"
    }

    async fn start(&mut self) -> Result<()> {
        #[cfg(not(feature = "channel-email"))]
        {
            return Err(ZeptoError::Channel(
                "Email channel requires the 'channel-email' build feature. \
                 Rebuild with: cargo build --features channel-email"
                    .into(),
            ));
        }

        #[cfg(feature = "channel-email")]
        {
            if self.running.load(Ordering::Relaxed) {
                warn!("Email channel already running");
                return Ok(());
            }

            self.running.store(true, Ordering::Relaxed);

            let config = self.config.clone();
            let bus = Arc::clone(&self.bus);
            let seen_ids = Arc::clone(&self.seen_ids);
            let this_running = Arc::clone(&self.running);

            tokio::spawn(async move {
                let channel = EmailChannel {
                    config,
                    base_config: BaseChannelConfig::new("email"),
                    bus,
                    running: Arc::clone(&this_running),
                    seen_ids,
                };

                let mut backoff = std::time::Duration::from_secs(1);
                let max_backoff = std::time::Duration::from_secs(60);

                while this_running.load(Ordering::Relaxed) {
                    match channel.run_idle_session().await {
                        Ok(()) => break,
                        Err(e) => {
                            error!("Email IMAP session error: {e}. Reconnecting in {backoff:?}…");
                            tokio::time::sleep(backoff).await;
                            backoff = std::cmp::min(backoff * 2, max_backoff);
                        }
                    }
                }

                this_running.store(false, Ordering::Relaxed);
                info!("Email channel stopped");
            });

            info!(
                "Email channel starting (IMAP IDLE on {})",
                self.config.imap_host
            );
            Ok(())
        }
    }

    async fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        #[cfg(not(feature = "channel-email"))]
        {
            let _ = msg;
            return Err(ZeptoError::Channel(
                "Email channel requires the 'channel-email' build feature. \
                 Rebuild with: cargo build --features channel-email"
                    .into(),
            ));
        }

        #[cfg(feature = "channel-email")]
        {
            use lettre::{
                message::SinglePart,
                transport::smtp::authentication::Credentials,
                Message as LettreMessage, SmtpTransport, Transport,
            };

            let (subject, body) = if msg.content.starts_with("Subject: ") {
                if let Some(pos) = msg.content.find("\n\n") {
                    (
                        msg.content[9..pos].to_string(),
                        msg.content[pos + 2..].to_string(),
                    )
                } else if let Some(pos) = msg.content.find('\n') {
                    (
                        msg.content[9..pos].to_string(),
                        msg.content[pos + 1..].to_string(),
                    )
                } else {
                    (
                        "ZeptoClaw Message".to_string(),
                        msg.content.clone(),
                    )
                }
            } else {
                ("ZeptoClaw Message".to_string(), msg.content.clone())
            };

            let from_addr = if let Some(ref name) = self.config.display_name {
                format!("{name} <{}>", self.config.username)
            } else {
                self.config.username.clone()
            };

            let email = LettreMessage::builder()
                .from(
                    from_addr
                        .parse()
                        .map_err(|e| ZeptoError::Channel(format!("Invalid from address: {e}")))?,
                )
                .to(msg
                    .chat_id
                    .parse()
                    .map_err(|e| ZeptoError::Channel(format!("Invalid to address: {e}")))?)
                .subject(subject)
                .singlepart(SinglePart::plain(body))
                .map_err(|e| ZeptoError::Channel(format!("Failed to build email: {e}")))?;

            let creds =
                Credentials::new(self.config.username.clone(), self.config.password.clone());

            let transport = SmtpTransport::starttls_relay(&self.config.smtp_host)
                .map_err(|e| ZeptoError::Channel(format!("SMTP relay error: {e}")))?
                .port(self.config.smtp_port)
                .credentials(creds)
                .build();

            transport
                .send(&email)
                .map_err(|e| ZeptoError::Channel(format!("SMTP send failed: {e}")))?;

            info!("Email sent to {}", msg.chat_id);
            Ok(())
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn is_allowed(&self, user_id: &str) -> bool {
        self.base_config.is_allowed(user_id)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> EmailConfig {
        EmailConfig {
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            username: "bot@example.com".into(),
            password: "secret".into(),
            imap_folder: "INBOX".into(),
            display_name: Some("My Bot".into()),
            allowed_senders: vec![],
            deny_by_default: false,
            idle_timeout_secs: 1740,
        }
    }

    fn make_channel(config: EmailConfig) -> EmailChannel {
        let bus = Arc::new(MessageBus::new());
        EmailChannel::new(config, bus)
    }

    // ---- sender allowlist tests ----

    #[test]
    fn test_sender_allowed_empty_list() {
        // deny_by_default = false, allowed_senders empty → allow all
        let ch = make_channel(make_config());
        assert!(ch.is_sender_allowed("anyone@example.com"));
        assert!(ch.is_sender_allowed("random@test.org"));
    }

    #[test]
    fn test_sender_filtered_by_allowlist() {
        let mut cfg = make_config();
        cfg.allowed_senders = vec!["allowed@example.com".into()];
        let ch = make_channel(cfg);
        assert!(ch.is_sender_allowed("allowed@example.com"));
        assert!(!ch.is_sender_allowed("blocked@example.com"));
    }

    #[test]
    fn test_sender_case_insensitive() {
        let mut cfg = make_config();
        cfg.allowed_senders = vec!["Allowed@Example.COM".into()];
        let ch = make_channel(cfg);
        assert!(ch.is_sender_allowed("allowed@example.com"));
        assert!(ch.is_sender_allowed("ALLOWED@EXAMPLE.COM"));
        assert!(ch.is_sender_allowed("AlLoWeD@eXaMpLe.cOm"));
    }

    #[test]
    fn test_sender_wildcard_allows_all() {
        let mut cfg = make_config();
        cfg.allowed_senders = vec!["*".into()];
        let ch = make_channel(cfg);
        assert!(ch.is_sender_allowed("anyone@anywhere.com"));
    }

    #[test]
    fn test_sender_domain_with_at_prefix() {
        let mut cfg = make_config();
        cfg.allowed_senders = vec!["@example.com".into()];
        let ch = make_channel(cfg);
        assert!(ch.is_sender_allowed("user@example.com"));
        assert!(ch.is_sender_allowed("admin@example.com"));
        assert!(!ch.is_sender_allowed("user@other.com"));
    }

    #[test]
    fn test_sender_domain_without_at_prefix() {
        let mut cfg = make_config();
        cfg.allowed_senders = vec!["example.com".into()];
        let ch = make_channel(cfg);
        assert!(ch.is_sender_allowed("user@example.com"));
        assert!(!ch.is_sender_allowed("user@other.com"));
    }

    // ---- dedup tests ----

    #[tokio::test]
    async fn test_dedup_new_id_accepted() {
        let ch = make_channel(make_config());
        let mut seen = ch.seen_ids.lock().await;
        assert!(seen.insert("msg-001".to_string()));
    }

    #[tokio::test]
    async fn test_dedup_duplicate_rejected() {
        let ch = make_channel(make_config());
        let mut seen = ch.seen_ids.lock().await;
        seen.insert("msg-dup".to_string());
        assert!(!seen.insert("msg-dup".to_string()));
    }

    // ---- name test ----

    #[test]
    fn test_channel_name() {
        let ch = make_channel(make_config());
        assert_eq!(ch.name(), "email");
    }

    // ---- config defaults ----

    #[test]
    fn test_config_default_ports() {
        let cfg = EmailConfig::default();
        assert_eq!(cfg.imap_port, 993);
        assert_eq!(cfg.smtp_port, 587);
    }

    #[test]
    fn test_config_default_folder() {
        let cfg = EmailConfig::default();
        assert_eq!(cfg.imap_folder, "INBOX");
    }

    #[test]
    fn test_config_default_idle_timeout() {
        let cfg = EmailConfig::default();
        assert_eq!(cfg.idle_timeout_secs, 1740);
    }

    // ---- deny_by_default tests ----

    #[test]
    fn test_deny_by_default_false_allows_all() {
        let mut cfg = make_config();
        cfg.deny_by_default = false;
        cfg.allowed_senders = vec![];
        let ch = make_channel(cfg);
        assert!(ch.is_sender_allowed("anyone@example.com"));
    }

    #[test]
    fn test_deny_by_default_true_with_empty_allowlist() {
        let mut cfg = make_config();
        cfg.deny_by_default = true;
        cfg.allowed_senders = vec![];
        let ch = make_channel(cfg);
        assert!(!ch.is_sender_allowed("anyone@example.com"));
    }

    // ---- feature-gated extract_plain_text ----

    #[cfg(feature = "channel-email")]
    #[test]
    fn test_extract_plain_text_from_email() {
        let raw = b"From: sender@example.com\r\n\
                    To: bot@example.com\r\n\
                    Subject: Test\r\n\
                    Content-Type: text/plain; charset=UTF-8\r\n\
                    \r\n\
                    Hello, world!\r\n";

        let parsed = mail_parser::MessageParser::default()
            .parse(raw.as_ref())
            .expect("parse failed");
        let text = EmailChannel::extract_plain_text(&parsed);
        assert!(text.contains("Hello, world!"), "got: {text}");
    }

    // ---- serde roundtrip ----

    #[test]
    fn test_config_serde_roundtrip() {
        let json = serde_json::json!({
            "imap_host": "imap.gmail.com",
            "smtp_host": "smtp.gmail.com",
            "username": "user@gmail.com",
            "password": "pass"
        });
        let cfg: EmailConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.imap_port, 993);
        assert_eq!(cfg.smtp_port, 587);
        assert_eq!(cfg.imap_folder, "INBOX");
        assert!(!cfg.deny_by_default);
        assert!(cfg.allowed_senders.is_empty());
    }

    // ---- strip_html helper ----

    #[test]
    fn test_strip_html_basic() {
        assert_eq!(EmailChannel::strip_html("<p>Hello</p>"), "Hello");
        assert_eq!(EmailChannel::strip_html("<b>World</b>"), "World");
    }

    #[test]
    fn test_strip_html_no_tags() {
        assert_eq!(EmailChannel::strip_html("plain text"), "plain text");
        assert_eq!(EmailChannel::strip_html(""), "");
    }

    // ---- is_running default ----

    #[test]
    fn test_is_running_default_false() {
        let ch = make_channel(make_config());
        assert!(!ch.is_running());
    }
}
