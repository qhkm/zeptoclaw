//! Container-based agent proxy that spawns containers for each request
//!
//! This module provides the `ContainerAgentProxy` which runs agents in isolated
//! containers (Docker or Apple Container), enabling multi-user scenarios with
//! proper isolation.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{watch, Semaphore};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::bus::{InboundMessage, MessageBus, OutboundMessage};
use crate::config::{Config, ContainerAgentBackend, ContainerAgentConfig};
use crate::error::{Result, ZeptoError};
use crate::health::UsageMetrics;
use crate::security::mount::validate_mount_not_blocked;
use crate::security::pairing::PairingManager;
use crate::session::SessionManager;

use super::idempotency::IdempotencyStore;
use super::ipc::{parse_marked_response, AgentRequest, AgentResponse, AgentResult};
use super::rate_limit::GatewayRateLimiter;

const CONTAINER_WORKSPACE_DIR: &str = "/data/.zeptoclaw/workspace";
const CONTAINER_SESSIONS_DIR: &str = "/data/.zeptoclaw/sessions";
const CONTAINER_CONFIG_PATH: &str = "/data/.zeptoclaw/config.json";

/// Path inside the container where the env file is mounted (Apple Container only).
#[cfg(target_os = "macos")]
const CONTAINER_ENV_DIR: &str = "/tmp/zeptoclaw-env";

/// Resolved backend after auto-detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    Docker,
    #[cfg(target_os = "macos")]
    Apple,
}

impl std::fmt::Display for ResolvedBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolvedBackend::Docker => write!(f, "docker"),
            #[cfg(target_os = "macos")]
            ResolvedBackend::Apple => write!(f, "apple-container"),
        }
    }
}

#[derive(Debug, Clone)]
struct ContainerInvocation {
    binary: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    /// Temp directory to clean up after container exits (Apple Container env file).
    temp_dir: Option<std::path::PathBuf>,
}

/// Proxy that spawns containers to process agent requests.
///
/// Each inbound message is processed in an isolated container, providing
/// security isolation for multi-user scenarios.
pub struct ContainerAgentProxy {
    config: Config,
    container_config: ContainerAgentConfig,
    bus: Arc<MessageBus>,
    session_manager: Option<SessionManager>,
    running: AtomicBool,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    usage_metrics: RwLock<Option<Arc<UsageMetrics>>>,
    resolved_backend: ResolvedBackend,
    semaphore: Arc<Semaphore>,
    /// Optional pairing manager for device token validation.
    /// Present only when `config.pairing.enabled` is true.
    pairing: Option<std::sync::Mutex<PairingManager>>,
    /// Per-IP rate limiter (None when both limits are 0 = unlimited).
    rate_limiter: Option<Arc<GatewayRateLimiter>>,
    /// Idempotency store for deduplicating messages with a "message_id" metadata key.
    idempotency: Arc<IdempotencyStore>,
}

impl ContainerAgentProxy {
    /// Create a new container agent proxy with explicit resolved backend.
    pub fn new(config: Config, bus: Arc<MessageBus>, backend: ResolvedBackend) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let container_config = config.container_agent.clone();
        let max_concurrent = container_config.max_concurrent.max(1);
        let session_manager = match SessionManager::new() {
            Ok(manager) => Some(manager),
            Err(e) => {
                warn!(
                    "Failed to initialize session manager for container agent proxy: {}",
                    e
                );
                None
            }
        };

        let pairing = if config.pairing.enabled {
            Some(std::sync::Mutex::new(PairingManager::new(
                config.pairing.max_attempts,
                config.pairing.lockout_secs,
            )))
        } else {
            None
        };

        let rl = &config.gateway.rate_limit;
        let rate_limiter = if rl.pair_per_min > 0 || rl.webhook_per_min > 0 {
            Some(Arc::new(GatewayRateLimiter::new(
                rl.pair_per_min,
                rl.webhook_per_min,
                Duration::from_secs(60),
            )))
        } else {
            None
        };

        let idempotency = Arc::new(IdempotencyStore::new(
            Duration::from_secs(300), // 5-minute dedup window
            10_000,                   // max tracked IDs
        ));

