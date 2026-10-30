//! Interactive agent UI: live status line, tool steps, reply rendering and the
//! per-turn footer.
//!
//! Everything that writes to the terminal during a turn goes through
//! [`AgentUi`] on a single task, so tool feedback and the reply appear in order.
//! The animated [`StatusLine`] shares one lock with every line printed above
//! it, and clearing it is synchronous, so the spinner can never be left
//! behind on screen or overwrite the reply.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthStr;
use zeptoclaw::agent::{ToolFeedback, ToolFeedbackPhase};

use super::markdown::{MarkdownRenderer, MarkdownStream, INDENT};
use super::term::{self, TermCaps};

/// Braille spinner frames.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Gradient (ANSI 256-color indices) swept across the status label.
const GRADIENT: &[u8] = &[244, 248, 252, 255, 252, 248, 244];

/// Frame interval in milliseconds.
const FRAME_MS: u64 = 80;

/// Elapsed time is shown on the status line only after this long.
const SHOW_ELAPSED_AFTER: Duration = Duration::from_secs(1);

const DIM: &str = "\x1b[38;5;245m";
const FAINT: &str = "\x1b[38;5;240m";
const ACCENT: &str = "\x1b[38;5;117m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// Wrap `text` in an ANSI style when color is enabled.
fn paint(color: bool, style: &str, text: &str) -> String {
    if color && !text.is_empty() {
        format!("{style}{text}{RESET}")
    } else {
        text.to_string()
    }
}

// ---------------------------------------------------------------------------
// Status line
// ---------------------------------------------------------------------------

struct Activity {
    label: String,
    detail: Option<String>,
    started: Instant,
}

struct StatusState {
    activity: Option<Activity>,
    frame: usize,
    drawn: bool,
    paused: bool,
    color: bool,
}

impl StatusState {
    /// Erase the status line if it is on screen.
    fn erase(&mut self) {
        if self.drawn {
            eprint!("\r\x1b[2K\x1b[?25h");
            let _ = io::stderr().flush();
            self.drawn = false;
        }
    }

    fn draw(&mut self) {
        if self.paused {
            return;
        }
        let Some(activity) = &self.activity else {
            return;
        };
        let line = render_status(
            activity,
            self.frame,
            self.color,
            term::width().saturating_sub(1),
        );
        eprint!("\r\x1b[2K\x1b[?25l{line}");
        let _ = io::stderr().flush();
        self.drawn = true;
    }
}

/// Build one frame of the status line, truncated to `max_width` columns so it
/// never wraps (a wrapped line cannot be erased with `\r`).
fn render_status(activity: &Activity, frame: usize, color: bool, max_width: usize) -> String {
    let spinner = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()].to_string();
    let elapsed = activity.started.elapsed();
    let elapsed_text = if elapsed >= SHOW_ELAPSED_AFTER {
        format!(" · {}", fmt_elapsed(elapsed.as_millis() as u64))
    } else {
        String::new()
    };

    let budget = max_width
        .saturating_sub(INDENT.len() + 2 + elapsed_text.width())
        .max(8);
    let label = truncate_to_width(&activity.label, budget);
    let detail = activity
        .detail
        .as_deref()
        .map(|d| {
            let room = budget.saturating_sub(label.width() + 3);
            if room < 4 {
                String::new()
            } else {
                format!(" → {}", truncate_to_width(d, room))
            }
        })
        .unwrap_or_default();

    let mut out = String::from(INDENT);
    out.push_str(&paint(color, DIM, &spinner));
    out.push(' ');
    if color {
        out.push_str(&shimmer(&label, frame));
    } else {
        out.push_str(&label);
    }
    out.push_str(&paint(color, DIM, &detail));
    out.push_str(&paint(color, FAINT, &elapsed_text));
    out
}

/// Sweep a highlight across `text`.
fn shimmer(text: &str, frame: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let span = chars.len() + GRADIENT.len() * 2;
    let pos = (frame % span) as isize - GRADIENT.len() as isize;
    let mut out = String::new();
    for (i, ch) in chars.iter().enumerate() {
        let d = i as isize - pos;
        let color = if d >= 0 && (d as usize) < GRADIENT.len() {
            GRADIENT[d as usize]
        } else {
            250
        };
        out.push_str(&format!("\x1b[38;5;{color}m{ch}"));
    }
    out.push_str(RESET);
    out
}

/// A single animated status line on stderr.
///
/// Cloning gives another handle to the same line (used to pause it around the
/// approval prompt). All drawing happens under one lock.
#[derive(Clone)]
pub struct StatusLine {
    state: Arc<Mutex<StatusState>>,
    enabled: bool,
}

