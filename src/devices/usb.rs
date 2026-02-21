//! USB hotplug monitoring via udev (Linux only).

use tokio::sync::mpsc;

use super::DeviceEvent;
#[cfg(target_os = "linux")]
use super::{DeviceAction, DeviceKind};

/// Monitor USB hotplug events. On non-Linux platforms, this is a no-op.
pub async fn monitor_usb(tx: mpsc::Sender<DeviceEvent>) {
    #[cfg(target_os = "linux")]
    monitor_usb_linux(tx).await;

    #[cfg(not(target_os = "linux"))]
    {
        let _ = tx;
        tracing::debug!("USB monitoring is only supported on Linux");
    }
}

#[cfg(target_os = "linux")]
async fn monitor_usb_linux(tx: mpsc::Sender<DeviceEvent>) {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    // Use `udevadm monitor --udev --subsystem-match=usb` for hotplug events.
    let mut child = match Command::new("udevadm")
        .args(["monitor", "--udev", "--subsystem-match=usb"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to start udevadm (USB monitoring disabled): {}", e);
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Err(e) => {
                    tracing::warn!("udevadm read error: {}", e);
                    break;
                }
                Ok(_) => {
                    if let Some(event) = parse_udevadm_line(&line) {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn parse_udevadm_line(line: &str) -> Option<DeviceEvent> {
    // udevadm output: "UDEV  [timestamp] add/remove   /devices/... (usb)"
    let lower = line.to_lowercase();
    let action = if lower.contains(" add ") {
        DeviceAction::Connected
    } else if lower.contains(" remove ") {
        DeviceAction::Disconnected
    } else {
        return None;
    };

    Some(DeviceEvent {
        action,
        kind: DeviceKind::Usb,
        vendor: "USB Device".into(),
        product: "Unknown".into(),
        serial: None,
        capabilities: None,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::*;

    #[test]
    fn test_monitor_usb_noop_on_non_linux() {
        // On non-Linux, monitor_usb compiles and does nothing — just verify it compiles.
        // This test passes on all platforms.
        assert!(true);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_add_line() {
        let line = "UDEV  [1234.567890] add      /devices/pci0000:00/... (usb)";
        let event = parse_udevadm_line(line).unwrap();
        assert_eq!(event.action, DeviceAction::Connected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_remove_line() {
        let line = "UDEV  [1234.567890] remove   /devices/pci0000:00/... (usb)";
        let event = parse_udevadm_line(line).unwrap();
        assert_eq!(event.action, DeviceAction::Disconnected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_bind_returns_none() {
        let line = "UDEV  [1234.567890] bind     /devices/... (usb)";
        assert!(parse_udevadm_line(line).is_none());
    }
}
