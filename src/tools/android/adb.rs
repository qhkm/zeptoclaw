//! ADB command executor.
//!
//! Wraps `adb` subprocess calls with retry logic, device detection, and
//! timeout handling. All methods are async via `tokio::process::Command`.

use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::error::{Result, ZeptoError};

/// Default ADB command timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Max retry attempts for transient failures.
const MAX_RETRIES: u32 = 3;

/// ADB command executor with device targeting and retry support.
#[derive(Debug, Clone)]
pub struct AdbExecutor {
    /// Target device serial (empty = default device).
    device_serial: String,
    /// Path to adb binary.
    adb_path: String,
    /// Command timeout.
    timeout: Duration,
}

impl Default for AdbExecutor {
    fn default() -> Self {
        Self {
            device_serial: String::new(),
            adb_path: "adb".into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl AdbExecutor {
    /// Create an executor targeting a specific device.
    pub fn with_device(serial: &str) -> Self {
        Self {
            device_serial: serial.to_string(),
            ..Default::default()
        }
    }

    /// Run a raw ADB command with the given arguments.
    pub async fn run(&self, args: &[&str]) -> Result<String> {
        let mut cmd_args = Vec::new();
        if !self.device_serial.is_empty() {
            cmd_args.push("-s");
            cmd_args.push(&self.device_serial);
        }
        cmd_args.extend_from_slice(args);

        debug!(adb_path = %self.adb_path, args = ?cmd_args, "Running ADB command");

        let output = tokio::time::timeout(
            self.timeout,
            Command::new(&self.adb_path).args(&cmd_args).output(),
        )
        .await
        .map_err(|_| ZeptoError::Tool("ADB command timed out".into()))?
        .map_err(|e| ZeptoError::Tool(format!("Failed to run adb: {}", e)))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ZeptoError::Tool(format!("ADB error: {}", stderr.trim())))
        }
    }

    /// Run an ADB shell command.
    pub async fn shell(&self, cmd: &str) -> Result<String> {
        self.run(&["shell", cmd]).await
    }

    /// Run an ADB shell command with retry on transient failures.
    pub async fn shell_retry(&self, cmd: &str) -> Result<String> {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            match self.shell(cmd).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    let msg = e.to_string();
                    // Only retry on transient errors
                    if msg.contains("device not found")
                        || msg.contains("device offline")
                        || msg.contains("closed")
                    {
                        let delay = Duration::from_millis(1000 * 2u64.pow(attempt));
                        warn!(attempt = attempt + 1, delay_ms = ?delay, "ADB transient error, retrying");
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ZeptoError::Tool("ADB retry exhausted".into())))
    }

    /// List connected devices (serial numbers).
    pub async fn list_devices(&self) -> Result<Vec<String>> {
        let output = self.run(&["devices"]).await?;
        let devices: Vec<String> = output
            .lines()
            .skip(1) // skip "List of devices attached"
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 && parts[1] == "device" {
                    Some(parts[0].to_string())
                } else {
                    None
                }
            })
            .collect();
        Ok(devices)
    }

    /// Get screen dimensions `(width, height)`.
    pub async fn get_screen_size(&self) -> Result<(i32, i32)> {
        let output = self.shell("wm size").await?;
        // Output: "Physical size: 1080x2400"
        parse_screen_size(&output)
    }

    /// Get the foreground app package name.
    pub async fn get_foreground_app(&self) -> Result<String> {
        let output = self
            .shell("dumpsys activity activities | grep mResumedActivity")
            .await?;
        parse_foreground_app(&output)
    }

    /// Build the full command args (for testing/inspection).
    pub fn build_args<'a>(&'a self, args: &[&'a str]) -> Vec<&'a str> {
        let mut cmd_args = Vec::new();
        if !self.device_serial.is_empty() {
            cmd_args.push("-s");
            cmd_args.push(self.device_serial.as_str());
        }
        cmd_args.extend_from_slice(args);
        cmd_args
    }
}

