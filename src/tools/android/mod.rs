//! Android device control tool.
//!
//! Provides screen perception and device interaction via ADB (Android Debug
//! Bridge). The LLM uses a single `android` tool with an `action` parameter
//! to read screen state, tap elements, type text, and navigate apps.
//!
//! # Actions
//!
//! - `screen` — Get parsed UI elements from the current screen
//! - `list_devices` — List connected Android devices
//! - `tap` — Tap at coordinates
//! - `long_press` — Long press at coordinates
//! - `swipe` — Swipe between two points
//! - `scroll` — Scroll in a direction (up/down/left/right)
//! - `type` — Type text into the focused field
//! - `clear_field` — Clear the focused text field
//! - `back` / `home` / `recent` / `enter` — Navigation buttons
//! - `key_event` — Send a key event by code
//! - `set_clipboard` / `get_clipboard` / `paste` — Clipboard operations
//! - `launch` — Launch an app by package name
//! - `open_url` — Open a URL in the browser
//! - `open_notifications` / `open_quick_settings` — System panels
//! - `screenshot` — Take a screenshot (base64 PNG)
//! - `wake_screen` — Wake up the device screen
//! - `shell` — Run a shell command on the device

pub mod actions;
pub mod adb;
pub mod screen;
pub mod stuck;
pub mod types;

use std::sync::Arc;

use tokio::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::debug;

use crate::error::{Result, ZeptoError};
use crate::tools::types::{Tool, ToolCategory, ToolContext, ToolOutput};

use self::adb::AdbExecutor;
use self::stuck::StuckDetector;

/// Android device control tool.
///
/// Wraps ADB commands behind a single tool interface with action-based
/// dispatch, screen perception, and stuck detection.
pub struct AndroidTool {
    adb: AdbExecutor,
    stuck: Arc<Mutex<StuckDetector>>,
}

impl Default for AndroidTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidTool {
    /// Create a new AndroidTool targeting the default device.
    pub fn new() -> Self {
        Self {
            adb: AdbExecutor::default(),
            stuck: Arc::new(Mutex::new(StuckDetector::default())),
        }
    }

    /// Create a new AndroidTool targeting a specific device serial.
    pub fn with_device(serial: &str) -> Self {
        Self {
            adb: AdbExecutor::with_device(serial),
            stuck: Arc::new(Mutex::new(StuckDetector::default())),
        }
    }

    /// Handle the `screen` action: dump UI, parse, score, return compact JSON.
    async fn handle_screen(&self) -> Result<String> {
        // Dump UI hierarchy
        let dump = self
            .adb
            .shell_retry("uiautomator dump /dev/tty")
            .await
            .map_err(|e| ZeptoError::Tool(format!("UI dump failed: {}", e)))?;

        // Strip the "UI hierarchy dumped to:" line if present, but keep the full XML payload
        let xml_start = dump.find("<?xml").or_else(|| dump.find("<hierarchy"));
        let xml = match xml_start {
            Some(idx) => &dump[idx..],
            None => &dump,
        };

        // Parse XML
        let elements = screen::parse_ui_dump(xml)?;

        // Get screen size and foreground app
        let (screen_w, screen_h) = self.adb.get_screen_size().await.unwrap_or((1080, 2400));
        let package = self
            .adb
            .get_foreground_app()
            .await
            .unwrap_or_else(|_| "unknown".into());

        // Process elements (score, dedup, top-N)
        let processed = screen::process_elements(elements, None);

        // Run stuck detection on the processed elements
        let alerts = {
            let mut detector = self.stuck.lock().await;
            detector.observe_screen(&processed)
        };

        // Build screen state
        let state = types::ScreenState {
            package,
            screen_size: [screen_w, screen_h],
            elements: processed,
        };

        // Serialize to JSON value so we can optionally attach alerts
        let mut screen_value = serde_json::to_value(&state)
            .map_err(|e| ZeptoError::Tool(format!("Serialization failed: {}", e)))?;

        // Attach alerts as a JSON field if any are present
        if !alerts.is_empty() {
            if let Value::Object(ref mut obj) = screen_value {
                obj.insert("alerts".to_string(), json!(alerts));
            }
        }

        let output = serde_json::to_string(&screen_value)
            .map_err(|e| ZeptoError::Tool(format!("Serialization failed: {}", e)))?;

        Ok(output)
    }

