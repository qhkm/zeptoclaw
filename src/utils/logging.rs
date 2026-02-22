//! Logging initialization for ZeptoClaw.
//!
//! Supports three formats:
//! - `pretty`: default tracing pretty-print (human-readable, coloured)
//! - `component`: `[timestamp] [LEVEL] target message {fields}` — compact and grep-friendly;
//!   use the [`log_component!`] macro to add a `component` field for per-subsystem filtering
//! - `json`: structured JSON lines for log aggregators (e.g. Loki, CloudWatch)

use crate::config::{LogFormat, LoggingConfig};

/// Initialize the global tracing subscriber from config.
///
/// Call this once at startup before any tracing events are emitted.
/// Falls back to `RUST_LOG` env var; if unset, uses `cfg.level`.
pub fn init_logging(cfg: &LoggingConfig) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    match cfg.format {
        LogFormat::Json => {
            if let Some(path) = &cfg.file {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .expect("failed to open log file");
                tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(filter)
                    .with_writer(move || file.try_clone().expect("file writer"))
                    .init();
            } else {
                tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(filter)
                    .init();
            }
        }
        // Pretty and Component both use the compact text formatter.
        // Component-tagged events are emitted via the `log_component!` macro
        // which adds a structured `component` field — no custom layer needed.
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(true)
                .compact()
                .init();
        }
    }
}

/// Emit a component-tagged tracing event.
///
/// Works with any tracing level (`trace`, `debug`, `info`, `warn`, `error`).
/// The `component` field makes it easy to grep logs by subsystem:
///
/// ```
/// # use zeptoclaw::log_component;
/// log_component!(info, "telegram", "message received");
/// log_component!(warn, "agent", "token budget low", used = 8000u64, limit = 10000u64);
/// ```
#[macro_export]
macro_rules! log_component {
    ($level:ident, $component:expr, $msg:expr) => {
        tracing::$level!(component = $component, $msg)
    };
    ($level:ident, $component:expr, $msg:expr, $($key:ident = $val:expr),+ $(,)?) => {
        tracing::$level!(component = $component, $($key = $val,)+ $msg)
    };
}

#[cfg(test)]
mod tests {
    use crate::config::{LogFormat, LoggingConfig};

    #[test]
    fn test_default_logging_config() {
        let cfg = LoggingConfig::default();
        assert_eq!(cfg.format, LogFormat::Component);
        assert_eq!(cfg.level, "info");
        assert!(cfg.file.is_none());
    }

    #[test]
    fn test_log_format_deserialize_json() {
        let cfg: LoggingConfig =
            serde_json::from_str(r#"{"format":"json","level":"debug"}"#).unwrap();
        assert_eq!(cfg.format, LogFormat::Json);
        assert_eq!(cfg.level, "debug");
    }

    #[test]
    fn test_log_format_deserialize_pretty() {
        let cfg: LoggingConfig = serde_json::from_str(r#"{"format":"pretty"}"#).unwrap();
        assert_eq!(cfg.format, LogFormat::Pretty);
        assert_eq!(cfg.level, "info"); // default
    }

    #[test]
    fn test_log_format_deserialize_component() {
        let cfg: LoggingConfig = serde_json::from_str(r#"{"format":"component"}"#).unwrap();
        assert_eq!(cfg.format, LogFormat::Component);
    }

    #[test]
    fn test_logging_config_roundtrip() {
        let cfg = LoggingConfig {
            format: LogFormat::Json,
            file: Some("/tmp/zeptoclaw.log".to_string()),
            level: "debug".to_string(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: LoggingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.format, LogFormat::Json);
        assert_eq!(restored.file.as_deref(), Some("/tmp/zeptoclaw.log"));
        assert_eq!(restored.level, "debug");
    }

    #[test]
    fn test_log_format_partial_config_uses_defaults() {
        // Only specify level — format and file should use defaults
        let cfg: LoggingConfig = serde_json::from_str(r#"{"level":"trace"}"#).unwrap();
        assert_eq!(cfg.format, LogFormat::Component);
        assert!(cfg.file.is_none());
        assert_eq!(cfg.level, "trace");
    }
}