impl StatusLine {
    fn new(enabled: bool, color: bool) -> Self {
        let state = Arc::new(Mutex::new(StatusState {
            activity: None,
            frame: 0,
            drawn: false,
            paused: false,
            color,
        }));
        if enabled {
            spawn_ticker(Arc::downgrade(&state));
        }
        Self { state, enabled }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StatusState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Show `label` (and an optional dimmed `detail`), restarting the timer.
    pub fn set(&self, label: &str, detail: Option<String>) {
        if !self.enabled {
            return;
        }
        let mut s = self.lock();
        s.activity = Some(Activity {
            label: label.to_string(),
            detail,
            started: Instant::now(),
        });
        s.draw();
    }

    /// Remove the status line. Returns once the line is erased.
    pub fn clear(&self) {
        let mut s = self.lock();
        s.activity = None;
        s.erase();
    }

    /// Print a permanent line above the status line.
    pub fn print_above(&self, line: &str) {
        let mut s = self.lock();
        s.erase();
        eprintln!("{line}");
        s.draw();
    }

    /// Stop drawing (e.g. while the user answers a prompt).
    pub fn pause(&self) {
        let mut s = self.lock();
        s.paused = true;
        s.erase();
    }

    /// Resume drawing after [`StatusLine::pause`].
    pub fn resume(&self) {
        let mut s = self.lock();
        s.paused = false;
        s.draw();
    }
}

/// Animate the status line until every handle to it is dropped.
fn spawn_ticker(state: Weak<Mutex<StatusState>>) {
    let run = async move {
        let mut tick = tokio::time::interval(Duration::from_millis(FRAME_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let Some(state) = state.upgrade() else {
                break;
            };
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if s.activity.is_some() && !s.paused {
                s.frame = s.frame.wrapping_add(1);
                s.draw();
            }
        }
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(run);
    }
}

// ---------------------------------------------------------------------------
// Agent UI
// ---------------------------------------------------------------------------

struct RunningTool {
    name: String,
    hint: Option<String>,
}

/// Per-session terminal UI for `zeptoclaw agent`.
pub struct AgentUi {
    caps: TermCaps,
    status: StatusLine,
    running: Vec<RunningTool>,
    tool_lines: usize,
    reply_started: bool,
    stream: Option<MarkdownStream>,
}

impl AgentUi {
    pub fn new() -> Self {
        Self::with_caps(TermCaps::detect())
    }

    fn with_caps(caps: TermCaps) -> Self {
        Self {
            caps,
            status: StatusLine::new(caps.stderr_tty, caps.color),
            running: Vec::new(),
            tool_lines: 0,
            reply_started: false,
            stream: None,
        }
    }

    /// Whether ANSI color is enabled.
    pub fn color(&self) -> bool {
        self.caps.color
    }

    /// A handle to the status line, for pausing it around prompts.
    pub fn status(&self) -> StatusLine {
        self.status.clone()
    }

    /// Print the interactive banner.
    pub fn print_banner(&self, model: &str) {
        let c = self.caps.color;
        println!();
        println!(
            "{INDENT}{} {} {}",
            paint(c, ACCENT, "◆"),
            paint(c, BOLD, "ZeptoClaw"),
            paint(c, DIM, &format!("{} · {model}", env!("CARGO_PKG_VERSION"))),
        );
        println!(
            "{INDENT}  {}",
            paint(
                c,
                FAINT,
                "/help commands · /model switch · /clear reset · ctrl-d quit"
            )
        );
        println!();
    }

    /// Reset per-turn state before a message is sent.
    pub fn begin_turn(&mut self) {
        self.running.clear();
        self.tool_lines = 0;
        self.reply_started = false;
        self.stream = None;
        if self.caps.stderr_tty {
            eprintln!();
        }
    }

    /// Render one agent feedback event.
    pub fn on_feedback(&mut self, fb: ToolFeedback) {
        match fb.phase {
            ToolFeedbackPhase::Thinking => self.status.set("Thinking", None),
            ToolFeedbackPhase::ThinkingDone | ToolFeedbackPhase::ResponseReady => {
                if self.running.is_empty() {
                    self.status.clear();
                }
            }
            ToolFeedbackPhase::Starting => {
                let hint = fb
                    .args_json
                    .as_deref()
                    .and_then(|a| extract_args_hint(&fb.tool_name, a));
                self.running.push(RunningTool {
                    name: fb.tool_name,
                    hint,
                });
                self.show_running();
            }
            ToolFeedbackPhase::Done { elapsed_ms } => {
                let hint = self.take_running(&fb.tool_name, fb.args_json.as_deref());
                let line =
                    format_tool_done(&fb.tool_name, hint.as_deref(), elapsed_ms, self.caps.color);
                self.emit_tool_line(&line);
            }
            ToolFeedbackPhase::Failed { elapsed_ms, error } => {
                let hint = self.take_running(&fb.tool_name, fb.args_json.as_deref());
                let line = format_tool_failed(
                    &fb.tool_name,
                    hint.as_deref(),
                    elapsed_ms,
                    &error,
                    self.caps.color,
                    term::width(),
                );
                self.emit_tool_line(&line);
            }
        }
    }

    fn take_running(&mut self, name: &str, args_json: Option<&str>) -> Option<String> {
        match self.running.iter().position(|t| t.name == name) {
            Some(i) => self.running.remove(i).hint,
            None => args_json.and_then(|a| extract_args_hint(name, a)),
        }
    }

    fn emit_tool_line(&mut self, line: &str) {
        self.tool_lines += 1;
        if self.caps.stderr_tty {
            self.status.print_above(line);
        } else {
            eprintln!("{line}");
        }
        self.show_running();
    }

    fn show_running(&self) {
        match self.running.as_slice() {
            [] => self.status.clear(),
            [tool] => self.status.set(&tool.name, tool.hint.clone()),
            many => self
                .status
                .set(&format!("Running {} tools", many.len()), None),
        }
    }

    /// Clear the status line and separate tool output from the reply.
    fn start_reply(&mut self) {
        if self.reply_started {
            return;
        }
        self.reply_started = true;
        self.running.clear();
        self.status.clear();
        if self.tool_lines > 0 && self.caps.stdout_tty {
            println!();
        }
    }

    fn renderer(&self) -> MarkdownRenderer {
        let width = term::text_width(term::width(), INDENT.len()) + INDENT.len();
        MarkdownRenderer::new(width, self.caps.color)
    }

    /// Print a complete reply: rendered on a terminal, verbatim when piped.
    pub fn print_reply(&mut self, text: &str) {
        self.start_reply();
        if self.caps.stdout_tty {
            let mut out = io::stdout().lock();
            for line in self.renderer().render(text) {
                let _ = writeln!(out, "{line}");
            }
        } else {
            println!("{text}");
        }
    }

    /// Print one streamed reply delta.
    pub fn stream_delta(&mut self, delta: &str) {
        self.start_reply();
        if !self.caps.stdout_tty {
            print!("{delta}");
            let _ = io::stdout().flush();
            return;
        }
        if self.stream.is_none() {
            self.stream = Some(MarkdownStream::new(self.renderer()));
        }
        if let Some(stream) = self.stream.as_mut() {
            let lines = stream.push(delta);
            let mut out = io::stdout().lock();
            for line in lines {
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
        }
    }

    /// Finish a streamed reply.
    pub fn stream_end(&mut self) {
        self.start_reply();
        match self.stream.take() {
            Some(mut stream) => {
                for line in stream.finish() {
                    println!("{line}");
                }
            }
            None if !self.caps.stdout_tty => println!(),
            None => {}
        }
    }

    /// Print an agent error with its guidance lines.
    pub fn print_error(&mut self, message: &str) {
        self.running.clear();
        self.status.clear();
        let c = self.caps.color;
        for (i, line) in message.lines().enumerate() {
            if i == 0 {
                eprintln!("{INDENT}{} {}", paint(c, RED, "✗"), paint(c, RED, line));
            } else if line.trim().is_empty() {
                eprintln!();
            } else {
                eprintln!("{INDENT}{}", paint(c, DIM, line));
            }
        }
    }

    /// Print the per-turn footer: model · tokens · tools · time.
    pub fn print_footer(&mut self, model: &str, tokens: u64, tool_calls: u64, elapsed: Duration) {
        self.status.clear();
        if !self.caps.stderr_tty {
            return;
        }
        eprintln!();
        eprintln!(
            "{INDENT}{}",
            format_footer(model, tokens, tool_calls, elapsed, self.caps.color)
        );
    }

    /// Print a blank line before the next prompt.
    pub fn end_turn(&self) {
        if self.caps.stderr_tty {
            eprintln!();
        }
    }
}

impl Drop for AgentUi {
    fn drop(&mut self) {
        self.status.clear();
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format elapsed time: milliseconds for fast ops, seconds for slower ones.
fn fmt_elapsed(elapsed_ms: u64) -> String {
    if elapsed_ms < 1000 {
        format!("{elapsed_ms}ms")
    } else if elapsed_ms < 60_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1000.0)
    } else {
        format!("{}m {}s", elapsed_ms / 60_000, (elapsed_ms % 60_000) / 1000)
    }
}

/// Compact token count: `842`, `5.3k`, `1.2M`.
fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Truncate `text` to at most `width` display columns, adding `…` when cut.
fn truncate_to_width(text: &str, width: usize) -> String {
    let text = text.replace(['\n', '\r', '\t'], " ");
    if text.width() <= width {
        return text;
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

fn tool_label(tool: &str, hint: Option<&str>, color: bool) -> String {
    let hint = hint
        .map(|h| paint(color, DIM, &format!(" → {h}")))
        .unwrap_or_default();
    format!("{tool}{hint}")
}

/// `  ✓ read_file → src/main.rs · 12ms`
pub fn format_tool_done(tool: &str, hint: Option<&str>, elapsed_ms: u64, color: bool) -> String {
    format!(
        "{INDENT}{} {}{}",
        paint(color, GREEN, "✓"),
        tool_label(tool, hint, color),
        paint(color, FAINT, &format!(" · {}", fmt_elapsed(elapsed_ms))),
    )
}

/// `  ✗ shell → cargo build · 5.0s · exit code 1`, truncated to the terminal.
pub fn format_tool_failed(
    tool: &str,
    hint: Option<&str>,
    elapsed_ms: u64,
    error: &str,
    color: bool,
    term_width: usize,
) -> String {
    let head_plain = format!(
        "{INDENT}✗ {tool}{} · {}",
        hint.map(|h| format!(" → {h}")).unwrap_or_default(),
        fmt_elapsed(elapsed_ms)
    );
    let room = term_width.saturating_sub(head_plain.width() + 4).max(20);
    let error = truncate_to_width(error.trim(), room);
    format!(
        "{INDENT}{} {}{}{}",
        paint(color, RED, "✗"),
        tool_label(tool, hint, color),
        paint(color, FAINT, &format!(" · {}", fmt_elapsed(elapsed_ms))),
        paint(color, RED, &format!(" · {error}")),
    )
}

/// `deepseek-flash · 5.3k tokens · 2 tools · 2.4s`
pub fn format_footer(
    model: &str,
    tokens: u64,
    tool_calls: u64,
    elapsed: Duration,
    color: bool,
) -> String {
    let mut parts = Vec::with_capacity(4);
    if !model.is_empty() {
        parts.push(model.to_string());
    }
    if tokens > 0 {
        parts.push(format!("{} tokens", fmt_tokens(tokens)));
    }
    if tool_calls > 0 {
        let noun = if tool_calls == 1 { "tool" } else { "tools" };
        parts.push(format!("{tool_calls} {noun}"));
    }
    parts.push(fmt_elapsed(elapsed.as_millis() as u64));
    let sep = if color {
        format!(" {FAINT}·{RESET}{DIM} ")
    } else {
        " · ".to_string()
    };
    paint(color, DIM, &parts.join(&sep))
}

/// Header for the approval prompt.
pub fn format_approval_header(tool: &str, color: bool) -> String {
    format!(
        "{INDENT}{} {} {}",
        paint(color, YELLOW, "⚠"),
        paint(color, BOLD, "Approval required"),
        paint(color, DIM, &format!("· {tool}")),
    )
}

/// Extract a short argument hint from tool call arguments JSON.
///
/// Looks for common keys like `path`, `command`, `action`, `query` and returns
/// the first value found, truncated to 50 characters.
pub fn extract_args_hint(_tool_name: &str, args_json: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(args_json).ok()?;
    let obj = val.as_object()?;

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
            if *key == "content" {
                return Some(format!("writing {} chars", s.chars().count()));
            }
            let s = s.replace(['\n', '\r', '\t'], " ");
            if s.chars().count() > 50 {
                let head: String = s.chars().take(50).collect();
                return Some(format!("{head}…"));
            }
            return Some(s);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feedback(tool: &str, phase: ToolFeedbackPhase, args: Option<&str>) -> ToolFeedback {
        ToolFeedback {
            tool_name: tool.to_string(),
            phase,
            args_json: args.map(str::to_string),
        }
    }

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
        assert_eq!(extract_args_hint("echo", r#"{"foo": "bar"}"#), None);
    }

    #[test]
    fn test_extract_args_hint_truncation() {
        let args = format!(r#"{{"path": "{}"}}"#, "a".repeat(60));
        let hint = extract_args_hint("read_file", &args).unwrap();
        assert_eq!(hint.chars().count(), 51);
        assert!(hint.ends_with('…'));
    }

    #[test]
    fn test_extract_args_hint_multibyte_truncation_does_not_panic() {
        let args = format!(r#"{{"path": "{}"}}"#, "é".repeat(60));
        let hint = extract_args_hint("read_file", &args).unwrap();
        assert!(hint.ends_with('…'));
        assert_eq!(hint.chars().count(), 51);
    }

    #[test]
    fn test_extract_args_hint_invalid_json() {
        assert_eq!(extract_args_hint("echo", "not json"), None);
    }

    #[test]
    fn test_extract_args_hint_prefers_path_over_content() {
        let args = r#"{"path": "file.py", "content": "def hello():\n    pass"}"#;
        assert_eq!(
            extract_args_hint("write_file", args),
            Some("file.py".to_string())
        );
    }

    #[test]
    fn test_extract_args_hint_flattens_newlines() {
        let args = r#"{"command": "echo a\necho b"}"#;
        assert_eq!(
            extract_args_hint("shell", args),
            Some("echo a echo b".to_string())
        );
    }

    #[test]
    fn test_format_tool_done_plain() {
        assert_eq!(
            format_tool_done("read_file", Some("main.rs"), 150, false),
            "  ✓ read_file → main.rs · 150ms"
        );
        assert_eq!(format_tool_done("echo", None, 3, false), "  ✓ echo · 3ms");
    }

    #[test]
    fn test_format_tool_failed_plain_and_truncated() {
        assert_eq!(
            format_tool_failed("shell", None, 5000, "exit code 1", false, 80),
            "  ✗ shell · 5.0s · exit code 1"
        );
        let long = "é".repeat(200);
        let line = format_tool_failed("shell", None, 100, &long, false, 60);
        assert!(line.ends_with('…'));
        assert!(line.width() <= 60, "{line}");
    }

    #[test]
    fn test_format_footer() {
        assert_eq!(
            format_footer(
                "deepseek-flash",
                5_255,
                2,
                Duration::from_millis(2_400),
                false
            ),
            "deepseek-flash · 5.3k tokens · 2 tools · 2.4s"
        );
        assert_eq!(
            format_footer("m", 0, 1, Duration::from_millis(80), false),
            "m · 1 tool · 80ms"
        );
    }

    #[test]
    fn test_fmt_elapsed() {
        assert_eq!(fmt_elapsed(0), "0ms");
        assert_eq!(fmt_elapsed(999), "999ms");
        assert_eq!(fmt_elapsed(1000), "1.0s");
        assert_eq!(fmt_elapsed(1500), "1.5s");
        assert_eq!(fmt_elapsed(125_000), "2m 5s");
    }

    #[test]
    fn test_fmt_tokens() {
        assert_eq!(fmt_tokens(842), "842");
        assert_eq!(fmt_tokens(5_255), "5.3k");
        assert_eq!(fmt_tokens(1_250_000), "1.2M");
    }

    #[test]
    fn test_truncate_to_width() {
        assert_eq!(truncate_to_width("short", 10), "short");
        assert_eq!(truncate_to_width("abcdefghij", 5), "abcd…");
        assert!(truncate_to_width("你好世界你好世界", 7).width() <= 7);
    }

    #[test]
    fn test_render_status_fits_width_and_shows_detail() {
        let activity = Activity {
            label: "read_file".to_string(),
            detail: Some("x".repeat(200)),
            started: Instant::now(),
        };
        let line = render_status(&activity, 0, false, 40);
        assert!(line.starts_with("  ⠋ read_file → "), "{line}");
        assert!(line.width() <= 40, "{line}");
    }

    #[test]
    fn test_feedback_tracks_running_tools_without_a_terminal() {
        let mut ui = AgentUi::with_caps(TermCaps::plain());
        ui.begin_turn();
        ui.on_feedback(feedback(
            "read_file",
            ToolFeedbackPhase::Starting,
            Some(r#"{"path":"a.rs"}"#),
        ));
        ui.on_feedback(feedback("shell", ToolFeedbackPhase::Starting, None));
        assert_eq!(ui.running.len(), 2);
        ui.on_feedback(feedback(
            "read_file",
            ToolFeedbackPhase::Done { elapsed_ms: 5 },
            Some(r#"{"path":"a.rs"}"#),
        ));
        assert_eq!(ui.running.len(), 1);
        assert_eq!(ui.running[0].name, "shell");
        ui.on_feedback(feedback(
            "shell",
            ToolFeedbackPhase::Failed {
                elapsed_ms: 9,
                error: "boom".to_string(),
            },
            None,
        ));
        assert!(ui.running.is_empty());
        assert_eq!(ui.tool_lines, 2);

        ui.begin_turn();
        assert_eq!(ui.tool_lines, 0, "tool count resets every turn");
    }
}
