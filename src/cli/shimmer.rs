//! Shimmer spinner for CLI "Thinking..." state.
//!
//! Renders a gradient text wave animation on stderr while the LLM is processing.
//! The wave sweeps across the "Thinking..." text using ANSI 256-color codes,
//! creating a shimmer effect alongside a braille spinner character.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// Braille spinner frames.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The text to shimmer.
const SHIMMER_TEXT: &str = "Thinking...";

/// Gradient palette (ANSI 256-color indices): dark gray → white → dark gray.
/// This creates the "highlight sweep" effect.
const GRADIENT: &[u8] = &[240, 244, 248, 252, 255, 252, 248, 244, 240];

/// Width of the shimmer highlight (number of gradient entries).
const WAVE_WIDTH: usize = GRADIENT.len();

/// Frame interval in milliseconds.
const FRAME_MS: u64 = 80;

/// A shimmer spinner that runs in the background until stopped.
pub struct ShimmerSpinner {
    running: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShimmerSpinner {
    /// Start the shimmer animation on stderr. Returns a handle to stop it.
    pub fn start() -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let notify = Arc::new(Notify::new());
        let r = Arc::clone(&running);
        let n = Arc::clone(&notify);

        tokio::spawn(async move {
            let text_chars: Vec<char> = SHIMMER_TEXT.chars().collect();
            let text_len = text_chars.len();
            // The wave position sweeps from -WAVE_WIDTH to text_len
            let total_positions = text_len + WAVE_WIDTH;
            let mut frame: usize = 0;

            // Hide cursor
            eprint!("\x1b[?25l");

            while r.load(Ordering::Relaxed) {
                let spinner = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                let wave_pos = (frame % total_positions) as isize - WAVE_WIDTH as isize;

                // Build the shimmered text
                let mut buf = String::with_capacity(128);
                buf.push_str("\r\x1b[2K"); // clear line
                buf.push_str(&format!("  \x1b[38;5;245m{}\x1b[0m ", spinner));

                for (i, ch) in text_chars.iter().enumerate() {
                    let dist = i as isize - wave_pos;
                    if dist >= 0 && (dist as usize) < WAVE_WIDTH {
                        // Inside the wave — use gradient color
                        let color = GRADIENT[dist as usize];
                        buf.push_str(&format!("\x1b[38;5;{}m{}\x1b[0m", color, ch));
                    } else {
                        // Outside wave — dim gray
                        buf.push_str(&format!("\x1b[38;5;240m{}\x1b[0m", ch));
                    }
                }

                eprint!("{}", buf);
                let _ = io::stderr().flush();

                frame += 1;

                // Wait for frame interval or stop signal
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(FRAME_MS)) => {}
                    _ = n.notified() => break,
                }
            }

            // Clear the shimmer line and show cursor
            eprint!("\r\x1b[2K\x1b[?25h");
            let _ = io::stderr().flush();
        });

        Self { running, notify }
    }

    /// Stop the shimmer animation.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.notify.notify_one();
    }
}

impl Drop for ShimmerSpinner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Format elapsed time: show milliseconds for fast ops, seconds for slower ones.
fn fmt_elapsed(elapsed_ms: u64) -> String {
    if elapsed_ms < 1000 {
        format!("{}ms", elapsed_ms)
    } else {
        format!("{:.1}s", elapsed_ms as f64 / 1000.0)
    }
}

/// Format a tool step line with checkmark, step number, tool name, and argument hint.
///
/// `step` is 1-based. `args_hint` is an optional short description extracted from
/// the tool call arguments (e.g. a file path or action name).
pub fn format_tool_start(step: usize, tool_name: &str, args_hint: Option<&str>) -> String {
    let hint = args_hint
        .map(|h| format!(" \x1b[38;5;245m→ {}\x1b[0m", h))
        .unwrap_or_default();
    format!(
        "  \x1b[38;5;245m⠸\x1b[0m \x1b[2mStep {}\x1b[0m · \x1b[1m{}\x1b[0m{}",
        step, tool_name, hint
    )
}

