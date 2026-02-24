//! HTTP health server for ZeptoClaw.
//!
//! Exposes `/health` (liveness) and `/ready` (readiness) endpoints.
//! Components register named checks via [`HealthRegistry`].
//!
//! Also provides:
//! - [`UsageMetrics`] for lock-free per-request counters
//! - [`start_periodic_usage_flush`] for periodic metric emission
//! - [`health_port`] helper for legacy env-only port resolution
//!
//! Uses raw TCP + manual HTTP to avoid adding a web framework dependency,
//! preserving the ultra-light binary footprint (4MB design goal).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tracing::{info, warn};

// ============================================================================
// Default health check port
// ============================================================================

const DEFAULT_HEALTH_PORT: u16 = 9090;
const USAGE_FLUSH_INTERVAL_SECS: u64 = 60;

// ============================================================================
// Platform RSS helper
// ============================================================================

/// Return the current process RSS (Resident Set Size) in bytes, or `None`
/// on unsupported platforms.
///
/// - **macOS**: Uses `mach_task_self()` + `task_info()` FFI to read
///   `resident_size` from `MACH_TASK_BASIC_INFO` (flavor 20).
/// - **Linux**: Reads `/proc/self/statm`, parses the 2nd field (RSS pages),
///   and multiplies by the kernel page size via `sysconf(_SC_PAGESIZE)`.
/// - **Other**: Returns `None`.
///
/// No new crate dependencies are added; all FFI is declared inline.
pub fn get_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // mach/mach_types.h
        type MachPort = u32;
        type KernReturn = i32;
        type TaskFlavor = u32;
        type NaturalT = u32;

        // MACH_TASK_BASIC_INFO flavor (20) struct layout
        #[repr(C)]
        struct MachTaskBasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time_sec: i32,
            user_time_usec: i32,
            system_time_sec: i32,
            system_time_usec: i32,
            policy: i32,
            suspend_count: i32,
        }

        const MACH_TASK_BASIC_INFO: TaskFlavor = 20;
        const KERN_SUCCESS: KernReturn = 0;

        extern "C" {
            static mach_task_self_: MachPort;
            fn task_info(
                target_task: MachPort,
                flavor: TaskFlavor,
                task_info_out: *mut MachTaskBasicInfo,
                task_info_out_cnt: *mut NaturalT,
            ) -> KernReturn;
        }

        let mut info = MachTaskBasicInfo {
            virtual_size: 0,
            resident_size: 0,
            resident_size_max: 0,
            user_time_sec: 0,
            user_time_usec: 0,
            system_time_sec: 0,
            system_time_usec: 0,
            policy: 0,
            suspend_count: 0,
        };
        let mut count = (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<NaturalT>())
            as NaturalT;

        let ret =
            unsafe { task_info(mach_task_self_, MACH_TASK_BASIC_INFO, &mut info, &mut count) };

        if ret == KERN_SUCCESS {
            Some(info.resident_size)
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Read /proc/self/statm: fields are in pages.
        // Format: size resident shared text lib data dt
        // We want the 2nd field (resident pages).
        let content = std::fs::read_to_string("/proc/self/statm").ok()?;
        let resident_pages: u64 = content.split_whitespace().nth(1)?.parse().ok()?;

        extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        // _SC_PAGESIZE = 30 on Linux (Linux-specific value)
        const SC_PAGESIZE: i32 = 30;
        let page_size = unsafe { sysconf(SC_PAGESIZE) };
        if page_size <= 0 {
            return None;
        }
        Some(resident_pages * (page_size as u64))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

// ============================================================================
// HealthStatus
// ============================================================================

/// The status of a single named health component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    /// Component is operating normally.
    Ok,
    /// Component is partially degraded but still functional.
    Degraded,
    /// Component is fully unavailable.
    Down,
}

impl HealthStatus {
    fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Ok => "ok",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Down => "down",
        }
    }
}

// ============================================================================
// HealthCheck
// ============================================================================

