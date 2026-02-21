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
/// registry.register(HealthCheck { name: "provider".into(), status: HealthStatus::Ok, message: None });
/// assert!(registry.is_ready());
/// ```
#[derive(Clone)]
pub struct HealthRegistry {
    checks: Arc<RwLock<HashMap<String, HealthCheck>>>,
    start_time: Instant,
}

impl HealthRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            checks: Arc::new(RwLock::new(HashMap::new())),
            start_time: Instant::now(),
        }
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

    /// Elapsed time since the registry was created (proxy for process uptime).
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Render all checks as a compact JSON object for `/health` responses.
    fn render_checks_json(&self) -> String {
        let checks = self.checks.read().unwrap();
        if checks.is_empty() {
            return "{}".to_string();
        }
        let parts: Vec<String> = checks
            .values()
            .map(|c| {
                if let Some(ref msg) = c.message {
                    format!(
                        "\"{}\":{{\"status\":\"{}\",\"message\":\"{}\"}}",
                        c.name,
                        c.status.as_str(),
                        msg.replace('"', "\\\"")
                    )
                } else {
                    format!("\"{}\":{{\"status\":\"{}\"}}", c.name, c.status.as_str())
                }
            })
            .collect();
        format!("{{{}}}", parts.join(","))
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
                                let checks_json = registry.render_checks_json();
                                let uptime = registry.uptime().as_secs();
                                let body = format!(
                                    "{{\"status\":\"ok\",\"uptime_secs\":{},\"checks\":{}}}",
                                    uptime, checks_json
                                );
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

                        let (status, body) = match (method, path) {
                            ("GET", "/healthz") | ("GET", "/health") => {
                                ("200 OK", "{\"status\":\"ok\"}")
                            }
                            ("GET", "/readyz") | ("GET", "/ready") => {
                                if metrics.ready.load(Ordering::SeqCst) {
                                    ("200 OK", "{\"status\":\"ready\"}")
                                } else {
                                    ("503 Service Unavailable", "{\"status\":\"not_ready\"}")
                                }
                            }
                            _ => ("404 Not Found", "{\"error\":\"not_found\"}"),
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
        });
        reg.register(HealthCheck {
            name: "provider".into(),
            status: HealthStatus::Ok,
            message: None,
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
        });
        reg.register(HealthCheck {
            name: "svc".into(),
            status: HealthStatus::Down,
            message: Some("crashed".into()),
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
}