        Self {
            config,
            container_config,
            bus,
            session_manager,
            running: AtomicBool::new(false),
            shutdown_tx,
            shutdown_rx,
            usage_metrics: RwLock::new(None),
            resolved_backend: backend,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            pairing,
            rate_limiter,
            idempotency,
        }
    }

    /// Return the resolved backend.
    pub fn backend(&self) -> ResolvedBackend {
        self.resolved_backend
    }

    /// Enable usage metrics collection for this proxy.
    pub fn set_usage_metrics(&self, metrics: Arc<UsageMetrics>) {
        match self.usage_metrics.write() {
            Ok(mut guard) => *guard = Some(metrics),
            Err(e) => warn!("Failed to set usage metrics (poisoned lock): {}", e),
        }
    }

    /// Start the proxy loop, processing messages from the bus.
    ///
    /// Each inbound message is processed concurrently in its own spawned task,
    /// gated by a semaphore that limits the number of simultaneous container
    /// invocations to `container_agent.max_concurrent` (default: 5).
    pub async fn start(self: Arc<Self>) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(ZeptoError::Config(
                "Container agent proxy already running".into(),
            ));
        }

        info!(
            "Starting containerized agent proxy (backend={}, max_concurrent={})",
            self.resolved_backend, self.container_config.max_concurrent,
        );

        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Container agent proxy shutting down");
                        break;
                    }
                }
                msg = self.bus.consume_inbound() => {
                    match msg {
                        Some(inbound) => {
                            // Rate limit check (by source_ip metadata if present)
                            if let Some(ref limiter) = self.rate_limiter {
                                if let Some(ip_str) = inbound.metadata.get("source_ip") {
                                    if let Ok(ip) = ip_str.parse::<std::net::IpAddr>() {
                                        if !limiter.check_webhook(ip) {
                                            warn!(
                                                sender = %inbound.sender_id,
                                                ip = %ip,
                                                "Rate limited inbound message"
                                            );
                                            let rejection = OutboundMessage::new(
                                                &inbound.channel,
                                                &inbound.chat_id,
                                                "Rate limit exceeded. Please wait before sending another message.",
                                            );
                                            if let Err(e) = self.bus.publish_outbound(rejection).await {
                                                error!("Failed to publish rate limit rejection: {}", e);
                                            }
                                            continue;
                                        }
                                    }
                                }
                            }

                            // Idempotency check (by message_id metadata if present)
                            if let Some(msg_id) = inbound.metadata.get("message_id") {
                                let key = format!("{}:{}", inbound.channel, msg_id);
                                if !self.idempotency.check_and_record(&key) {
                                    debug!(
                                        sender = %inbound.sender_id,
                                        msg_id = %msg_id,
                                        "Skipping duplicate message"
                                    );
                                    continue;
                                }
                            }

                            // Device pairing check: if enabled, validate bearer token
                            if let Some(ref pairing_mutex) = self.pairing {
                                let identifier = inbound.sender_id.clone();
                                let token = inbound.metadata.get("auth_token").cloned();
                                let valid = match token {
                                    Some(raw_token) => {
                                        match pairing_mutex.lock() {
                                            Ok(mut mgr) => mgr.validate_token(&raw_token, &identifier).is_some(),
                                            Err(_) => false,
                                        }
                                    }
                                    None => false,
                                };
                                if !valid {
                                    warn!(
                                        sender = %inbound.sender_id,
                                        channel = %inbound.channel,
                                        "Rejected unpaired device (pairing enabled)"
                                    );
                                    let rejection = OutboundMessage::new(
                                        &inbound.channel,
                                        &inbound.chat_id,
                                        "Access denied: device not paired. Use `zeptoclaw pair new` to generate a pairing code.",
                                    );
                                    if let Err(e) = self.bus.publish_outbound(rejection).await {
                                        error!("Failed to publish pairing rejection: {}", e);
                                    }
                                    continue;
                                }
                            }

                            let permit = self.semaphore.clone().acquire_owned().await;
                            match permit {
                                Ok(permit) => {
                                    let proxy = Arc::clone(&self);
                                    tokio::spawn(async move {
                                        let response = proxy.process_in_container(&inbound).await;
                                        if let Err(e) = proxy.bus.publish_outbound(response).await {
                                            error!("Failed to publish response: {}", e);
                                        }
                                        drop(permit);
                                    });
                                }
                                Err(_) => {
                                    error!("Concurrency semaphore closed unexpectedly");
                                    break;
                                }
                            }
                        }
                        None => {
                            error!("Inbound channel closed");
                            break;
                        }
                    }
                }
            }
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Stop the proxy loop.
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Check if the proxy is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Process a message in a container.
    async fn process_in_container(&self, message: &InboundMessage) -> OutboundMessage {
        let usage_metrics = self
            .usage_metrics
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        if let Some(metrics) = usage_metrics.as_ref() {
            metrics.record_request();
        }

        let request_id = Uuid::new_v4().to_string();
        let session_snapshot = self.load_session_snapshot(&message.session_key).await;

        let request = AgentRequest {
            request_id: request_id.clone(),
            message: message.clone(),
            agent_config: self.config.agents.defaults.clone(),
            session: session_snapshot,
        };

        match self.spawn_container(&request).await {
            Ok(response) => match response.result {
                AgentResult::Success { content, session } => {
                    self.persist_session_snapshot(&message.session_key, session)
                        .await;
                    OutboundMessage::new(&message.channel, &message.chat_id, &content)
                }
                AgentResult::Error { message: err, .. } => {
                    if let Some(metrics) = usage_metrics.as_ref() {
                        metrics.record_error();
                    }
                    OutboundMessage::new(
                        &message.channel,
                        &message.chat_id,
                        &format!("Error: {}", err),
                    )
                }
            },
            Err(e) => {
                error!("Container execution failed: {}", e);
                if let Some(metrics) = usage_metrics.as_ref() {
                    metrics.record_error();
                }
                OutboundMessage::new(
                    &message.channel,
                    &message.chat_id,
                    &format!("Container error: {}", e),
                )
            }
        }
    }

    async fn load_session_snapshot(&self, session_key: &str) -> Option<crate::session::Session> {
        let manager = self.session_manager.as_ref()?;

        match manager.get(session_key).await {
            Ok(session) => session,
            Err(e) => {
                warn!("Failed to load session snapshot for {}: {}", session_key, e);
                None
            }
        }
    }

    async fn persist_session_snapshot(
        &self,
        expected_session_key: &str,
        session: Option<crate::session::Session>,
    ) {
        let Some(session) = session else {
            return;
        };

        if session.key != expected_session_key {
            warn!(
                expected = %expected_session_key,
                actual = %session.key,
                "Ignoring container session snapshot with mismatched key"
            );
            return;
        }

        let Some(manager) = self.session_manager.as_ref() else {
            return;
        };

        if let Err(e) = manager.save(&session).await {
            warn!(
                session = %expected_session_key,
                "Failed to persist container session snapshot: {}",
                e
            );
        }
    }

    /// Spawn a container and communicate via stdin/stdout.
    async fn spawn_container(&self, request: &AgentRequest) -> Result<AgentResponse> {
        let config_root = dirs::home_dir().unwrap_or_default().join(".zeptoclaw");
        let workspace_dir = config_root.join("workspace");
        let sessions_dir = config_root.join("sessions");
        let config_path = config_root.join("config.json");

        tokio::fs::create_dir_all(&workspace_dir)
            .await
            .map_err(|e| ZeptoError::Config(format!("Failed to create workspace dir: {}", e)))?;
        tokio::fs::create_dir_all(&sessions_dir)
            .await
            .map_err(|e| ZeptoError::Config(format!("Failed to create sessions dir: {}", e)))?;
        tokio::fs::create_dir_all(&config_root)
            .await
            .map_err(|e| ZeptoError::Config(format!("Failed to create config dir: {}", e)))?;

        let invocation = match self.resolved_backend {
            ResolvedBackend::Docker => {
                self.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path)?
            }
            #[cfg(target_os = "macos")]
            ResolvedBackend::Apple => {
                self.build_apple_invocation(&workspace_dir, &sessions_dir, &config_path)
                    .await?
            }
        };

        debug!(
            request_id = %request.request_id,
            backend = %self.resolved_backend,
            image = %self.container_config.image,
            args_len = invocation.args.len(),
            env_len = invocation.env.len(),
            "Spawning containerized agent request"
        );

        let mut command = Command::new(&invocation.binary);
        command.args(&invocation.args);
        for (name, value) in &invocation.env {
            command.env(name, value);
        }

        let result = self.run_container_process(&mut command, request).await;

        // Clean up temp dir (Apple Container env file) regardless of outcome.
        if let Some(ref temp_dir) = invocation.temp_dir {
            if let Err(e) = tokio::fs::remove_dir_all(temp_dir).await {
                warn!("Failed to clean up temp env dir {:?}: {}", temp_dir, e);
            }
        }

        result
    }

    /// Run the container process, write request to stdin, and parse output.
    async fn run_container_process(
        &self,
        command: &mut Command,
        request: &AgentRequest,
    ) -> Result<AgentResponse> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ZeptoError::Config(format!("Failed to spawn container: {}", e)))?;

        // Write request to stdin
        let request_json = serde_json::to_string(request)
            .map_err(|e| ZeptoError::Config(format!("Failed to serialize request: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| ZeptoError::Config(format!("Failed to write to stdin: {}", e)))?;
            stdin.write_all(b"\n").await?;
            stdin.shutdown().await?;
        }

        // Wait for output with timeout.
        //
        // On timeout the inner future (and the `Child`) is dropped. On Unix,
        // dropping a `tokio::process::Child` sends SIGKILL if the process is
        // still running, so the container process IS cleaned up.  We log a
        // warning here to make this implicit behaviour visible in traces.
        let timeout_duration = Duration::from_secs(self.container_config.timeout_secs);
        let output = tokio::time::timeout(timeout_duration, child.wait_with_output())
            .await
            .map_err(|_| {
                warn!(
                    timeout_secs = self.container_config.timeout_secs,
                    "Container process timed out; child will be killed on drop (SIGKILL)"
                );
                ZeptoError::Config(format!(
                    "Container timeout after {}s: process killed",
                    self.container_config.timeout_secs
                ))
            })?
            .map_err(|e| ZeptoError::Config(format!("Container failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ZeptoError::Config(format!(
                "Container exited with code {:?}: {}",
                output.status.code(),
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_marked_response(&stdout)
            .ok_or_else(|| ZeptoError::Config("Failed to parse container response".into()))
    }

    /// Collect env var pairs to pass into the container.
    fn collect_env_vars(&self) -> Vec<(String, String)> {
        let mut env_vars = Vec::new();

        // Provider API keys
        if let Some(ref anthropic) = self.config.providers.anthropic {
            if let Some(ref key) = anthropic.api_key {
                if !key.trim().is_empty() {
                    env_vars.push((
                        "ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY".to_string(),
                        key.clone(),
                    ));
                }
            }
            if let Some(ref base) = anthropic.api_base {
                if !base.trim().is_empty() {
                    env_vars.push((
                        "ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_BASE".to_string(),
                        base.clone(),
                    ));
                }
            }
        }
        if let Some(ref openai) = self.config.providers.openai {
            if let Some(ref key) = openai.api_key {
                if !key.trim().is_empty() {
                    env_vars.push((
                        "ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY".to_string(),
                        key.clone(),
                    ));
                }
            }
            if let Some(ref base) = openai.api_base {
                if !base.trim().is_empty() {
                    env_vars.push((
                        "ZEPTOCLAW_PROVIDERS_OPENAI_API_BASE".to_string(),
                        base.clone(),
                    ));
                }
            }
        }
        if let Some(ref openrouter) = self.config.providers.openrouter {
            if let Some(ref key) = openrouter.api_key {
                if !key.trim().is_empty() {
                    env_vars.push((
                        "ZEPTOCLAW_PROVIDERS_OPENROUTER_API_KEY".to_string(),
                        key.clone(),
                    ));
                }
            }
            if let Some(ref base) = openrouter.api_base {
                if !base.trim().is_empty() {
                    env_vars.push((
                        "ZEPTOCLAW_PROVIDERS_OPENROUTER_API_BASE".to_string(),
                        base.clone(),
                    ));
                }
            }
        }

        // Container-internal paths
        env_vars.push(("HOME".to_string(), "/data".to_string()));
        env_vars.push((
            "ZEPTOCLAW_AGENTS_DEFAULTS_WORKSPACE".to_string(),
            CONTAINER_WORKSPACE_DIR.to_string(),
        ));

        env_vars
    }

    /// Build Docker invocation arguments.
    fn build_docker_invocation(
        &self,
        workspace_dir: &Path,
        sessions_dir: &Path,
        config_path: &Path,
    ) -> Result<ContainerInvocation> {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-i".to_string(),
            "--network".to_string(),
            self.container_config.network.clone(),
        ];
        let env_vars = self.collect_env_vars();

        // Resource limits
        if let Some(ref mem) = self.container_config.memory_limit {
            args.push("--memory".to_string());
            args.push(mem.clone());
        }
        if let Some(ref cpu) = self.container_config.cpu_limit {
            args.push("--cpus".to_string());
            args.push(cpu.clone());
        }

        // Volume mounts
        args.push("-v".to_string());
        args.push(format!(
            "{}:{}",
            workspace_dir.display(),
            CONTAINER_WORKSPACE_DIR
        ));
        args.push("-v".to_string());
        args.push(format!(
            "{}:{}",
            sessions_dir.display(),
            CONTAINER_SESSIONS_DIR
        ));
        if config_path.exists() {
            args.push("-v".to_string());
            args.push(format!(
                "{}:{}:ro",
                config_path.display(),
                CONTAINER_CONFIG_PATH
            ));
        }

        // Environment variables — Docker uses `-e NAME` with process env for secrets
        let mut process_env = Vec::new();
        for (name, value) in &env_vars {
            args.push("-e".to_string());
            args.push(name.clone());
            process_env.push((name.clone(), value.clone()));
        }

        // Extra mounts from config — validate against blocked patterns first.
        for mount in &self.container_config.extra_mounts {
            validate_mount_not_blocked(mount)?;
            args.push("-v".to_string());
            args.push(mount.clone());
        }

        // Image and command
        args.push(self.container_config.image.clone());
        args.push("zeptoclaw".to_string());
        args.push("agent-stdin".to_string());

        let binary = validate_docker_binary(&self.container_config)?;

        Ok(ContainerInvocation {
            binary,
            args,
            env: process_env,
            temp_dir: None,
        })
    }

    /// Build Apple Container invocation arguments (macOS only).
    ///
    /// Key differences from Docker:
    /// - Binary: `container` not `docker`
    /// - RO mounts: `--mount type=bind,source=X,target=Y,readonly` (not `-v X:Y:ro`)
    /// - Env vars: `-e` flag is broken, use env file mount workaround
    /// - No `--memory`, `--cpus`, or `--network` flags
    /// - Needs explicit `--name` for container naming
    #[cfg(target_os = "macos")]
    async fn build_apple_invocation(
        &self,
        workspace_dir: &Path,
        sessions_dir: &Path,
        config_path: &Path,
    ) -> Result<ContainerInvocation> {
        let container_name = format!("zeptoclaw-{}", Uuid::new_v4());
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-i".to_string(),
            "--name".to_string(),
            container_name,
        ];

        // Volume mounts — RW mounts use -v, RO mounts use --mount with readonly
        args.push("-v".to_string());
        args.push(format!(
            "{}:{}",
            workspace_dir.display(),
            CONTAINER_WORKSPACE_DIR
        ));
        args.push("-v".to_string());
        args.push(format!(
            "{}:{}",
            sessions_dir.display(),
            CONTAINER_SESSIONS_DIR
        ));
        if config_path.exists() {
            args.push("--mount".to_string());
            args.push(format!(
                "type=bind,source={},target={},readonly",
                config_path.display(),
                CONTAINER_CONFIG_PATH
            ));
        }

        // Extra mounts from config — validate against blocked patterns first.
        for mount in &self.container_config.extra_mounts {
            validate_mount_not_blocked(mount)?;
            args.push("-v".to_string());
            args.push(mount.clone());
        }

        // Env file workaround: Apple Container's -e flag is broken, so we write
        // env vars to a shell file, mount it read-only, and source it before exec.
        let env_vars = self.collect_env_vars();
        let temp_dir = tempfile::tempdir()
            .map_err(|e| ZeptoError::Config(format!("Failed to create temp dir for env: {}", e)))?;
        let env_file_path = temp_dir.path().join("env.sh");
        let env_content = generate_env_file_content(&env_vars);
        tokio::fs::write(&env_file_path, &env_content)
            .await
            .map_err(|e| ZeptoError::Config(format!("Failed to write env file: {}", e)))?;

        // Mount env dir read-only
        args.push("--mount".to_string());
        args.push(format!(
            "type=bind,source={},target={},readonly",
            temp_dir.path().display(),
            CONTAINER_ENV_DIR
        ));

        // Image
        args.push(self.container_config.image.clone());

        // Wrap command: source env file then exec zeptoclaw
        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(format!(
            ". {}/env.sh && exec zeptoclaw agent-stdin",
            CONTAINER_ENV_DIR
        ));

        // Keep temp_dir alive — `keep` prevents automatic cleanup on drop.
        let temp_path = temp_dir.keep();

        Ok(ContainerInvocation {
            binary: "container".to_string(),
            args,
            env: Vec::new(), // Env is passed via file mount, not process env
            temp_dir: Some(temp_path),
        })
    }
}

/// Generate shell-sourceable env file content.
///
/// Each variable is exported via `export NAME='VALUE'` with single quotes
/// escaped so that values containing special characters work correctly.
pub fn generate_env_file_content(env_vars: &[(String, String)]) -> String {
    let mut lines = Vec::with_capacity(env_vars.len() + 1);
    lines.push("#!/bin/sh".to_string());
    for (name, value) in env_vars {
        // Escape single quotes: replace ' with '\''
        let escaped = value.replace('\'', "'\\''");
        lines.push(format!("export {}='{}'", name, escaped));
    }
    lines.push(String::new()); // trailing newline
    lines.join("\n")
}

/// Resolve the container backend from config, performing auto-detection.
pub async fn resolve_backend(config: &ContainerAgentConfig) -> Result<ResolvedBackend> {
    match config.backend {
        ContainerAgentBackend::Docker => Ok(ResolvedBackend::Docker),
        #[cfg(target_os = "macos")]
        ContainerAgentBackend::Apple => Ok(ResolvedBackend::Apple),
        ContainerAgentBackend::Auto => auto_detect_backend(config).await,
    }
}

/// Auto-detect: on macOS try Apple Container first, then Docker.
async fn auto_detect_backend(config: &ContainerAgentConfig) -> Result<ResolvedBackend> {
    #[cfg(target_os = "macos")]
    {
        if is_apple_container_available().await {
            return Ok(ResolvedBackend::Apple);
        }
    }

    if is_docker_available_with_binary(configured_docker_binary_raw(config)).await {
        return Ok(ResolvedBackend::Docker);
    }

    Err(ZeptoError::Config(
        "No container backend available. Install Docker or Apple Container (macOS 15+).".into(),
    ))
}

/// Check if Docker is available and the daemon is running.
pub async fn is_docker_available() -> bool {
    is_docker_available_with_binary("docker").await
}

/// Check if a specific Docker binary is available and the daemon is running.
pub async fn is_docker_available_with_binary(binary: &str) -> bool {
    let binary = binary.trim();
    if binary.is_empty() {
        return false;
    }

    tokio::process::Command::new(binary)
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Well-known Docker-compatible binary names that are accepted without path
/// validation.
const ALLOWED_DOCKER_BINARIES: &[&str] = &["docker", "podman"];

/// Resolve and validate the Docker binary from configuration.
///
/// Accepts:
/// - `None` / empty / whitespace-only -> defaults to `"docker"`
/// - A well-known name: `"docker"` or `"podman"`
/// - An absolute path that exists and is **not** inside a temp directory
///
/// Rejects everything else with a `SecurityViolation`.
fn validate_docker_binary(config: &ContainerAgentConfig) -> Result<String> {
    let raw = config
        .docker_binary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let binary = match raw {
        None => return Ok("docker".to_string()),
        Some(b) => b,
    };

    // Allow well-known names without further checks.
    if ALLOWED_DOCKER_BINARIES.contains(&binary) {
        return Ok(binary.to_string());
    }

    // Must be an absolute path.
    let path = Path::new(binary);
    if !path.is_absolute() {
        return Err(ZeptoError::SecurityViolation(format!(
            "docker_binary '{}' must be an absolute path or one of {:?}",
            binary, ALLOWED_DOCKER_BINARIES
        )));
    }

    // Must exist on disk.
    if !path.exists() {
        return Err(ZeptoError::SecurityViolation(format!(
            "docker_binary '{}' does not exist",
            binary
        )));
    }

    // Block temp directories to prevent untrusted binaries.
    let temp_prefixes: &[&str] = &["/tmp", "/var/tmp"];
    #[cfg(target_os = "macos")]
    let temp_prefixes_extra: &[&str] = &["/private/tmp", "/private/var/tmp"];
    #[cfg(not(target_os = "macos"))]
    let temp_prefixes_extra: &[&str] = &[];

    let lowered = binary.to_lowercase();
    for prefix in temp_prefixes.iter().chain(temp_prefixes_extra.iter()) {
        if lowered.starts_with(prefix) {
            return Err(ZeptoError::SecurityViolation(format!(
                "docker_binary '{}' is in a temporary directory; this is not allowed",
                binary
            )));
        }
    }

    warn!(
        docker_binary = binary,
        "Using non-default Docker binary from configuration"
    );

    Ok(binary.to_string())
}

/// Return the configured docker binary **without** validation.
///
/// This is only used in contexts where the caller needs the raw value for
/// probing (e.g. auto-detection).  The spawning code path always uses
/// [`validate_docker_binary`] instead.
fn configured_docker_binary_raw(config: &ContainerAgentConfig) -> &str {
    config
        .docker_binary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("docker")
}

/// Check if Apple Container CLI is available (macOS only).
#[cfg(target_os = "macos")]
pub async fn is_apple_container_available() -> bool {
    // Check that the `container` binary exists and responds to --version
    let version_ok = tokio::process::Command::new("container")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if !version_ok {
        return false;
    }

    // Also verify that `container run` is available via --help
    tokio::process::Command::new("container")
        .args(["run", "--help"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use tokio::time::{sleep, timeout};

    #[test]
    fn test_container_agent_proxy_creation() {
        let config = Config::default();
        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        assert!(!proxy.is_running());
        assert_eq!(proxy.backend(), ResolvedBackend::Docker);
    }

    #[test]
    fn test_build_docker_invocation_mounts_expected_paths_and_hides_secrets() {
        let mut config = Config::default();
        config.container_agent.image = "zeptoclaw:test".to_string();
        config.providers.anthropic = Some(ProviderConfig {
            api_key: Some("secret-anthropic-key".to_string()),
            ..Default::default()
        });

        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root =
            std::env::temp_dir().join(format!("zeptoclaw-proxy-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(&config_path, "{}").unwrap();

        let invocation = proxy
            .build_docker_invocation(&workspace_dir, &sessions_dir, &config_path)
            .expect("build_docker_invocation should succeed with default binary");

        assert_eq!(invocation.binary, "docker");

        let workspace_mount = format!("{}:{}", workspace_dir.display(), CONTAINER_WORKSPACE_DIR);
        let sessions_mount = format!("{}:{}", sessions_dir.display(), CONTAINER_SESSIONS_DIR);
        let config_mount = format!("{}:{}:ro", config_path.display(), CONTAINER_CONFIG_PATH);

        assert!(has_arg_pair(&invocation.args, "-v", &workspace_mount));
        assert!(has_arg_pair(&invocation.args, "-v", &sessions_mount));
        assert!(has_arg_pair(&invocation.args, "-v", &config_mount));
        assert!(has_arg_pair(
            &invocation.args,
            "-e",
            "ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY"
        ));
        assert!(!invocation
            .args
            .iter()
            .any(|arg| arg.contains("secret-anthropic-key")));
        assert!(invocation.env.iter().any(|(name, value)| {
            name == "ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY" && value == "secret-anthropic-key"
        }));

        assert!(invocation.temp_dir.is_none());

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[tokio::test]
    async fn test_stop_unblocks_start_loop() {
        let config = Config::default();
        let bus = Arc::new(MessageBus::new());
        let proxy = Arc::new(ContainerAgentProxy::new(
            config,
            bus,
            ResolvedBackend::Docker,
        ));

        let proxy_task = Arc::clone(&proxy);
        let handle = tokio::spawn(async move { proxy_task.start().await });

        for _ in 0..50 {
            if proxy.is_running() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }

        proxy.stop();
        let joined = timeout(Duration::from_secs(2), handle)
            .await
            .expect("proxy task should stop");
        joined
            .expect("proxy task should join")
            .expect("proxy start should exit cleanly");
        assert!(!proxy.is_running());
    }

    #[test]
    fn test_generate_env_file_content_basic() {
        let vars = vec![
            ("FOO".to_string(), "bar".to_string()),
            ("KEY".to_string(), "value with spaces".to_string()),
        ];
        let content = generate_env_file_content(&vars);
        assert!(content.starts_with("#!/bin/sh\n"));
        assert!(content.contains("export FOO='bar'"));
        assert!(content.contains("export KEY='value with spaces'"));
    }

    #[test]
    fn test_generate_env_file_content_special_chars() {
        let vars = vec![
            (
                "QUOTED".to_string(),
                "it's a \"test\" with $var".to_string(),
            ),
            ("EMPTY".to_string(), String::new()),
        ];
        let content = generate_env_file_content(&vars);
        // Single quotes inside value should be escaped
        assert!(content.contains("export QUOTED='it'\\''s a \"test\" with $var'"));
        assert!(content.contains("export EMPTY=''"));
    }

    #[test]
    fn test_collect_env_vars_includes_internal_paths() {
        let config = Config::default();
        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let vars = proxy.collect_env_vars();
        assert!(vars.iter().any(|(k, v)| k == "HOME" && v == "/data"));
        assert!(vars.iter().any(
            |(k, v)| k == "ZEPTOCLAW_AGENTS_DEFAULTS_WORKSPACE" && v == CONTAINER_WORKSPACE_DIR
        ));
    }

    #[test]
    fn test_build_docker_invocation_rejects_temp_binary() {
        let mut config = Config::default();
        config.container_agent.docker_binary = Some("/tmp/mock-docker".to_string());
        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root =
            std::env::temp_dir().join(format!("zeptoclaw-binary-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(&config_path, "{}").unwrap();

        let result = proxy.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("temporary directory") || err_msg.contains("does not exist"),
            "Expected temp dir or missing file error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_build_docker_invocation_accepts_well_known_binaries() {
        // "docker" is the default and should always work
        let config = Config::default();
        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root = std::env::temp_dir().join(format!("zeptoclaw-wk-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(&config_path, "{}").unwrap();

        let invocation = proxy
            .build_docker_invocation(&workspace_dir, &sessions_dir, &config_path)
            .expect("default 'docker' binary should be accepted");
        assert_eq!(invocation.binary, "docker");

        // "podman" should also be accepted
        let mut config2 = Config::default();
        config2.container_agent.docker_binary = Some("podman".to_string());
        let bus2 = Arc::new(MessageBus::new());
        let proxy2 = ContainerAgentProxy::new(config2, bus2, ResolvedBackend::Docker);

        let invocation2 = proxy2
            .build_docker_invocation(&workspace_dir, &sessions_dir, &config_path)
            .expect("'podman' binary should be accepted");
        assert_eq!(invocation2.binary, "podman");

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_validate_docker_binary_rejects_relative_path() {
        let mut config = ContainerAgentConfig::default();
        config.docker_binary = Some("./my-docker".to_string());
        let result = validate_docker_binary(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("absolute path"));
    }

    #[test]
    fn test_validate_docker_binary_rejects_nonexistent_absolute_path() {
        let mut config = ContainerAgentConfig::default();
        config.docker_binary = Some("/usr/local/bin/nonexistent-docker-zzz".to_string());
        let result = validate_docker_binary(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_validate_docker_binary_defaults_to_docker_when_empty() {
        let mut config = ContainerAgentConfig::default();

        // None
        config.docker_binary = None;
        assert_eq!(validate_docker_binary(&config).unwrap(), "docker");

        // Empty string
        config.docker_binary = Some(String::new());
        assert_eq!(validate_docker_binary(&config).unwrap(), "docker");

        // Whitespace only
        config.docker_binary = Some("   ".to_string());
        assert_eq!(validate_docker_binary(&config).unwrap(), "docker");
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn test_resolve_backend_auto_respects_docker_binary_override() {
        let mut config = ContainerAgentConfig::default();
        config.backend = ContainerAgentBackend::Auto;
        config.docker_binary = Some("/definitely-not-a-real-docker-binary".to_string());

        let result = resolve_backend(&config).await;
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_proxy_end_to_end_with_mocked_docker_binary() {
        use std::os::unix::fs::PermissionsExt;

        // Place mock binary under the project target directory (not /tmp) so it
        // passes the validate_docker_binary temp-directory check.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mock_dir = std::path::PathBuf::from(manifest_dir)
            .join("target")
            .join("test-mocks");
        std::fs::create_dir_all(&mock_dir).unwrap();
        let script_path = mock_dir.join(format!("mock-docker-{}.sh", Uuid::new_v4()));

        let chat_id = format!("chat-{}", Uuid::new_v4());
        let session_key = format!("test:{}", chat_id);
        let session_json = format!(
            r#"{{"request_id":"mock-req","result":{{"Success":{{"content":"mock response","session":{{"key":"{}","messages":[],"summary":null,"created_at":"2026-02-13T00:00:00Z","updated_at":"2026-02-13T00:00:00Z"}}}}}}}}"#,
            session_key
        );
        let script = format!(
            r#"#!/bin/sh
cat >/dev/null
cat <<'EOF'
<<<AGENT_RESPONSE_START>>>
{}
<<<AGENT_RESPONSE_END>>>
EOF
"#,
            session_json
        );
        std::fs::write(&script_path, script).unwrap();
        let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).unwrap();

        let mut config = Config::default();
        config.container_agent.image = "mock-image:latest".to_string();
        config.container_agent.timeout_secs = 5;
        config.container_agent.docker_binary = Some(script_path.to_string_lossy().to_string());

        let bus = Arc::new(MessageBus::new());
        let proxy = Arc::new(ContainerAgentProxy::new(
            config,
            bus.clone(),
            ResolvedBackend::Docker,
        ));

        let proxy_task = Arc::clone(&proxy);
        let handle = tokio::spawn(async move { proxy_task.start().await });

        let inbound = InboundMessage::new("test", "u1", &chat_id, "hello");
        bus.publish_inbound(inbound).await.unwrap();

        let outbound = timeout(Duration::from_secs(2), bus.consume_outbound())
            .await
            .expect("should receive outbound within timeout")
            .expect("outbound should be present");
        assert_eq!(outbound.channel, "test");
        assert_eq!(outbound.chat_id, chat_id);
        assert_eq!(outbound.content, "mock response");
        let saved_session = proxy
            .load_session_snapshot(&session_key)
            .await
            .expect("session snapshot should be persisted");
        assert_eq!(saved_session.key, session_key);
        if let Some(manager) = proxy.session_manager.as_ref() {
            let _ = manager.delete(&session_key).await;
        }

        proxy.stop();
        timeout(Duration::from_secs(2), handle)
            .await
            .expect("proxy should stop quickly")
            .expect("proxy task join should succeed")
            .expect("proxy start should return ok");

        // Clean up mock script
        let _ = std::fs::remove_file(&script_path);
    }

    #[test]
    fn test_container_agent_backend_serde_roundtrip() {
        // Auto
        let json = serde_json::to_string(&ContainerAgentBackend::Auto).unwrap();
        assert_eq!(json, "\"auto\"");
        let back: ContainerAgentBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ContainerAgentBackend::Auto);

        // Docker
        let json = serde_json::to_string(&ContainerAgentBackend::Docker).unwrap();
        assert_eq!(json, "\"docker\"");
        let back: ContainerAgentBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ContainerAgentBackend::Docker);

        // Apple (macOS only)
        #[cfg(target_os = "macos")]
        {
            let json = serde_json::to_string(&ContainerAgentBackend::Apple).unwrap();
            assert_eq!(json, "\"apple\"");
            let back: ContainerAgentBackend = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ContainerAgentBackend::Apple);
        }
    }

    // -----------------------------------------------------------------------
    // Issue 2: extra_mounts blocked-pattern validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_docker_invocation_rejects_sensitive_extra_mount() {
        let mut config = Config::default();
        config.container_agent.extra_mounts = vec!["/home/user/.ssh:/container/.ssh".to_string()];

        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root =
            std::env::temp_dir().join(format!("zeptoclaw-mount-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let result = proxy.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(".ssh"),
            "Expected .ssh blocked pattern in error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_build_docker_invocation_rejects_env_file_mount() {
        let mut config = Config::default();
        config.container_agent.extra_mounts = vec!["/app/.env:/container/.env".to_string()];

        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root = std::env::temp_dir().join(format!("zeptoclaw-env-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let result = proxy.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(".env"),
            "Expected .env blocked pattern in error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_build_docker_invocation_rejects_aws_credentials_mount() {
        let mut config = Config::default();
        config.container_agent.extra_mounts = vec!["/home/user/.aws:/container/aws:ro".to_string()];

        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root = std::env::temp_dir().join(format!("zeptoclaw-aws-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let result = proxy.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(".aws"),
            "Expected .aws blocked pattern in error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_build_docker_invocation_rejects_traversal_in_extra_mount() {
        let mut config = Config::default();
        config.container_agent.extra_mounts =
            vec!["/home/user/../etc/passwd:/container/passwd".to_string()];

        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root =
            std::env::temp_dir().join(format!("zeptoclaw-trav-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let result = proxy.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("traversal"),
            "Expected path traversal error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_build_docker_invocation_accepts_safe_extra_mount() {
        // Create a real directory to mount — no blocked patterns in path
        let mount_root =
            std::env::temp_dir().join(format!("zeptoclaw-safe-mount-{}", Uuid::new_v4()));
        let safe_dir = mount_root.join("project-data");
        std::fs::create_dir_all(&safe_dir).unwrap();

        let mut config = Config::default();
        config.container_agent.extra_mounts =
            vec![format!("{}:/container/data", safe_dir.display())];

        let bus = Arc::new(MessageBus::new());
        let proxy = ContainerAgentProxy::new(config, bus, ResolvedBackend::Docker);

        let temp_root =
            std::env::temp_dir().join(format!("zeptoclaw-safe-test-{}", Uuid::new_v4()));
        let workspace_dir = temp_root.join("workspace");
        let sessions_dir = temp_root.join("sessions");
        let config_path = temp_root.join("config.json");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let result = proxy.build_docker_invocation(&workspace_dir, &sessions_dir, &config_path);
        assert!(result.is_ok(), "Safe mount should be accepted");
        let invocation = result.unwrap();
        assert!(
            invocation
                .args
                .iter()
                .any(|a| a.contains("/container/data")),
            "Extra mount should appear in args"
        );

        let _ = std::fs::remove_dir_all(&temp_root);
        let _ = std::fs::remove_dir_all(&mount_root);
    }

    // -----------------------------------------------------------------------
    // validate_mount_not_blocked unit tests (from security::mount)
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_mount_not_blocked_rejects_private_key() {
        let result = validate_mount_not_blocked("/home/user/private_key:/container/key");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("private_key"));
    }

    #[test]
    fn test_validate_mount_not_blocked_rejects_docker_dir() {
        let result = validate_mount_not_blocked("/home/user/.docker:/container/docker");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".docker"));
    }

    #[test]
    fn test_validate_mount_not_blocked_rejects_invalid_container_path() {
        let result = validate_mount_not_blocked("/home/user/data:relative/path");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid container"));
    }

    #[test]
    fn test_validate_mount_not_blocked_rejects_container_path_traversal() {
        let result = validate_mount_not_blocked("/home/user/data:/container/../etc");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid container"));
    }

    fn has_arg_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2)
            .any(|window| window[0] == flag && window[1] == value)
    }
}