/// A named health check entry managed by [`HealthRegistry`].
#[derive(Debug, Clone)]
pub struct HealthCheck {
    /// Unique name for this check (e.g. "telegram", "provider", "db").
    pub name: String,
    /// Current status of this check.
    pub status: HealthStatus,
    /// Optional human-readable status message.
    pub message: Option<String>,
    /// Number of times this component has been restarted.
    pub restart_count: u64,
    /// Last error message, if any.
    pub last_error: Option<String>,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: HealthStatus::Ok,
            message: None,
            restart_count: 0,
            last_error: None,
        }
    }
}

// ============================================================================
// HealthRegistry
// ============================================================================

/// Registry of named component health checks.
///
/// Components register themselves at startup and update their status
/// throughout the process lifetime. The registry drives `/ready` responses.
///
/// # Example
/// ```
/// use zeptoclaw::health::{HealthRegistry, HealthCheck, HealthStatus};
/// let registry = HealthRegistry::new();
/// registry.register(HealthCheck { name: "provider".into(), status: HealthStatus::Ok, ..Default::default() });
/// assert!(registry.is_ready());
/// ```
#[derive(Clone)]
pub struct HealthRegistry {
    checks: Arc<RwLock<HashMap<String, HealthCheck>>>,
    start_time: Instant,
    metrics: Arc<RwLock<Option<Arc<UsageMetrics>>>>,
}