    /// Dispatch an action, recording it for stuck detection.
    async fn dispatch_action(&self, action: &str, args: &Value) -> Result<String> {
        // Record action for stuck detection
        {
            let mut detector = self.stuck.lock().await;
            let alerts = detector.observe_action(action);
            if !alerts.is_empty() {
                let alerts_json = serde_json::to_string(&alerts).unwrap_or_default();
                debug!("Stuck alerts: {}", alerts_json);
            }
        }

        match action {
            "screen" => self.handle_screen().await,
            "list_devices" => {
                let devices = self.adb.list_devices().await?;
                if devices.is_empty() {
                    Ok(
                        "No devices connected. Connect a device via USB or start an emulator."
                            .into(),
                    )
                } else {
                    Ok(format!("Connected devices: {}", devices.join(", ")))
                }
            }
            "tap" => {
                let (x, y) =
                    actions::parse_coordinates(args.get("x"), args.get("y"), args.get("coords"))?;
                actions::tap(&self.adb, x, y).await
            }
            "long_press" => {
                let (x, y) =
                    actions::parse_coordinates(args.get("x"), args.get("y"), args.get("coords"))?;
                let duration = args
                    .get("duration_ms")
                    .map(actions::value_to_i32)
                    .transpose()?;
                actions::long_press(&self.adb, x, y, duration).await
            }
            "swipe" => {
                let x1 =
                    actions::value_to_i32(args.get("x1").ok_or_else(|| {
                        ZeptoError::Tool("Missing required parameter 'x1'".into())
                    })?)?;
                let y1 =
                    actions::value_to_i32(args.get("y1").ok_or_else(|| {
                        ZeptoError::Tool("Missing required parameter 'y1'".into())
                    })?)?;
                let x2 =
                    actions::value_to_i32(args.get("x2").ok_or_else(|| {
                        ZeptoError::Tool("Missing required parameter 'x2'".into())
                    })?)?;
                let y2 =
                    actions::value_to_i32(args.get("y2").ok_or_else(|| {
                        ZeptoError::Tool("Missing required parameter 'y2'".into())
                    })?)?;
                let dur = args
                    .get("duration_ms")
                    .map(actions::value_to_i32)
                    .transpose()?;
                actions::swipe(&self.adb, x1, y1, x2, y2, dur).await
            }
            "scroll" => {
                let direction = args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("down");
                let (sw, sh) = self.adb.get_screen_size().await.unwrap_or((1080, 2400));
                actions::scroll(&self.adb, direction, sw, sh).await
            }
            "type" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'text' parameter".into()))?;
                actions::type_text(&self.adb, text).await
            }
            "clear_field" => actions::clear_field(&self.adb).await,
            "back" => actions::back(&self.adb).await,
            "home" => actions::home(&self.adb).await,
            "recent" => actions::recent(&self.adb).await,
            "enter" => actions::enter(&self.adb).await,
            "key_event" => {
                let key = args
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'key' parameter".into()))?;
                actions::key_event(&self.adb, key).await
            }
            "set_clipboard" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'text' parameter".into()))?;
                actions::set_clipboard(&self.adb, text).await
            }
            "get_clipboard" => actions::get_clipboard(&self.adb).await,
            "paste" => actions::paste(&self.adb).await,
            "launch" => {
                let package = args
                    .get("package")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'package' parameter".into()))?;
                actions::launch_app(&self.adb, package).await
            }
            "open_url" => {
                let url = args
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'url' parameter".into()))?;
                actions::open_url(&self.adb, url).await
            }
            "open_notifications" => actions::open_notifications(&self.adb).await,
            "open_quick_settings" => actions::open_quick_settings(&self.adb).await,
            "screenshot" => actions::screenshot_base64(&self.adb).await,
            "wake_screen" => actions::wake_screen(&self.adb).await,
            "shell" => {
                let cmd = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ZeptoError::Tool("Missing 'command' parameter".into()))?;
                actions::device_shell(&self.adb, cmd).await
            }
            _ => Err(ZeptoError::Tool(format!(
                "Unknown android action '{}'. Available: screen, list_devices, tap, long_press, \
                 swipe, scroll, type, clear_field, back, home, recent, enter, key_event, \
                 set_clipboard, get_clipboard, paste, launch, open_url, open_notifications, \
                 open_quick_settings, screenshot, wake_screen, shell",
                action
            ))),
        }
    }
}