/// Format a completed tool step (overwrites the current line).
pub fn format_tool_done(
    step: usize,
    tool_name: &str,
    args_hint: Option<&str>,
    elapsed_ms: u64,
) -> String {
    let hint = args_hint
        .map(|h| format!(" \x1b[38;5;245m→ {}\x1b[0m", h))
        .unwrap_or_default();
    format!(
        "  \x1b[32m✓\x1b[0m \x1b[2mStep {}\x1b[0m · {}{} \x1b[2m({})\x1b[0m",
        step,
        tool_name,
        hint,
        fmt_elapsed(elapsed_ms)
    )
}

/// Format a failed tool step.
pub fn format_tool_failed(
    step: usize,
    tool_name: &str,
    args_hint: Option<&str>,
    elapsed_ms: u64,
    error: &str,
) -> String {
    let hint = args_hint
        .map(|h| format!(" \x1b[38;5;245m→ {}\x1b[0m", h))
        .unwrap_or_default();
    // Truncate error to first 80 chars for display
    let short_error = if error.len() > 80 {
        format!("{}…", &error[..80])
    } else {
        error.to_string()
    };
    format!(
        "  \x1b[31m✗\x1b[0m \x1b[2mStep {}\x1b[0m · {}{} \x1b[31m({}: {})\x1b[0m",
        step,
        tool_name,
        hint,
        fmt_elapsed(elapsed_ms),
        short_error,
    )
}

/// Print a separator line before the final response.
pub fn print_response_separator() {
    eprintln!();
    eprintln!("  \x1b[2m{}\x1b[0m", "─".repeat(40));
    eprintln!();
}

/// Print a compact metadata footer after the response.
///
/// Example: `⠿ 1,247 tokens · 3 tool calls · 2.1s`
pub fn print_metadata_footer(total_tokens: u64, tool_calls: u64, elapsed: std::time::Duration) {
    // Skip footer entirely when there is nothing meaningful to report
    if total_tokens == 0 && tool_calls == 0 {
        return;
    }

    let mut parts = Vec::with_capacity(3);

    if total_tokens > 0 {
        parts.push(format_number_with_commas(total_tokens) + " tokens");
    }

    if tool_calls > 0 {
        let label = if tool_calls == 1 {
            "tool call"
        } else {
            "tool calls"
        };
        parts.push(format!("{} {}", tool_calls, label));
    }

    let elapsed_ms = elapsed.as_millis() as u64;
    parts.push(fmt_elapsed(elapsed_ms));

    eprintln!();
    eprintln!(
        "  \x1b[38;5;245m⠿ {}\x1b[0m",
        parts.join(" \x1b[38;5;240m·\x1b[38;5;245m ")
    );
}

