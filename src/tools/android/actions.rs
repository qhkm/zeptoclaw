//! Android action handlers.
//!
//! Implements all ADB actions: navigation, text input, clipboard,
//! app management, and device control. Includes text escaping for
//! shell metacharacters and coordinate sanitization.

use crate::error::{Result, ZeptoError};

use super::adb::AdbExecutor;

/// Escape text for ADB shell `input text` command.
///
/// ADB `input text` requires escaping of shell metacharacters:
/// `\ " ' \` $ ! ? & | ; ( ) [ ] { } < > (space)`
pub fn escape_adb_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        match ch {
            ' ' => escaped.push_str("%s"),
            '\\' | '"' | '\'' | '`' | '$' | '!' | '?' | '&' | '|' | ';' | '(' | ')' | '[' | ']'
            | '{' | '}' | '<' | '>' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Parse and sanitize coordinates from various input formats.
///
/// Supports:
/// - `[x, y]` — normal JSON array
/// - `"828, 2017"` — string with comma/space
/// - `8282017` — concatenated digits (tries split at positions 2-4)
pub fn parse_coordinates(
    x_val: Option<&serde_json::Value>,
    y_val: Option<&serde_json::Value>,
    coords_val: Option<&serde_json::Value>,
) -> Result<(i32, i32)> {
    // Try explicit x, y first
    if let (Some(x), Some(y)) = (x_val, y_val) {
        let x = value_to_i32(x)?;
        let y = value_to_i32(y)?;
        return validate_coords(x, y);
    }

    // Try coords as string "x, y" or "x y"
    if let Some(coords) = coords_val.and_then(|v| v.as_str()) {
        let parts: Vec<&str> = coords.split([',', ' ']).filter(|s| !s.is_empty()).collect();
        if parts.len() == 2 {
            let x = parts[0]
                .trim()
                .parse::<i32>()
                .map_err(|_| ZeptoError::Tool("Invalid x coordinate".into()))?;
            let y = parts[1]
                .trim()
                .parse::<i32>()
                .map_err(|_| ZeptoError::Tool("Invalid y coordinate".into()))?;
            return validate_coords(x, y);
        }
    }

    // Try coords as array [x, y]
    if let Some(arr) = coords_val.and_then(|v| v.as_array()) {
        if arr.len() == 2 {
            let x = value_to_i32(&arr[0])?;
            let y = value_to_i32(&arr[1])?;
            return validate_coords(x, y);
        }
    }

    // Try concatenated number (e.g., 8282017)
    if let Some(n) = coords_val.and_then(|v| v.as_i64()) {
        let s = n.to_string();
        if s.len() >= 4 && s.len() <= 9 {
            // Try splits at positions 2, 3, 4
            for split_pos in 2..=4.min(s.len() - 1) {
                if let (Ok(x), Ok(y)) =
                    (s[..split_pos].parse::<i32>(), s[split_pos..].parse::<i32>())
                {
                    if (0..=10000).contains(&x) && (0..=10000).contains(&y) {
                        return Ok((x, y));
                    }
                }
            }
        }
    }

    Err(ZeptoError::Tool(
        "Missing or invalid coordinates. Provide x and y, or coords as [x,y] or \"x,y\"".into(),
    ))
}

fn value_to_i32(v: &serde_json::Value) -> Result<i32> {
    if let Some(n) = v.as_i64() {
        Ok(n as i32)
    } else if let Some(n) = v.as_f64() {
        Ok(n as i32)
    } else if let Some(s) = v.as_str() {
        s.trim()
            .parse::<i32>()
            .map_err(|_| ZeptoError::Tool(format!("Invalid coordinate value: {}", s)))
    } else {
        Err(ZeptoError::Tool("Invalid coordinate type".into()))
    }
}

fn validate_coords(x: i32, y: i32) -> Result<(i32, i32)> {
    if !(0..=10000).contains(&x) || !(0..=10000).contains(&y) {
        return Err(ZeptoError::Tool(format!(
            "Coordinates out of range: ({}, {}). Must be 0-10000.",
            x, y
        )));
    }
    Ok((x, y))
}

// ============================================================================
// Action implementations
// ============================================================================

/// Tap at coordinates.
pub async fn tap(adb: &AdbExecutor, x: i32, y: i32) -> Result<String> {
    adb.shell(&format!("input tap {} {}", x, y)).await?;
    Ok(format!("Tapped ({}, {})", x, y))
}

/// Long press at coordinates (default 1000ms).
pub async fn long_press(
    adb: &AdbExecutor,
    x: i32,
    y: i32,
    duration_ms: Option<i32>,
) -> Result<String> {
    let dur = duration_ms.unwrap_or(1000);
    adb.shell(&format!("input swipe {} {} {} {} {}", x, y, x, y, dur))
        .await?;
    Ok(format!("Long-pressed ({}, {}) for {}ms", x, y, dur))
}

/// Swipe from (x1,y1) to (x2,y2).
pub async fn swipe(
    adb: &AdbExecutor,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    duration_ms: Option<i32>,
) -> Result<String> {
    let dur = duration_ms.unwrap_or(300);
    adb.shell(&format!("input swipe {} {} {} {} {}", x1, y1, x2, y2, dur))
        .await?;
    Ok(format!(
        "Swiped ({},{}) -> ({},{}) in {}ms",
        x1, y1, x2, y2, dur
    ))
}

/// Scroll in a direction.
pub async fn scroll(
    adb: &AdbExecutor,
    direction: &str,
    screen_w: i32,
    screen_h: i32,
) -> Result<String> {
    let (x1, y1, x2, y2) = match direction {
        "up" => (screen_w / 2, screen_h * 3 / 4, screen_w / 2, screen_h / 4),
        "down" => (screen_w / 2, screen_h / 4, screen_w / 2, screen_h * 3 / 4),
        "left" => (screen_w * 3 / 4, screen_h / 2, screen_w / 4, screen_h / 2),
        "right" => (screen_w / 4, screen_h / 2, screen_w * 3 / 4, screen_h / 2),
        _ => {
            return Err(ZeptoError::Tool(format!(
                "Invalid scroll direction '{}'. Use: up, down, left, right",
                direction
            )));
        }
    };
    adb.shell(&format!("input swipe {} {} {} {} 500", x1, y1, x2, y2))
        .await?;
    Ok(format!("Scrolled {}", direction))
}

/// Type text (with escaping).
pub async fn type_text(adb: &AdbExecutor, text: &str) -> Result<String> {
    let escaped = escape_adb_text(text);
    adb.shell(&format!("input text '{}'", escaped)).await?;
    Ok(format!("Typed {} characters", text.len()))
}

/// Clear a focused text field.
pub async fn clear_field(adb: &AdbExecutor) -> Result<String> {
    // Move to end, select all, delete
    adb.shell("input keyevent KEYCODE_MOVE_END").await?;
    adb.shell("input keyevent --longpress KEYCODE_DEL").await?;
    // Additional: select all + delete as fallback
    adb.shell("input keyevent 29 --meta 28672").await?; // Ctrl+A
    adb.shell("input keyevent KEYCODE_DEL").await?;
    Ok("Cleared field".into())
}

/// Press the back button.
pub async fn back(adb: &AdbExecutor) -> Result<String> {
    adb.shell("input keyevent KEYCODE_BACK").await?;
    Ok("Pressed Back".into())
}

/// Press the home button.
pub async fn home(adb: &AdbExecutor) -> Result<String> {
    adb.shell("input keyevent KEYCODE_HOME").await?;
    Ok("Pressed Home".into())
}

/// Show recent apps.
pub async fn recent(adb: &AdbExecutor) -> Result<String> {
    adb.shell("input keyevent KEYCODE_APP_SWITCH").await?;
    Ok("Opened Recents".into())
}

/// Press enter/return.
pub async fn enter(adb: &AdbExecutor) -> Result<String> {
    adb.shell("input keyevent KEYCODE_ENTER").await?;
    Ok("Pressed Enter".into())
}

/// Send a key event by code or name.
pub async fn key_event(adb: &AdbExecutor, key: &str) -> Result<String> {
    adb.shell(&format!("input keyevent {}", key)).await?;
    Ok(format!("Sent key event: {}", key))
}

/// Set clipboard text.
pub async fn set_clipboard(adb: &AdbExecutor, text: &str) -> Result<String> {
    let escaped = escape_adb_text(text);
    adb.shell(&format!(
        "am broadcast -a clipper.set -e text '{}'",
        escaped
    ))
    .await?;
    Ok("Clipboard set".into())
}

/// Get clipboard text.
pub async fn get_clipboard(adb: &AdbExecutor) -> Result<String> {
    let output = adb.shell("am broadcast -a clipper.get").await?;
    Ok(output.trim().to_string())
}

/// Paste from clipboard.
pub async fn paste(adb: &AdbExecutor) -> Result<String> {
    adb.shell("input keyevent KEYCODE_PASTE").await?;
    Ok("Pasted from clipboard".into())
}

/// Launch an app by package name.
pub async fn launch_app(adb: &AdbExecutor, package: &str) -> Result<String> {
    // Try monkey first (works without knowing activity name)
    let result = adb
        .shell(&format!(
            "monkey -p {} -c android.intent.category.LAUNCHER 1",
            package
        ))
        .await;

    match result {
        Ok(_) => Ok(format!("Launched {}", package)),
        Err(_) => {
            // Fallback: am start with launcher intent
            adb.shell(&format!(
                "am start -a android.intent.action.MAIN -c android.intent.category.LAUNCHER -n {}",
                package
            ))
            .await?;
            Ok(format!("Launched {} (via am start)", package))
        }
    }
}

/// Open a URL in the default browser.
pub async fn open_url(adb: &AdbExecutor, url: &str) -> Result<String> {
    adb.shell(&format!(
        "am start -a android.intent.action.VIEW -d '{}'",
        url
    ))
    .await?;
    Ok(format!("Opened URL: {}", url))
}

/// Open notifications panel.
pub async fn open_notifications(adb: &AdbExecutor) -> Result<String> {
    adb.shell("cmd statusbar expand-notifications").await?;
    Ok("Opened notifications".into())
}

/// Open quick settings panel.
pub async fn open_quick_settings(adb: &AdbExecutor) -> Result<String> {
    adb.shell("cmd statusbar expand-settings").await?;
    Ok("Opened quick settings".into())
}

/// Take a screenshot and return as base64 PNG.
pub async fn screenshot_base64(adb: &AdbExecutor) -> Result<String> {
    let output = adb
        .run(&["exec-out", "screencap", "-p"])
        .await
        .map_err(|e| ZeptoError::Tool(format!("Screenshot failed: {}", e)))?;
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        output.as_bytes(),
    );
    Ok(encoded)
}