#[async_trait]
impl Tool for AndroidTool {
    fn name(&self) -> &str {
        "android"
    }

    fn description(&self) -> &str {
        "Control an Android device via ADB. Workflow: (1) ALWAYS call action='screen' \
         first to see the current UI elements and their coordinates. (2) Use the \
         returned element coordinates for 'tap', 'type', 'swipe', 'scroll'. (3) Call \
         'screen' again after each interaction to verify the result. Never guess \
         coordinates — always read them from the screen response. For text input, \
         tap a field first, then use 'type'. Use 'scroll' with direction to reveal \
         off-screen content."
    }

    fn compact_description(&self) -> &str {
        "Android device control via ADB"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Hardware
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "screen", "list_devices", "tap", "long_press", "swipe", "scroll",
                        "type", "clear_field", "back", "home", "recent", "enter",
                        "key_event", "set_clipboard", "get_clipboard", "paste",
                        "launch", "open_url", "open_notifications", "open_quick_settings",
                        "screenshot", "wake_screen", "shell"
                    ],
                    "description": "Action to perform on the Android device"
                },
                "x": {
                    "type": "integer",
                    "description": "X coordinate for tap/long_press"
                },
                "y": {
                    "type": "integer",
                    "description": "Y coordinate for tap/long_press"
                },
                "coords": {
                    "description": "Alternative coordinates as [x,y] array or 'x,y' string"
                },
                "x1": { "type": "integer", "description": "Start X for swipe" },
                "y1": { "type": "integer", "description": "Start Y for swipe" },
                "x2": { "type": "integer", "description": "End X for swipe" },
                "y2": { "type": "integer", "description": "End Y for swipe" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type or set on clipboard"
                },
                "key": {
                    "type": "string",
                    "description": "Key code for key_event (e.g. 'KEYCODE_TAB')"
                },
                "package": {
                    "type": "string",
                    "description": "Android package name for launch (e.g. 'com.example.app')"
                },
                "url": {
                    "type": "string",
                    "description": "URL to open in browser"
                },
                "command": {
                    "type": "string",
                    "description": "Shell command for device shell action"
                },
                "duration_ms": {
                    "type": "integer",
                    "description": "Duration in ms for long_press (default 1000) or swipe (default 300)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZeptoError::Tool("Missing 'action' parameter".into()))?;

        debug!(action = action, "Android tool executing");
        self.dispatch_action(action, &args)
            .await
            .map(ToolOutput::llm_only)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name() {
        let tool = AndroidTool::new();
        assert_eq!(tool.name(), "android");
    }

    #[test]
    fn test_tool_description() {
        let tool = AndroidTool::new();
        assert!(tool.description().contains("ADB"));
        assert!(tool.description().contains("screen"));
        assert!(tool.description().contains("Never guess"));
    }

    #[test]
    fn test_tool_compact_description() {
        let tool = AndroidTool::new();
        assert!(tool.compact_description().len() < tool.description().len());
    }

    #[test]
    fn test_tool_parameters_has_action() {
        let tool = AndroidTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"]["enum"].is_array());
    }

    #[test]
    fn test_tool_parameters_action_list() {
        let tool = AndroidTool::new();
        let params = tool.parameters();
        let actions: Vec<&str> = params["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(actions.contains(&"screen"));
        assert!(actions.contains(&"tap"));
        assert!(actions.contains(&"type"));
        assert!(actions.contains(&"scroll"));
        assert!(actions.contains(&"launch"));
        assert!(actions.contains(&"shell"));
    }

    #[tokio::test]
    async fn test_missing_action() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'action'"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "nonexistent"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown android action"));
    }

    #[tokio::test]
    async fn test_type_missing_text() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "type"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'text'"));
    }

    #[tokio::test]
    async fn test_launch_missing_package() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "launch"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'package'"));
    }

    #[tokio::test]
    async fn test_key_event_missing_key() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "key_event"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'key'"));
    }

    #[tokio::test]
    async fn test_shell_missing_command() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "shell"}), &ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Missing 'command'"));
    }

    #[tokio::test]
    async fn test_open_url_missing_url() {
        let tool = AndroidTool::new();
        let ctx = ToolContext::new();
        let result = tool.execute(json!({"action": "open_url"}), &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'url'"));
    }

    #[test]
    fn test_with_device() {
        let tool = AndroidTool::with_device("emulator-5554");
        assert_eq!(tool.name(), "android");
    }
}