impl HealthRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            checks: Arc::new(RwLock::new(HashMap::new())),
            start_time: Instant::now(),
            metrics: Arc::new(RwLock::new(None)),
        }
    }

    /// Attach a [`UsageMetrics`] instance for inclusion in health responses.
    pub fn set_metrics(&self, metrics: Arc<UsageMetrics>) {
        *self.metrics.write().unwrap() = Some(metrics);
    }

    /// Register a new named check. Replaces any existing check with the same name.
    pub fn register(&self, check: HealthCheck) {
        self.checks
            .write()
            .unwrap()
            .insert(check.name.clone(), check);
    }

    /// Update an existing check's status and message.
    ///
    /// No-op if no check with that name is registered.
    pub fn update(&self, name: &str, status: HealthStatus, message: Option<String>) {
        let mut checks = self.checks.write().unwrap();
        if let Some(check) = checks.get_mut(name) {
            check.status = status;
            check.message = message;
        }
    }

    /// Returns `true` when all registered checks are not [`HealthStatus::Down`].
    ///
    /// An empty registry is considered ready.
    pub fn is_ready(&self) -> bool {
        let checks = self.checks.read().unwrap();
        checks.values().all(|c| c.status != HealthStatus::Down)
    }

    /// Increment the restart counter for a named component.
    ///
    /// No-op if no check with that name is registered.
    pub fn bump_restart(&self, name: &str) {
        let mut checks = self.checks.write().unwrap();
        if let Some(check) = checks.get_mut(name) {
            check.restart_count += 1;
        }
    }

    /// Mark a component as Down with an error message, and record the last error.
    ///
    /// No-op if no check with that name is registered.
    pub fn set_error(&self, name: &str, error: &str) {
        let mut checks = self.checks.write().unwrap();
        if let Some(check) = checks.get_mut(name) {
            check.status = HealthStatus::Down;
            check.last_error = Some(error.to_string());
        }
    }

    /// Return a snapshot of all registered checks.
    pub fn all_checks(&self) -> Vec<HealthCheck> {
        self.checks.read().unwrap().values().cloned().collect()
    }

    /// Elapsed time since the registry was created (proxy for process uptime).
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Render all checks as a compact JSON object for `/health` responses.
    pub(crate) fn render_checks_json(&self) -> String {
        let checks = self.checks.read().unwrap();
        if checks.is_empty() {
            return "{}".to_string();
        }
        let parts: Vec<String> = checks
            .values()
            .map(|c| {
                let mut fields = format!("\"status\":\"{}\"", c.status.as_str());
                if let Some(ref msg) = c.message {
                    fields.push_str(&format!(",\"message\":\"{}\"", msg.replace('"', "\\\"")));
                }
                if c.restart_count > 0 {
                    fields.push_str(&format!(",\"restart_count\":{}", c.restart_count));
                }
                if let Some(ref err) = c.last_error {
                    fields.push_str(&format!(",\"last_error\":\"{}\"", err.replace('"', "\\\"")));
                }
                format!("\"{}\":{{{}}}", c.name, fields)
            })
            .collect();
        format!("{{{}}}", parts.join(","))
    }

    /// Render a rich health JSON response with status, version, uptime, memory, usage, and checks.
    pub fn render_health_json(&self) -> String {
        let status = if self.is_ready() { "ok" } else { "degraded" };
        let version = env!("CARGO_PKG_VERSION");
        let uptime = self.uptime().as_secs();
        let checks_json = self.render_checks_json();

        let mut json = format!(
            "{{\"status\":\"{}\",\"version\":\"{}\",\"uptime_secs\":{}",
            status, version, uptime
        );

        // memory section — only on supported platforms
        if let Some(rss) = get_rss_bytes() {
            let rss_mb = rss as f64 / (1024.0 * 1024.0);
            json.push_str(&format!(
                ",\"memory\":{{\"rss_bytes\":{},\"rss_mb\":{:.1}}}",
                rss, rss_mb
            ));
        }

        // usage section — only when metrics are attached
        if let Some(ref m) = *self.metrics.read().unwrap() {
            let requests = m.requests.load(Ordering::Relaxed);
            let tool_calls = m.tool_calls.load(Ordering::Relaxed);
            let input_tokens = m.input_tokens.load(Ordering::Relaxed);
            let output_tokens = m.output_tokens.load(Ordering::Relaxed);
            let errors = m.errors.load(Ordering::Relaxed);
            json.push_str(&format!(
                ",\"usage\":{{\"requests\":{},\"tool_calls\":{},\"input_tokens\":{},\"output_tokens\":{},\"errors\":{}}}",
                requests, tool_calls, input_tokens, output_tokens, errors
            ));
        }

        json.push_str(&format!(",\"checks\":{}}}", checks_json));
        json
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// UsageMetrics (retained from original for gateway wiring)
// ============================================================================

/// Lock-free per-request counters for gateway usage tracking.
#[derive(Debug)]
pub struct UsageMetrics {
    /// Total requests processed.
    pub requests: AtomicU64,
    /// Total tool calls executed.
    pub tool_calls: AtomicU64,
    /// Total input tokens consumed.
    pub input_tokens: AtomicU64,
    /// Total output tokens produced.
    pub output_tokens: AtomicU64,
    /// Total errors encountered.
    pub errors: AtomicU64,
    /// Whether the gateway is ready to accept requests.
    pub ready: AtomicBool,
}

impl UsageMetrics {
    /// Create zeroed counters with `ready = false`.
    pub fn new() -> Self {
        Self {
            requests: AtomicU64::new(0),
            tool_calls: AtomicU64::new(0),
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            ready: AtomicBool::new(false),
        }
    }

    /// Increment the request counter.
    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the tool call counter.
    pub fn record_tool_calls(&self, count: u64) {
        self.tool_calls.fetch_add(count, Ordering::Relaxed);
    }

    /// Record token usage from an LLM response.
    pub fn record_tokens(&self, input: u64, output: u64) {
        self.input_tokens.fetch_add(input, Ordering::Relaxed);
        self.output_tokens.fetch_add(output, Ordering::Relaxed);
    }

    /// Increment the error counter.
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Set the ready flag.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    /// Emit current counters as a structured log line.
    pub fn emit_usage(&self, reason: &str) {
        info!(
            event = "usage_summary",
            reason = reason,
            requests = self.requests.load(Ordering::Relaxed),
            tool_calls = self.tool_calls.load(Ordering::Relaxed),
            input_tokens = self.input_tokens.load(Ordering::Relaxed),
            output_tokens = self.output_tokens.load(Ordering::Relaxed),
            errors = self.errors.load(Ordering::Relaxed),
            "Usage metrics"
        );
    }
}

impl Default for UsageMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Health server (raw TCP, no axum — preserves binary size)
// ============================================================================

/// Start the HTTP health server.
///
/// Serves:
/// - `GET /health` → 200 with JSON body `{"status":"ok","uptime_secs":N,"checks":{...}}`
/// - `GET /ready`  → 200 if all checks are not Down, 503 otherwise
/// - `GET /healthz` → 200 OK (liveness alias, retained for backward compat)
/// - `GET /readyz`  → delegates to the same readiness logic (backward compat)
/// - Anything else → 404
///
/// Returns a `JoinHandle` so callers can abort on shutdown.
pub async fn start_health_server(
    host: &str,
    port: u16,
    registry: HealthRegistry,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("{}:{}", host, port);
    let listener = TcpListener::bind(&addr).await?;
    info!(addr = %addr, "Health server listening on http://{}", addr);

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _addr)) => {
                    let registry = registry.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 512];
                        let n = match tokio::time::timeout(
                            Duration::from_secs(5),
                            tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
                        )
                        .await
                        {
                            Ok(Ok(n)) => n,
                            _ => return,
                        };

                        let request = String::from_utf8_lossy(&buf[..n]);
                        let request_line = request.lines().next().unwrap_or_default();
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or_default();
                        let raw_path = parts.next().unwrap_or_default();
                        let path = raw_path.split('?').next().unwrap_or(raw_path);

                        let (status_line, body) = match (method, path) {
                            ("GET", "/health") | ("GET", "/healthz") => {
                                let body = registry.render_health_json();
                                ("200 OK", body)
                            }
                            ("GET", "/ready") | ("GET", "/readyz") => {
                                if registry.is_ready() {
                                    ("200 OK", "{\"status\":\"ready\"}".to_string())
                                } else {
                                    (
                                        "503 Service Unavailable",
                                        "{\"status\":\"not_ready\"}".to_string(),
                                    )
                                }
                            }
                            _ => ("404 Not Found", "{\"error\":\"not_found\"}".to_string()),
                        };

                        let response = format!(
                            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            status_line,
                            body.len(),
                            body
                        );

                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
                Err(e) => {
                    warn!(error = %e, "Health server accept error");
                }
            }
        }
    });

    Ok(handle)
}

