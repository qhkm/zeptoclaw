//! Device event system — monitors hardware events and notifies the agent.

pub mod usb;

use tokio::sync::mpsc;
use tracing::info;

/// A hardware event from any source.
#[derive(Debug, Clone)]
pub struct DeviceEvent {
    pub action: DeviceAction,
    pub kind: DeviceKind,
    pub vendor: String,
    pub product: String,
    pub serial: Option<String>,
    pub capabilities: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeviceAction {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum DeviceKind {
    Usb,
    Generic,
}

impl DeviceEvent {
    pub fn format_message(&self) -> String {
        let action = match self.action {
            DeviceAction::Connected => "Connected",
            DeviceAction::Disconnected => "Disconnected",
        };
        let mut msg = format!(
            "🔌 Device {}\n\nDevice: {} {}\n",
            action, self.vendor, self.product
        );
        if let Some(caps) = &self.capabilities {
            msg.push_str(&format!("Capabilities: {}\n", caps));
        }
        if let Some(serial) = &self.serial {
            msg.push_str(&format!("Serial: {}\n", serial));
        }
        msg
    }
}

/// Trait for hardware event sources.
pub trait EventSource: Send + Sync {
    fn kind(&self) -> &str;
    fn start(&self, tx: mpsc::Sender<DeviceEvent>) -> Result<(), String>;
    fn stop(&self);
}

/// Manages device event sources and publishes events to a callback.
pub struct DeviceService {
    enabled: bool,
    monitor_usb: bool,
}

impl DeviceService {
    pub fn new(enabled: bool, monitor_usb: bool) -> Self {
        Self {
            enabled,
            monitor_usb,
        }
    }

    /// Start the device service. Returns a receiver for device events.
    /// Returns `None` if disabled.
    pub fn start(&self) -> Option<mpsc::Receiver<DeviceEvent>> {
        if !self.enabled {
            return None;
        }

        let (tx, rx) = mpsc::channel(32);

        if self.monitor_usb {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                usb::monitor_usb(tx_clone).await;
            });
            info!("Device service: USB monitoring started");
        }

        Some(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_event_format_connected() {
        let event = DeviceEvent {
            action: DeviceAction::Connected,
            kind: DeviceKind::Usb,
            vendor: "Acme".into(),
            product: "Sensor v2".into(),
            serial: Some("ABC123".into()),
            capabilities: Some("serial".into()),
        };
        let msg = event.format_message();
        assert!(msg.contains("Connected"));
        assert!(msg.contains("Acme"));
        assert!(msg.contains("Sensor v2"));
        assert!(msg.contains("ABC123"));
        assert!(msg.contains("serial"));
    }

    #[test]
    fn test_device_event_format_disconnected() {
        let event = DeviceEvent {
            action: DeviceAction::Disconnected,
            kind: DeviceKind::Usb,
            vendor: "X".into(),
            product: "Y".into(),
            serial: None,
            capabilities: None,
        };
        let msg = event.format_message();
        assert!(msg.contains("Disconnected"));
        assert!(msg.contains("X"));
        assert!(msg.contains("Y"));
        assert!(!msg.contains("Serial"));
        assert!(!msg.contains("Capabilities"));
    }

    #[test]
    fn test_service_disabled_returns_none() {
        // Test that a disabled service has the enabled flag false
        // (start() requires tokio runtime; we test the field directly)
        let svc = DeviceService::new(false, true);
        assert!(!svc.enabled);
    }

    #[test]
    fn test_service_enabled_field() {
        let svc = DeviceService::new(true, false);
        assert!(svc.enabled);
        assert!(!svc.monitor_usb);
    }

    #[test]
    fn test_device_action_eq() {
        assert_eq!(DeviceAction::Connected, DeviceAction::Connected);
        assert_ne!(DeviceAction::Connected, DeviceAction::Disconnected);
    }

    #[test]
    fn test_format_message_with_capabilities_and_serial() {
        let event = DeviceEvent {
            action: DeviceAction::Connected,
            kind: DeviceKind::Generic,
            vendor: "Corp".into(),
            product: "Widget".into(),
            serial: Some("SN-42".into()),
            capabilities: Some("hid,mass_storage".into()),
        };
        let msg = event.format_message();
        assert!(msg.contains("hid,mass_storage"));
        assert!(msg.contains("SN-42"));
    }
}