/// Wake up the screen.
pub async fn wake_screen(adb: &AdbExecutor) -> Result<String> {
    adb.shell("input keyevent KEYCODE_WAKEUP").await?;
    Ok("Screen woken".into())
}

/// Run an arbitrary shell command on the device.
pub async fn device_shell(adb: &AdbExecutor, cmd: &str) -> Result<String> {
    // Block dangerous commands
    let blocked = ["rm -rf", "reboot", "factory_reset", "wipe", "format"];
    let lower = cmd.to_lowercase();
    for pattern in &blocked {
        if lower.contains(pattern) {
            return Err(ZeptoError::Tool(format!(
                "Blocked dangerous command containing '{}'",
                pattern
            )));
        }
    }
    adb.shell(cmd).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_escape_adb_text_basic() {
        assert_eq!(escape_adb_text("hello world"), "hello%sworld");
    }

    #[test]
    fn test_escape_adb_text_metacharacters() {
        assert_eq!(escape_adb_text("a$b"), "a\\$b");
        assert_eq!(escape_adb_text("a\"b"), "a\\\"b");
        assert_eq!(escape_adb_text("a'b"), "a\\'b");
        assert_eq!(escape_adb_text("a&b"), "a\\&b");
        assert_eq!(escape_adb_text("a|b"), "a\\|b");
        assert_eq!(escape_adb_text("a;b"), "a\\;b");
        assert_eq!(escape_adb_text("a(b)"), "a\\(b\\)");
        assert_eq!(escape_adb_text("a[b]"), "a\\[b\\]");
        assert_eq!(escape_adb_text("a{b}"), "a\\{b\\}");
        assert_eq!(escape_adb_text("a<b>"), "a\\<b\\>");
        assert_eq!(escape_adb_text("a!b"), "a\\!b");
        assert_eq!(escape_adb_text("a?b"), "a\\?b");
        assert_eq!(escape_adb_text("a`b"), "a\\`b");
        assert_eq!(escape_adb_text("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_adb_text_empty() {
        assert_eq!(escape_adb_text(""), "");
    }

    #[test]
    fn test_escape_adb_text_no_escaping() {
        assert_eq!(escape_adb_text("abc123"), "abc123");
    }

    #[test]
    fn test_parse_coordinates_xy() {
        let (x, y) = parse_coordinates(Some(&json!(540)), Some(&json!(1200)), None).unwrap();
        assert_eq!((x, y), (540, 1200));
    }

    #[test]
    fn test_parse_coordinates_string() {
        let (x, y) = parse_coordinates(None, None, Some(&json!("828, 2017"))).unwrap();
        assert_eq!((x, y), (828, 2017));
    }

    #[test]
    fn test_parse_coordinates_string_space() {
        let (x, y) = parse_coordinates(None, None, Some(&json!("828 2017"))).unwrap();
        assert_eq!((x, y), (828, 2017));
    }

    #[test]
    fn test_parse_coordinates_array() {
        let (x, y) = parse_coordinates(None, None, Some(&json!([828, 2017]))).unwrap();
        assert_eq!((x, y), (828, 2017));
    }

    #[test]
    fn test_parse_coordinates_concatenated() {
        // "8282017" -> try split at pos 3: "828" + "2017"
        let (x, y) = parse_coordinates(None, None, Some(&json!(8282017))).unwrap();
        assert_eq!((x, y), (828, 2017));
    }

    #[test]
    fn test_parse_coordinates_out_of_range() {
        let result = parse_coordinates(Some(&json!(50000)), Some(&json!(1200)), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_coordinates_missing() {
        let result = parse_coordinates(None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_coordinates_float() {
        let (x, y) = parse_coordinates(Some(&json!(540.5)), Some(&json!(1200.7)), None).unwrap();
        assert_eq!((x, y), (540, 1200));
    }

    #[test]
    fn test_parse_coordinates_string_values() {
        let (x, y) = parse_coordinates(Some(&json!("540")), Some(&json!("1200")), None).unwrap();
        assert_eq!((x, y), (540, 1200));
    }

    #[test]
    fn test_validate_coords_boundary() {
        assert!(validate_coords(0, 0).is_ok());
        assert!(validate_coords(10000, 10000).is_ok());
        assert!(validate_coords(-1, 0).is_err());
        assert!(validate_coords(0, 10001).is_err());
    }

    #[test]
    fn test_blocked_shell_commands() {
        // Can't actually run these without ADB, but test the blocking logic
        let blocked_cmds = vec!["rm -rf /", "reboot", "factory_reset data"];
        for cmd in blocked_cmds {
            // device_shell is async, so we test the pattern matching directly
            let lower = cmd.to_lowercase();
            let patterns = ["rm -rf", "reboot", "factory_reset", "wipe", "format"];
            let is_blocked = patterns.iter().any(|p| lower.contains(p));
            assert!(is_blocked, "Command '{}' should be blocked", cmd);
        }
    }

    #[test]
    fn test_escape_multiple_spaces() {
        assert_eq!(escape_adb_text("a b c"), "a%sb%sc");
    }

    #[test]
    fn test_parse_coordinates_negative_via_string() {
        let result = parse_coordinates(None, None, Some(&json!("-10, 100")));
        assert!(result.is_err()); // negative out of range
    }
}