// ============================================================================
// Legacy overload: start_health_server(port, metrics) for gateway wiring
// ============================================================================

/// Start the health server using legacy `(port, metrics)` signature.
///
/// Used by [`crate::cli::gateway`] which passes a `UsageMetrics` instead of
/// a `HealthRegistry`. The metrics `ready` flag drives `/readyz` readiness.
pub async fn start_health_server_legacy(
    port: u16,
    metrics: Arc<UsageMetrics>,
) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    info!(port = port, "Health server listening");

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _addr)) => {
                    let metrics = Arc::clone(&metrics);
                    tokio::spawn(async move {
                        let mut buf = [0u8; 512];
                        let n = match tokio::time::timeout(
                            Duration::from_secs(5),
                            tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
                        )
                        .await
                        {
                            Ok(Ok(n)) => n,
                            _ => return,
                        };
                        let request = String::from_utf8_lossy(&buf[..n]);
                        let request_line = request.lines().next().unwrap_or_default();
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or_default();
                        let raw_path = parts.next().unwrap_or_default();
                        let path = raw_path.split('?').next().unwrap_or(raw_path);

                        let (status, body): (&str, String) = match (method, path) {
                            ("GET", "/healthz") | ("GET", "/health") => {
                                let mut parts: Vec<String> = Vec::with_capacity(5);
                                let ready = metrics.ready.load(Ordering::SeqCst);
                                parts.push(format!(
                                    "\"status\":\"{}\"",
                                    if ready { "ok" } else { "degraded" }
                                ));
                                parts
                                    .push(format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION")));
                                if let Some(rss) = get_rss_bytes() {
                                    let mb = rss as f64 / 1_048_576.0;
                                    parts.push(format!(
                                        "\"memory\":{{\"rss_bytes\":{},\"rss_mb\":{:.1}}}",
                                        rss, mb
                                    ));
                                }
                                parts.push(format!(
                                    "\"usage\":{{\"requests\":{},\"tool_calls\":{},\"input_tokens\":{},\"output_tokens\":{},\"errors\":{}}}",
                                    metrics.requests.load(Ordering::Relaxed),
                                    metrics.tool_calls.load(Ordering::Relaxed),
                                    metrics.input_tokens.load(Ordering::Relaxed),
                                    metrics.output_tokens.load(Ordering::Relaxed),
                                    metrics.errors.load(Ordering::Relaxed),
                                ));
                                ("200 OK", format!("{{{}}}", parts.join(",")))
                            }
                            ("GET", "/readyz") | ("GET", "/ready") => {
                                if metrics.ready.load(Ordering::SeqCst) {
                                    ("200 OK", "{\"status\":\"ready\"}".to_string())
                                } else {
                                    (
                                        "503 Service Unavailable",
                                        "{\"status\":\"not_ready\"}".to_string(),
                                    )
                                }
                            }
                            _ => ("404 Not Found", "{\"error\":\"not_found\"}".to_string()),
                        };

                        let response = format!(
                            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            status,
                            body.len(),
                            body
                        );

                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
                Err(e) => {
                    warn!(error = %e, "Health server accept error");
                }
            }
        }
    });

    Ok(handle)
}

