//! Panel configuration types.

use serde::{Deserialize, Serialize};

/// Authentication mode for the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Bearer token auth (default) — no login screen.
    #[default]
    Token,
    /// Username/password login with JWT session.
    Password,
    /// No authentication (localhost trust only).
    None,
}

/// Panel (control panel) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelConfig {
    /// Whether the panel is enabled.
    pub enabled: bool,
    /// Port for the panel frontend (static files).
    pub port: u16,
    /// Port for the API server.
    pub api_port: u16,
    /// Authentication mode.
    pub auth_mode: AuthMode,
    /// Bind address (default: 127.0.0.1).
    pub bind: String,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 9092,
            api_port: 9091,
            auth_mode: AuthMode::Token,
            bind: "127.0.0.1".to_string(),
        }
    }
}

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
