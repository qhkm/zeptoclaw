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
//! let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);
//! ```

use async_trait::async_trait;
use futures::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::Config;
use crate::config::TelegramConfig;
use crate::error::{Result, ZeptoError};
use crate::memory::builtin_searcher::BuiltinSearcher;
use crate::memory::longterm::LongTermMemory;

/// Maximum number of startup connectivity retries before giving up.
const MAX_STARTUP_RETRIES: u32 = 10;
/// Base delay (in seconds) for exponential backoff on startup retries.
const BASE_RETRY_DELAY_SECS: u64 = 2;
/// Maximum delay (in seconds) for exponential backoff on startup retries.
const MAX_RETRY_DELAY_SECS: u64 = 120;

use super::model_switch::{
    format_current_model, format_model_list, hydrate_overrides, new_override_store,
    parse_model_command, persist_single, remove_single, ModelCommand, ModelOverrideStore,
};
use super::persona_switch::{self, PersonaCommand, PersonaOverrideStore};
use super::{BaseChannelConfig, Channel};

/// Newtype wrappers to disambiguate `Vec<String>` / `String` in dptree's
/// type-based DI. Without these, the last registered value of a given type
/// silently overwrites earlier ones.
#[derive(Clone)]
struct Allowlist(Vec<String>);
#[derive(Clone)]
struct DefaultModel(String);
#[derive(Clone)]
struct ConfiguredProviders(Vec<String>);
/// Bundles both override stores into one DI dependency so that dptree's
/// 9-parameter arity limit is not exceeded.
#[derive(Clone)]
struct OverridesDep {
    model: ModelOverrideStore,
    persona: PersonaOverrideStore,
}

