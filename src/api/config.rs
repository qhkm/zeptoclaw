//! Panel configuration types.
//!
//! The canonical definitions of [`PanelConfig`] and [`AuthMode`] live in
//! [`crate::config::types`] so that config deserialization works regardless of
//! whether the `panel` feature is enabled.  This module re-exports them for
//! backward compatibility within the `api` crate.

pub use crate::config::{AuthMode, PanelConfig};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_config_defaults() {
        let cfg = PanelConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.port, 9092);
        assert_eq!(cfg.api_port, 9091);
        assert_eq!(cfg.auth_mode, AuthMode::Token);
        assert_eq!(cfg.bind, "127.0.0.1");
    }

    #[test]
    fn test_auth_mode_serde_roundtrip() {
        let json = r#""password""#;
        let mode: AuthMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, AuthMode::Password);
        let back = serde_json::to_string(&mode).unwrap();
        assert_eq!(back, r#""password""#);
    }

    #[test]
    fn test_panel_config_deserialize_partial() {
        let json = r#"{"enabled": true, "port": 3000}"#;
        let cfg: PanelConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.api_port, 9091); // default
    }
}