// ============================================================================
// Periodic usage flush
// ============================================================================

/// Start a background task that emits usage metrics every 60 seconds.
///
/// Emits a final `shutdown` summary when `shutdown_rx` signals `true`.
pub fn start_periodic_usage_flush(
    metrics: Arc<UsageMetrics>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(USAGE_FLUSH_INTERVAL_SECS));
        interval.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    metrics.emit_usage("periodic");
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        metrics.emit_usage("shutdown");
                        break;
                    }
                }
            }
        }
    })
}

// ============================================================================
// Legacy port helper
// ============================================================================

/// Resolve the health server port from `ZEPTOCLAW_HEALTH_PORT` env var,
/// falling back to the compiled-in default (9090).
pub fn health_port() -> u16 {
    std::env::var("ZEPTOCLAW_HEALTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_HEALTH_PORT)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- HealthRegistry tests ---

    #[test]
    fn test_registry_ready_when_empty() {
        let reg = HealthRegistry::new();
        assert!(reg.is_ready());
    }

    #[test]
    fn test_registry_not_ready_when_check_down() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "telegram".into(),
            status: HealthStatus::Down,
            message: None,
            ..Default::default()
        });
        assert!(!reg.is_ready());
    }

    #[test]
    fn test_registry_ready_when_all_ok() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "telegram".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });
        reg.register(HealthCheck {
            name: "provider".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });
        assert!(reg.is_ready());
    }

    #[test]
    fn test_registry_ready_with_degraded() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "web".into(),
            status: HealthStatus::Degraded,
            message: None,
            ..Default::default()
        });
        assert!(reg.is_ready()); // Degraded is not Down
    }

    #[test]
    fn test_update_check_status() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "db".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });
        reg.update("db", HealthStatus::Down, Some("connection refused".into()));
        assert!(!reg.is_ready());
    }

    #[test]
    fn test_update_nonexistent_noop() {
        let reg = HealthRegistry::new();
        // Should not panic or insert new entry
        reg.update("ghost", HealthStatus::Down, None);
        assert!(reg.is_ready());
    }

    #[test]
    fn test_uptime_increases() {
        let reg = HealthRegistry::new();
        std::thread::sleep(Duration::from_millis(10));
        assert!(reg.uptime().as_millis() >= 10);
    }

    #[test]
    fn test_render_checks_json_empty() {
        let reg = HealthRegistry::new();
        assert_eq!(reg.render_checks_json(), "{}");
    }

    #[test]
    fn test_render_checks_json_ok() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "db".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });
        let json = reg.render_checks_json();
        assert!(json.contains("\"db\""));
        assert!(json.contains("\"status\":\"ok\""));
    }

    #[test]
    fn test_render_checks_json_with_message() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "db".into(),
            status: HealthStatus::Down,
            message: Some("timeout".into()),
            ..Default::default()
        });
        let json = reg.render_checks_json();
        assert!(json.contains("\"message\":\"timeout\""));
    }

    #[test]
    fn test_render_checks_json_message_escapes_quotes() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "x".into(),
            status: HealthStatus::Ok,
            message: Some("say \"hi\"".into()),
            ..Default::default()
        });
        let json = reg.render_checks_json();
        assert!(json.contains("\\\"hi\\\""));
    }

    #[test]
    fn test_health_status_as_str() {
        assert_eq!(HealthStatus::Ok.as_str(), "ok");
        assert_eq!(HealthStatus::Degraded.as_str(), "degraded");
        assert_eq!(HealthStatus::Down.as_str(), "down");
    }

    // --- UsageMetrics tests ---

    #[test]
    fn test_usage_metrics_creation() {
        let metrics = UsageMetrics::new();
        assert_eq!(metrics.requests.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.tool_calls.load(Ordering::Relaxed), 0);
        assert!(!metrics.ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_usage_metrics_recording() {
        let metrics = UsageMetrics::new();
        metrics.record_request();
        metrics.record_request();
        metrics.record_tool_calls(3);
        metrics.record_tokens(100, 50);
        metrics.record_error();

        assert_eq!(metrics.requests.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.tool_calls.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.input_tokens.load(Ordering::Relaxed), 100);
        assert_eq!(metrics.output_tokens.load(Ordering::Relaxed), 50);
        assert_eq!(metrics.errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_ready_flag() {
        let metrics = UsageMetrics::new();
        assert!(!metrics.ready.load(Ordering::SeqCst));
        metrics.set_ready(true);
        assert!(metrics.ready.load(Ordering::SeqCst));
        metrics.set_ready(false);
        assert!(!metrics.ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_health_port_default() {
        std::env::remove_var("ZEPTOCLAW_HEALTH_PORT");
        assert_eq!(health_port(), DEFAULT_HEALTH_PORT);
    }

    #[test]
    fn test_registry_register_replaces_existing() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "svc".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });
        reg.register(HealthCheck {
            name: "svc".into(),
            status: HealthStatus::Down,
            message: Some("crashed".into()),
            ..Default::default()
        });
        assert!(!reg.is_ready());
    }

    // --- HTTP server integration tests ---

    #[tokio::test]
    async fn test_health_server_health_endpoint() {
        let registry = HealthRegistry::new();
        registry.register(HealthCheck {
            name: "provider".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server("127.0.0.1", port, registry)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("200 OK"), "response: {}", response);
        assert!(response.contains("\"status\":\"ok\""));
        assert!(response.contains("uptime_secs"));
        assert!(response.contains("\"provider\""));

        handle.abort();
    }

    #[tokio::test]
    async fn test_health_server_ready_endpoint_all_ok() {
        let registry = HealthRegistry::new();
        registry.register(HealthCheck {
            name: "svc".into(),
            status: HealthStatus::Ok,
            message: None,
            ..Default::default()
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server("127.0.0.1", port, registry)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /ready HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("200 OK"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_health_server_ready_endpoint_down() {
        let registry = HealthRegistry::new();
        registry.register(HealthCheck {
            name: "svc".into(),
            status: HealthStatus::Down,
            message: Some("unreachable".into()),
            ..Default::default()
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server("127.0.0.1", port, registry)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /ready HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("503"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_health_server_404_on_unknown_path() {
        let registry = HealthRegistry::new();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server("127.0.0.1", port, registry)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("404"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_health_server_backward_compat_healthz() {
        let registry = HealthRegistry::new();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server("127.0.0.1", port, registry)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("200 OK"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_legacy_health_server() {
        let metrics = Arc::new(UsageMetrics::new());
        metrics.set_ready(true);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server_legacy(port, Arc::clone(&metrics))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("200 OK"));
        assert!(response.contains("\"status\":\"ok\""));

        handle.abort();
    }

    #[tokio::test]
    async fn test_health_endpoint_includes_version_and_memory() {
        let registry = HealthRegistry::new();
        registry.register(HealthCheck {
            name: "svc".into(),
            status: HealthStatus::Ok,
            ..Default::default()
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let handle = start_health_server("127.0.0.1", port, registry)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 2048];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("\"version\":\""),
            "missing version: {}",
            response
        );
        assert!(
            response.contains("\"uptime_secs\":"),
            "missing uptime: {}",
            response
        );
        assert!(
            response.contains("\"checks\":"),
            "missing checks: {}",
            response
        );

        handle.abort();
    }

    // --- Health registry enhancement tests ---

    #[test]
    fn test_bump_restart_increments_count() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "gw".into(),
            status: HealthStatus::Ok,
            ..Default::default()
        });
        reg.bump_restart("gw");
        reg.bump_restart("gw");
        let checks = reg.all_checks();
        let check = checks.iter().find(|c| c.name == "gw").unwrap();
        assert_eq!(check.restart_count, 2);
    }

    #[test]
    fn test_bump_restart_noop_on_unknown() {
        let reg = HealthRegistry::new();
        // Should not panic
        reg.bump_restart("nonexistent");
        assert!(reg.is_ready());
    }

    #[test]
    fn test_set_error_marks_down_and_records_error() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "db".into(),
            status: HealthStatus::Ok,
            ..Default::default()
        });
        reg.set_error("db", "connection timeout");
        let checks = reg.all_checks();
        let check = checks.iter().find(|c| c.name == "db").unwrap();
        assert_eq!(check.status, HealthStatus::Down);
        assert_eq!(check.last_error.as_deref(), Some("connection timeout"));
        assert!(!reg.is_ready());
    }

    #[test]
    fn test_set_error_noop_on_unknown() {
        let reg = HealthRegistry::new();
        // Should not panic
        reg.set_error("ghost", "some error");
        assert!(reg.is_ready());
    }

    #[test]
    fn test_all_checks_returns_snapshot() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "a".into(),
            status: HealthStatus::Ok,
            ..Default::default()
        });
        reg.register(HealthCheck {
            name: "b".into(),
            status: HealthStatus::Degraded,
            ..Default::default()
        });
        let checks = reg.all_checks();
        assert_eq!(checks.len(), 2);
        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn test_all_checks_empty_registry() {
        let reg = HealthRegistry::new();
        assert!(reg.all_checks().is_empty());
    }

    // --- render_health_json + set_metrics tests ---

    #[test]
    fn test_registry_with_metrics() {
        let reg = HealthRegistry::new();
        let metrics = Arc::new(UsageMetrics::new());
        metrics.record_request();
        metrics.record_request();
        metrics.record_tool_calls(5);
        metrics.record_tokens(1000, 500);
        metrics.record_error();
        reg.set_metrics(Arc::clone(&metrics));

        let json = reg.render_health_json();
        assert!(json.contains("\"requests\":2"));
        assert!(json.contains("\"tool_calls\":5"));
        assert!(json.contains("\"input_tokens\":1000"));
        assert!(json.contains("\"output_tokens\":500"));
        assert!(json.contains("\"errors\":1"));
    }

    #[test]
    fn test_registry_without_metrics_omits_usage() {
        let reg = HealthRegistry::new();
        let json = reg.render_health_json();
        assert!(!json.contains("\"usage\""));
    }

    #[test]
    fn test_render_health_json_has_version() {
        let reg = HealthRegistry::new();
        let json = reg.render_health_json();
        assert!(json.contains("\"version\":\""));
        assert!(json.contains("\"uptime_secs\":"));
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"checks\":{}"));
    }

    #[test]
    fn test_render_health_json_status_degraded_when_down() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "db".into(),
            status: HealthStatus::Down,
            ..Default::default()
        });
        let json = reg.render_health_json();
        assert!(json.contains("\"status\":\"degraded\""));
    }

    #[test]
    fn test_render_checks_json_with_restart_and_error() {
        let reg = HealthRegistry::new();
        reg.register(HealthCheck {
            name: "gw".into(),
            status: HealthStatus::Down,
            message: None,
            restart_count: 3,
            last_error: Some("timeout".into()),
        });
        let json = reg.render_checks_json();
        assert!(json.contains("\"restart_count\":3"));
        assert!(json.contains("\"last_error\":\"timeout\""));
    }

    // --- get_rss_bytes tests ---

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn test_get_rss_bytes_returns_some() {
        let rss = get_rss_bytes();
        assert!(
            rss.is_some(),
            "get_rss_bytes() returned None on a supported platform"
        );
        assert!(
            rss.unwrap() > 0,
            "get_rss_bytes() returned Some(0), expected a positive RSS value"
        );
    }
}