fn render_telegram_html(content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 16);
    let mut chars = content.chars().peekable();
    let mut spoiler_open = false;

    while let Some(ch) = chars.next() {
        if ch == '|' && chars.peek() == Some(&'|') {
            let _ = chars.next();
            if spoiler_open {
                out.push_str("</tg-spoiler>");
            } else {
                out.push_str("<tg-spoiler>");
            }
            spoiler_open = !spoiler_open;
            continue;
        }

        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }

    // Graceful fallback for unmatched spoiler marker.
    if spoiler_open {
        out.push_str("</tg-spoiler>");
    }

    out
}

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
    /// Per-chat model overrides (in-memory)
    model_overrides: ModelOverrideStore,
    /// Per-chat persona overrides (in-memory)
    persona_overrides: PersonaOverrideStore,
    /// Default model name for /model status output
    default_model: String,
    /// Configured providers (for /model list)
    configured_providers: Vec<String>,
    /// Long-term memory backing store for model overrides (optional)
    longterm_memory: Option<Arc<Mutex<LongTermMemory>>>,
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
    /// let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);
    ///
    /// assert_eq!(channel.name(), "telegram");
    /// assert!(!channel.is_running());
    /// ```
    pub fn new(
        config: TelegramConfig,
        bus: Arc<MessageBus>,
        default_model: String,
        configured_providers: Vec<String>,
        memory_enabled: bool,
    ) -> Self {
        let base_config = BaseChannelConfig {
            name: "telegram".to_string(),
            allowlist: config.allow_from.clone(),
            deny_by_default: config.deny_by_default,
        };
        let longterm_memory = if memory_enabled {
            // Use a dedicated file to avoid conflicts with the agent loop's longterm.json.
            // Two LongTermMemory instances writing to the same file can cause data loss.
            let ltm_path = Config::dir().join("memory").join("model_prefs.json");
            match LongTermMemory::with_path_and_searcher(ltm_path, Arc::new(BuiltinSearcher)) {
                Ok(ltm) => Some(Arc::new(Mutex::new(ltm))),
                Err(e) => {
                    warn!(
                        "Failed to initialize long-term memory for Telegram model switching: {}",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };
        Self {
            config,
            base_config,
            bus,
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: None,
            bot: None,
            model_overrides: new_override_store(),
            persona_overrides: persona_switch::new_persona_store(),
            default_model,
            configured_providers,
            longterm_memory,
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

    /// Calculates the exponential backoff delay for a startup retry attempt.
    fn startup_backoff_delay(attempt: u32) -> Duration {
        let delay_secs = BASE_RETRY_DELAY_SECS
            .saturating_mul(2u64.saturating_pow(attempt))
            .min(MAX_RETRY_DELAY_SECS);
        Duration::from_secs(delay_secs)
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
                ZeptoError::Channel(format!("Failed to build Telegram HTTP client: {}", e))
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
            return Err(ZeptoError::Config("Telegram bot token is empty".into()));
        }

        info!("Starting Telegram channel");

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Clone values for the spawned task
        let token = self.config.token.clone();
        let bus = self.bus.clone();
        let allowlist = Allowlist(self.config.allow_from.clone());
        let deny_by_default = self.config.deny_by_default;
        let overrides_dep = OverridesDep {
            model: self.model_overrides.clone(),
            persona: self.persona_overrides.clone(),
        };
        let default_model = DefaultModel(self.default_model.clone());
        let configured_providers = ConfiguredProviders(self.configured_providers.clone());
        let longterm_memory = self.longterm_memory.clone();
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

        if let Some(ltm) = self.longterm_memory.as_ref() {
            hydrate_overrides(&self.model_overrides, ltm).await;
        }
        if let Some(ltm) = self.longterm_memory.as_ref() {
            persona_switch::hydrate_overrides(&self.persona_overrides, ltm).await;
        }

        // Spawn the bot polling task
        tokio::spawn(async move {
            use teloxide::prelude::*;

            let task_result = std::panic::AssertUnwindSafe(async move {
                // Perform a startup check with retries so transient errors (DNS
                // not ready, network interface still coming up) don't permanently
                // kill the channel.  Permanent errors (invalid token, API errors)
                // bail immediately on the first attempt.
                let mut attempt: u32 = 0;
                loop {
                    match bot.get_me().await {
                        Ok(_) => break,
                        Err(e) => {
                            use teloxide::RequestError;

                            let is_transient = matches!(
                                &e,
                                RequestError::Network(_)
                                    | RequestError::Io(_)
                                    | RequestError::RetryAfter(_)
                            );

                            if !is_transient || attempt >= MAX_STARTUP_RETRIES {
                                error!(
                                    "Telegram startup check failed after {} attempt(s): {}",
                                    attempt + 1,
                                    e
                                );
                                return;
                            }

                            let delay = if let RequestError::RetryAfter(d) = &e {
                                d.duration()
                            } else {
                                TelegramChannel::startup_backoff_delay(attempt)
                            };
                            warn!(
                                "Telegram startup check failed (attempt {}/{}), retrying in {}s: {}",
                                attempt + 1,
                                MAX_STARTUP_RETRIES,
                                delay.as_secs(),
                                e
                            );
                            tokio::select! {
                                _ = shutdown_rx.recv() => {
                                    info!("Telegram channel shutdown during startup retry");
                                    return;
                                }
                                _ = tokio::time::sleep(delay) => {}
                            }
                            attempt += 1;
                        }
                    }
                }

                // Create the handler for incoming messages
                // Note: dptree injects dependencies separately, not as tuples
                let handler =
                    Update::filter_message().endpoint(
                        |bot: Bot,
                         msg: Message,
                         bus: Arc<MessageBus>,
                         Allowlist(allowlist): Allowlist,
                         deny_by_default: bool,
                         overrides_dep: OverridesDep,
                         DefaultModel(default_model): DefaultModel,
                         ConfiguredProviders(configured_providers): ConfiguredProviders,
                         longterm_memory: Option<Arc<Mutex<LongTermMemory>>>| async move {
                            let model_overrides = overrides_dep.model;
                            let persona_overrides = overrides_dep.persona;
                            // Extract user ID and optional username
                            let user = msg.from.as_ref();
                            let user_id = user
                                .map(|u| u.id.0.to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            let username = user
                                .and_then(|u| u.username.clone())
                                .unwrap_or_default();

                            // Check allowlist with deny_by_default support.
                            // Accepts both numeric IDs ("123456") and usernames ("alice" or "@alice").
                            let allowed = if allowlist.is_empty() {
                                !deny_by_default
                            } else {
                                allowlist.contains(&user_id)
                                    || (!username.is_empty()
                                        && allowlist.iter().any(|entry| {
                                            let entry_lower = entry.to_lowercase();
                                            let user_lower = username.to_lowercase();
                                            entry_lower == user_lower
                                                || entry_lower == format!("@{user_lower}")
                                                || format!("@{entry_lower}") == user_lower
                                        }))
                            };
                            if !allowed {
                                if allowlist.is_empty() {
                                    info!(
                                        "Telegram: User {} blocked — deny_by_default=true and allow_from is empty. \
                                         Add their numeric user ID to channels.telegram.allow_from in config.json",
                                        user_id
                                    );
                                } else {
                                    info!(
                                        "Telegram: User {} (@{}) not in allow_from list ({} entries configured), ignoring message",
                                        user_id,
                                        if username.is_empty() { "no_username" } else { &username },
                                        allowlist.len()
                                    );
                                }
                                return Ok(());
                            }

                            // Only process text messages
                            if let Some(text) = msg.text() {
                                let chat_id = msg.chat.id.0.to_string();
                                let chat_id_num = msg.chat.id.0;

                                // Extract forum topic thread ID for topic-aware routing.
                                // In teloxide 0.13, Message::thread_id is Option<ThreadId>
                                // where ThreadId wraps MessageId which wraps i32.
                                let thread_id: Option<String> =
                                    msg.thread_id.map(|t| t.0 .0.to_string());

                                // Build a topic-aware override key. When a topic thread
                                // is present, model/persona overrides are scoped per-topic
                                // so each forum topic can have its own model/persona.
                                let override_key = if let Some(ref tid) = thread_id {
                                    format!("{}:{}", chat_id, tid)
                                } else {
                                    chat_id.clone()
                                };

                                info!(
                                    "Telegram: Received message from user {} in chat {}: {}",
                                    user_id,
                                    chat_id,
                                    crate::utils::string::preview(text, 50)
                                );

                                /// Helper to attach message_thread_id to a SendMessage request.
                                fn apply_thread_id(
                                    req: teloxide::requests::JsonRequest<
                                        teloxide::payloads::SendMessage,
                                    >,
                                    thread_id: &Option<String>,
                                ) -> teloxide::requests::JsonRequest<
                                    teloxide::payloads::SendMessage,
                                > {
                                    if let Some(ref tid) = thread_id {
                                        if let Ok(id) = tid.parse::<i32>() {
                                            return req.message_thread_id(
                                                teloxide::types::ThreadId(
                                                    teloxide::types::MessageId(id),
                                                ),
                                            );
                                        }
                                    }
                                    req
                                }

                                // Intercept /model commands
                                // TODO(#63): Migrate to CommandInterceptor (Approach B) when adding /model
                                // to more channels. See docs/plans/2026-02-18-llm-switching-design.md
                                if let Some(cmd) = parse_model_command(text) {
                                    match cmd {
                                        ModelCommand::Show => {
                                            let current = {
                                                let overrides = model_overrides.read().await;
                                                overrides.get(&override_key).cloned()
                                            };
                                            let reply =
                                                format_current_model(current.as_ref(), &default_model);
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                        ModelCommand::Set(ov) => {
                                            let reply = format!(
                                                "Switched to {}:{}",
                                                ov.provider.as_deref().unwrap_or("auto"),
                                                ov.model
                                            );
                                            {
                                                let mut overrides = model_overrides.write().await;
                                                overrides.insert(override_key.clone(), ov.clone());
                                            }
                                            if let Some(ref ltm) = longterm_memory {
                                                persist_single(&override_key, &ov, ltm).await;
                                            }
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                        ModelCommand::Reset => {
                                            {
                                                let mut overrides = model_overrides.write().await;
                                                overrides.remove(&override_key);
                                            }
                                            if let Some(ref ltm) = longterm_memory {
                                                remove_single(&override_key, ltm).await;
                                            }
                                            let reply = format!("Reset to default: {}", default_model);
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                        ModelCommand::List => {
                                            let current = {
                                                let overrides = model_overrides.read().await;
                                                overrides.get(&override_key).cloned()
                                            };
                                            let reply = format_model_list(
                                                &configured_providers,
                                                current.as_ref(),
                                            );
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                    }
                                    return Ok(());
                                }

                                // Intercept /persona commands
                                if let Some(cmd) = persona_switch::parse_persona_command(text) {
                                    match cmd {
                                        PersonaCommand::Show => {
                                            let current = {
                                                let overrides = persona_overrides.read().await;
                                                overrides.get(&override_key).cloned()
                                            };
                                            let reply = persona_switch::format_current_persona(
                                                current.as_deref(),
                                            );
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                        PersonaCommand::Set(value) => {
                                            let resolved =
                                                persona_switch::resolve_soul_content(&value);
                                            let reply = if resolved.is_empty() {
                                                "Switched to default persona".to_string()
                                            } else {
                                                format!("Switched to persona: {}", value)
                                            };
                                            {
                                                let mut overrides =
                                                    persona_overrides.write().await;
                                                overrides
                                                    .insert(override_key.clone(), value.clone());
                                            }
                                            if let Some(ref ltm) = longterm_memory {
                                                persona_switch::persist_single(
                                                    &override_key, &value, ltm,
                                                )
                                                .await;
                                            }
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                        PersonaCommand::Reset => {
                                            {
                                                let mut overrides =
                                                    persona_overrides.write().await;
                                                overrides.remove(&override_key);
                                            }
                                            if let Some(ref ltm) = longterm_memory {
                                                persona_switch::remove_single(&override_key, ltm)
                                                    .await;
                                            }
                                            let reply =
                                                "Persona reset to default".to_string();
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                        PersonaCommand::List => {
                                            let current = {
                                                let overrides = persona_overrides.read().await;
                                                overrides.get(&override_key).cloned()
                                            };
                                            let reply = persona_switch::format_persona_list(
                                                current.as_deref(),
                                            );
                                            let req = bot
                                                .send_message(
                                                    teloxide::types::ChatId(chat_id_num),
                                                    reply,
                                                );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
                                    }
                                    return Ok(());
                                }

                                // Create and publish the inbound message
                                let mut inbound =
                                    InboundMessage::new("telegram", &user_id, &chat_id, text);

                                // For forum topics, override session key to isolate
                                // per-topic conversations and attach thread metadata
                                // so outbound replies route to the correct topic.
                                if let Some(ref tid) = thread_id {
                                    inbound.session_key =
                                        format!("telegram:{}:{}", chat_id, tid);
                                    inbound =
                                        inbound.with_metadata("telegram_thread_id", tid);
                                }

                                let override_entry = {
                                    let overrides = model_overrides.read().await;
                                    overrides.get(&override_key).cloned()
                                };
                                if let Some(ov) = override_entry {
                                    inbound = inbound.with_metadata("model_override", &ov.model);
                                    if let Some(provider) = ov.provider {
                                        inbound =
                                            inbound.with_metadata("provider_override", &provider);
                                    }
                                }

                                let persona_entry = {
                                    let overrides = persona_overrides.read().await;
                                    overrides.get(&override_key).cloned()
                                };
                                if let Some(persona_value) = persona_entry {
                                    inbound = inbound
                                        .with_metadata("persona_override", &persona_value);
                                }

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
                    .dependencies(dptree::deps![
                        bus,
                        allowlist,
                        deny_by_default,
                        overrides_dep,
                        default_model,
                        configured_providers,
                        longterm_memory
                    ])
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
        use teloxide::types::{ChatId, ParseMode};

        if !self.running.load(Ordering::SeqCst) {
            warn!("Telegram channel not running, cannot send message");
            return Err(ZeptoError::Channel(
                "Telegram channel not running".to_string(),
            ));
        }

        // Parse the chat ID
        let chat_id: i64 = msg.chat_id.parse().map_err(|_| {
            ZeptoError::Channel(format!("Invalid Telegram chat ID: {}", msg.chat_id))
        })?;

        info!("Telegram: Sending message to chat {}", chat_id);

        // Use cached bot instance
        let bot = self
            .bot
            .as_ref()
            .ok_or_else(|| ZeptoError::Channel("Telegram bot not initialized".to_string()))?;

        let rendered = render_telegram_html(&msg.content);
        let mut req = bot
            .send_message(ChatId(chat_id), rendered)
            .parse_mode(ParseMode::Html);

        // Route reply to the correct forum topic when thread metadata is present.
        if let Some(thread_id_str) = msg.metadata.get("telegram_thread_id") {
            if let Ok(tid) = thread_id_str.parse::<i32>() {
                req = req
                    .message_thread_id(teloxide::types::ThreadId(teloxide::types::MessageId(tid)));
            }
        }

        req.await
            .map_err(|e| ZeptoError::Channel(format!("Failed to send Telegram message: {}", e)))?;

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

        assert!(channel.is_allowed("user1"));
        assert!(channel.is_allowed("user2"));
        assert!(channel.is_allowed("admin"));
        assert!(!channel.is_allowed("user3"));
        assert!(!channel.is_allowed("hacker"));
    }

    #[test]
    fn test_render_telegram_html_escapes_html() {
        let rendered = render_telegram_html("5 < 7 & 9 > 2");
        assert_eq!(rendered, "5 &lt; 7 &amp; 9 &gt; 2");
    }

    #[test]
    fn test_render_telegram_html_spoiler_pairs() {
        let rendered = render_telegram_html("Secret: ||classified|| data");
        assert_eq!(rendered, "Secret: <tg-spoiler>classified</tg-spoiler> data");
    }

    #[test]
    fn test_render_telegram_html_unmatched_spoiler() {
        let rendered = render_telegram_html("Dangling ||spoiler");
        assert_eq!(rendered, "Dangling <tg-spoiler>spoiler</tg-spoiler>");
    }

    #[tokio::test]
    async fn test_telegram_start_without_token() {
        let config = TelegramConfig {
            enabled: true,
            token: String::new(), // Empty token
            allow_from: vec![],
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let mut channel =
            TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let mut channel =
            TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let mut channel =
            TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

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
            ..Default::default()
        };
        let bus = Arc::new(MessageBus::new());
        let channel = TelegramChannel::new(config, bus, "default-model".to_string(), vec![], false);

        // Verify base config is set correctly
        assert_eq!(channel.base_config.name, "telegram");
        assert_eq!(channel.base_config.allowlist, vec!["allowed_user"]);
    }

    // -----------------------------------------------------------------------
    // Startup retry backoff
    // -----------------------------------------------------------------------

    #[test]
    fn test_startup_backoff_delay_increases() {
        let d0 = TelegramChannel::startup_backoff_delay(0);
        let d1 = TelegramChannel::startup_backoff_delay(1);
        let d2 = TelegramChannel::startup_backoff_delay(2);
        assert_eq!(d0, Duration::from_secs(2));
        assert_eq!(d1, Duration::from_secs(4));
        assert_eq!(d2, Duration::from_secs(8));
        assert!(d1 > d0);
        assert!(d2 > d1);
    }

    #[test]
    fn test_startup_backoff_delay_caps_at_max() {
        let d_high = TelegramChannel::startup_backoff_delay(20);
        assert_eq!(d_high, Duration::from_secs(MAX_RETRY_DELAY_SECS));
    }

    #[test]
    fn test_startup_backoff_delay_no_overflow() {
        let d = TelegramChannel::startup_backoff_delay(u32::MAX);
        assert_eq!(d, Duration::from_secs(MAX_RETRY_DELAY_SECS));
    }

    // -----------------------------------------------------------------------
    // Forum Topics (thread_id) support
    // -----------------------------------------------------------------------

    #[test]
    fn test_thread_id_override_key() {
        // Override key includes thread_id when present (per-topic model/persona).
        let chat_id = "12345";
        let thread_id: Option<String> = Some("99".to_string());
        let override_key = if let Some(ref tid) = thread_id {
            format!("{}:{}", chat_id, tid)
        } else {
            chat_id.to_string()
        };
        assert_eq!(override_key, "12345:99");
    }

    #[test]
    fn test_thread_id_override_key_no_thread() {
        // Override key falls back to plain chat_id when no thread is present.
        let chat_id = "12345";
        let thread_id: Option<String> = None;
        let override_key = if let Some(ref tid) = thread_id {
            format!("{}:{}", chat_id, tid)
        } else {
            chat_id.to_string()
        };
        assert_eq!(override_key, "12345");
    }

    #[test]
    fn test_inbound_message_with_thread_id() {
        use crate::bus::InboundMessage;
        let mut inbound = InboundMessage::new("telegram", "user1", "chat1", "Hello");
        let thread_id = Some("42".to_string());
        if let Some(ref tid) = thread_id {
            inbound.session_key = format!("telegram:{}:{}", "chat1", tid);
            inbound = inbound.with_metadata("telegram_thread_id", tid);
        }
        assert_eq!(inbound.session_key, "telegram:chat1:42");
        assert_eq!(
            inbound.metadata.get("telegram_thread_id"),
            Some(&"42".to_string())
        );
    }

    #[test]
    fn test_outbound_with_thread_metadata() {
        use crate::bus::OutboundMessage;
        let msg = OutboundMessage::new("telegram", "chat1", "Reply")
            .with_metadata("telegram_thread_id", "42");
        assert_eq!(
            msg.metadata.get("telegram_thread_id"),
            Some(&"42".to_string())
        );
    }
}