/// Format a number with comma separators (e.g. 1247 → "1,247").
fn format_number_with_commas(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

/// Extract a short argument hint from tool call arguments JSON.
///
/// Looks for common keys like `path`, `file`, `filename`, `command`, `action`,
/// `query`, `key`, `url` and returns the first found value (truncated).
pub fn extract_args_hint(_tool_name: &str, args_json: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(args_json).ok()?;
    let obj = val.as_object()?;

    // Priority order of keys to extract
    let keys = [
        "path",
        "file",
        "filename",
        "file_path",
        "command",
        "action",
        "query",
        "key",
        "url",
        "pattern",
        "content",
    ];

    for key in &keys {
        if let Some(v) = obj.get(*key) {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            // Truncate long values
            if s.len() > 50 {
                return Some(format!("{}…", &s[..50]));
            }
            // For "content" key, show just "writing N chars"
            if *key == "content" {
                return Some(format!("writing {} chars", s.len()));
            }
            return Some(s);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_args_hint_path() {
        let args = r#"{"path": "src/main.rs"}"#;
        assert_eq!(
            extract_args_hint("read_file", args),
            Some("src/main.rs".to_string())
        );
    }

    #[test]
    fn test_extract_args_hint_command() {
        let args = r#"{"command": "cargo build"}"#;
        assert_eq!(
            extract_args_hint("shell", args),
            Some("cargo build".to_string())
        );
    }

    #[test]
    fn test_extract_args_hint_no_match() {
        let args = r#"{"foo": "bar"}"#;
        assert_eq!(extract_args_hint("echo", args), None);
    }

    #[test]
    fn test_extract_args_hint_truncation() {
        let long = "a".repeat(60);
        let args = format!(r#"{{"path": "{}"}}"#, long);
        let hint = extract_args_hint("read_file", &args).unwrap();
        assert!(hint.len() <= 54); // 50 chars + "…" (3 bytes in UTF-8)
        assert!(hint.ends_with('…'));
    }

    #[test]
    fn test_extract_args_hint_invalid_json() {
        assert_eq!(extract_args_hint("echo", "not json"), None);
    }

    #[test]
    fn test_extract_args_hint_content_key() {
        let args = r#"{"path": "file.py", "content": "def hello():\n    pass"}"#;
        // Should prefer "path" over "content" due to priority
        assert_eq!(
            extract_args_hint("write_file", args),
            Some("file.py".to_string())
        );
    }

    #[test]
    fn test_extract_args_hint_action() {
        let args = r#"{"action": "set", "key": "user:name"}"#;
        assert_eq!(
            extract_args_hint("longterm_memory", args),
            Some("set".to_string())
        );
    }

    #[test]
    fn test_format_tool_done_contains_checkmark() {
        let line = format_tool_done(1, "read_file", Some("main.rs"), 150);
        assert!(line.contains('✓'));
        assert!(line.contains("Step 1"));
        assert!(line.contains("read_file"));
        assert!(line.contains("main.rs"));
        assert!(line.contains("150ms"));
    }

    #[test]
    fn test_format_tool_failed_contains_cross() {
        let line = format_tool_failed(2, "shell", None, 5000, "exit code 1");
        assert!(line.contains('✗'));
        assert!(line.contains("Step 2"));
        assert!(line.contains("shell"));
        assert!(line.contains("5.0s"));
        assert!(line.contains("exit code 1"));
    }

    #[test]
    fn test_fmt_elapsed_milliseconds() {
        assert_eq!(fmt_elapsed(0), "0ms");
        assert_eq!(fmt_elapsed(3), "3ms");
        assert_eq!(fmt_elapsed(150), "150ms");
        assert_eq!(fmt_elapsed(999), "999ms");
    }

    #[test]
    fn test_fmt_elapsed_seconds() {
        assert_eq!(fmt_elapsed(1000), "1.0s");
        assert_eq!(fmt_elapsed(1500), "1.5s");
        assert_eq!(fmt_elapsed(5000), "5.0s");
    }

    #[test]
    fn test_format_tool_start_with_hint() {
        let line = format_tool_start(3, "edit_file", Some("fixing bug"));
        assert!(line.contains('⠸'));
        assert!(line.contains("Step 3"));
        assert!(line.contains("edit_file"));
        assert!(line.contains("fixing bug"));
    }

    #[test]
    fn test_format_tool_start_no_hint() {
        let line = format_tool_start(1, "echo", None);
        assert!(line.contains("echo"));
        assert!(!line.contains('→'));
    }

    #[test]
    fn test_format_tool_failed_long_error_truncated() {
        let long_error = "e".repeat(120);
        let line = format_tool_failed(1, "shell", None, 100, &long_error);
        assert!(line.contains('…'));
    }

    #[test]
    fn test_format_number_with_commas() {
        assert_eq!(format_number_with_commas(0), "0");
        assert_eq!(format_number_with_commas(42), "42");
        assert_eq!(format_number_with_commas(999), "999");
        assert_eq!(format_number_with_commas(1000), "1,000");
        assert_eq!(format_number_with_commas(1247), "1,247");
        assert_eq!(format_number_with_commas(1_000_000), "1,000,000");
    }
}