/// Parse `wm size` output into (width, height).
fn parse_screen_size(output: &str) -> Result<(i32, i32)> {
    // Handle "Physical size: 1080x2400" or "Override size: 1080x2400"
    for line in output.lines() {
        let line = line.trim();
        if let Some(dims) = line.split(": ").nth(1) {
            let parts: Vec<&str> = dims.trim().split('x').collect();
            if parts.len() == 2 {
                let w = parts[0]
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| ZeptoError::Tool("Failed to parse screen width".into()))?;
                let h = parts[1]
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| ZeptoError::Tool("Failed to parse screen height".into()))?;
                return Ok((w, h));
            }
        }
    }
    Err(ZeptoError::Tool(format!(
        "Could not parse screen size from: {}",
        output.trim()
    )))
}

/// Parse `dumpsys activity` output to extract foreground package.
fn parse_foreground_app(output: &str) -> Result<String> {
    // "mResumedActivity: ActivityRecord{... com.example.app/.MainActivity ...}"
    for line in output.lines() {
        let line = line.trim();
        if line.contains("mResumedActivity") {
            // Extract "com.example.app/.MainActivity" then take package
            if let Some(start) = line.find('{') {
                let after_brace = &line[start + 1..];
                // Tokens: hash, pid:uid, component, ...
                for token in after_brace.split_whitespace() {
                    if token.contains('/') {
                        let pkg = token.split('/').next().unwrap_or("");
                        if pkg.contains('.') {
                            return Ok(pkg.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok("unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_screen_size_physical() {
        let output = "Physical size: 1080x2400\n";
        let (w, h) = parse_screen_size(output).unwrap();
        assert_eq!(w, 1080);
        assert_eq!(h, 2400);
    }

    #[test]
    fn test_parse_screen_size_override() {
        let output = "Physical size: 1080x2400\nOverride size: 720x1600\n";
        // Takes first match
        let (w, h) = parse_screen_size(output).unwrap();
        assert_eq!(w, 1080);
        assert_eq!(h, 2400);
    }

    #[test]
    fn test_parse_screen_size_invalid() {
        let result = parse_screen_size("garbage");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_foreground_app() {
        let output =
            "    mResumedActivity: ActivityRecord{abc1234 u0 com.example.app/.MainActivity t42}\n";
        let pkg = parse_foreground_app(output).unwrap();
        assert_eq!(pkg, "com.example.app");
    }

    #[test]
    fn test_parse_foreground_app_no_match() {
        let pkg = parse_foreground_app("some other output").unwrap();
        assert_eq!(pkg, "unknown");
    }

    #[test]
    fn test_build_args_no_device() {
        let exec = AdbExecutor::default();
        let args = exec.build_args(&["shell", "ls"]);
        assert_eq!(args, vec!["shell", "ls"]);
    }

    #[test]
    fn test_build_args_with_device() {
        let exec = AdbExecutor::with_device("emulator-5554");
        let args = exec.build_args(&["shell", "ls"]);
        assert_eq!(args, vec!["-s", "emulator-5554", "shell", "ls"]);
    }

    #[test]
    fn test_default_executor() {
        let exec = AdbExecutor::default();
        assert_eq!(exec.device_serial, "");
        assert_eq!(exec.adb_path, "adb");
        assert_eq!(exec.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_parse_screen_size_with_spaces() {
        let output = "Physical size:  1080x2400 \n";
        let (w, h) = parse_screen_size(output).unwrap();
        assert_eq!(w, 1080);
        assert_eq!(h, 2400);
    }

    #[test]
    fn test_parse_foreground_complex() {
        let output = "    mResumedActivity: ActivityRecord{d2f8e1a u0 org.mozilla.firefox/org.mozilla.fenix.HomeActivity t99}\n";
        let pkg = parse_foreground_app(output).unwrap();
        assert_eq!(pkg, "org.mozilla.firefox");
    }

    #[test]
    fn test_parse_foreground_empty() {
        let pkg = parse_foreground_app("").unwrap();
        assert_eq!(pkg, "unknown");
    }
}
