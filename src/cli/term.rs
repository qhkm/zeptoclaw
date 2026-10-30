//! Terminal capability detection for the interactive CLI.
//!
//! Decides whether stdout/stderr are terminals, whether to emit ANSI color
//! (honouring `NO_COLOR` and `TERM=dumb`), and how wide the terminal is.

use std::io::IsTerminal;

/// Fallback width when the terminal size cannot be read.
const DEFAULT_WIDTH: usize = 80;

/// Widest column count the reply text is wrapped to, so long lines stay readable
/// on very wide terminals.
pub const MAX_TEXT_WIDTH: usize = 100;

/// What the current process can render to its output streams.
#[derive(Debug, Clone, Copy)]
pub struct TermCaps {
    /// stdout is an interactive terminal (render markdown, wrap lines).
    pub stdout_tty: bool,
    /// stderr is an interactive terminal (draw spinner and status lines).
    pub stderr_tty: bool,
    /// ANSI color/style escapes are allowed.
    pub color: bool,
}

impl TermCaps {
    /// Detect capabilities from the real process environment.
    pub fn detect() -> Self {
        let stdout_tty = std::io::stdout().is_terminal();
        let stderr_tty = std::io::stderr().is_terminal();
        let color = color_allowed(
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("TERM").ok().as_deref(),
        ) && (stdout_tty || stderr_tty);
        Self {
            stdout_tty,
            stderr_tty,
            color,
        }
    }

    /// Capabilities for plain, non-terminal output (tests and pipes).
    #[cfg(test)]
    pub fn plain() -> Self {
        Self {
            stdout_tty: false,
            stderr_tty: false,
            color: false,
        }
    }
}

/// Color is allowed unless `NO_COLOR` is set or the terminal is `dumb`.
fn color_allowed(no_color: bool, term: Option<&str>) -> bool {
    !no_color && term != Some("dumb")
}

/// Current terminal width in columns.
///
/// Reads the window size from stdout, then stderr, then `$COLUMNS`, falling
/// back to 80.
pub fn width() -> usize {
    terminal_columns()
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|c| c.trim().parse::<usize>().ok())
        })
        .filter(|w| *w >= 20)
        .unwrap_or(DEFAULT_WIDTH)
}

#[cfg(unix)]
fn terminal_columns() -> Option<usize> {
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: `winsize` is plain old data; TIOCGWINSZ only writes into it,
        // and a failed ioctl leaves it zeroed, which is rejected below.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
        if rc == 0 && ws.ws_col > 0 {
            return Some(ws.ws_col as usize);
        }
    }
    None
}

#[cfg(not(unix))]
fn terminal_columns() -> Option<usize> {
    None
}

/// Width available for reply text: the terminal width minus the left indent and
/// a right margin, capped at [`MAX_TEXT_WIDTH`].
pub fn text_width(term_width: usize, indent: usize) -> usize {
    term_width
        .saturating_sub(indent + 2)
        .clamp(20, MAX_TEXT_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_respects_no_color_and_dumb_term() {
        assert!(color_allowed(false, Some("xterm-256color")));
        assert!(color_allowed(false, None));
        assert!(!color_allowed(true, Some("xterm-256color")));
        assert!(!color_allowed(false, Some("dumb")));
    }

    #[test]
    fn text_width_is_clamped() {
        assert_eq!(text_width(80, 2), 76);
        assert_eq!(text_width(300, 2), MAX_TEXT_WIDTH);
        assert_eq!(text_width(10, 2), 20);
    }
}
