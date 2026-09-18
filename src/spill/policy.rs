//! Spill policy: decide when output exceeds the inline budget, and format
//! what the model sees. Pure — performs no I/O so it can be tested exactly.

use std::path::PathBuf;

use crate::config::SpillConfig;

/// A spilled payload on disk.
#[derive(Debug, Clone)]
pub struct SpillRef {
    /// Absolute path the model can read.
    pub path: PathBuf,
    /// Line count of the full output.
    pub total_lines: usize,
    /// Byte count of the full output.
    pub total_bytes: usize,
}

/// True when `text` exceeds either inline budget.
pub fn needs_spill(text: &str, cfg: &SpillConfig) -> bool {
    text.len() > cfg.max_inline_bytes || text.split_inclusive('\n').count() > cfg.max_inline_lines
}

/// The leading slice kept in context, honoring both budgets and UTF-8 boundaries.
pub fn preview(text: &str, cfg: &SpillConfig) -> String {
    let mut out = String::new();
    for (i, segment) in text.split_inclusive('\n').enumerate() {
        if i >= cfg.max_inline_lines {
            break;
        }
        if out.len() + segment.len() > cfg.max_inline_bytes {
            let mut end = cfg.max_inline_bytes.saturating_sub(out.len());
            while end > 0 && !segment.is_char_boundary(end) {
                end -= 1;
            }
            out.push_str(&segment[..end]);
            break;
        }
        out.push_str(segment);
    }
    out
}

/// Preview + locator + retrieval guidance, as the model receives it.
pub fn format_with_locator(preview: &str, r: &SpillRef) -> String {
    let mut msg = String::from(preview);
    if !msg.ends_with('\n') {
        msg.push('\n');
    }
    msg.push_str(&format!(
        "... [output truncated. Full output: {} lines / {} bytes saved to {}\n\
         Read a slice: read_file(path, offset, limit) · Search: grep(pattern, path)]",
        r.total_lines,
        r.total_bytes,
        r.path.display()
    ));
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpillConfig;
    use std::path::PathBuf;

    fn cfg() -> SpillConfig {
        SpillConfig {
            max_inline_lines: 3,
            max_inline_bytes: 1000,
            ..Default::default()
        }
    }

    #[test]
    fn small_output_does_not_need_spill() {
        assert!(!needs_spill("a\nb\n", &cfg()));
    }

    #[test]
    fn output_over_line_budget_needs_spill() {
        assert!(needs_spill("a\nb\nc\nd\n", &cfg()));
    }

    #[test]
    fn output_over_byte_budget_needs_spill() {
        let c = SpillConfig {
            max_inline_lines: 9999,
            max_inline_bytes: 4,
            ..Default::default()
        };
        assert!(needs_spill("aaaaaaa\n", &c));
    }

    #[test]
    fn preview_respects_line_budget() {
        assert_eq!(preview("a\nb\nc\nd\n", &cfg()), "a\nb\nc\n");
    }

    #[test]
    fn preview_respects_utf8_boundaries() {
        let c = SpillConfig {
            max_inline_lines: 9999,
            max_inline_bytes: 6,
            ..Default::default()
        };
        let p = preview("\u{1F600}\u{1F600}\u{1F600}", &c);
        assert_eq!(p, "\u{1F600}");
    }

    #[test]
    fn formatted_message_carries_preview_path_and_guidance() {
        let r = SpillRef {
            path: PathBuf::from("/tmp/s/003-shell.txt"),
            total_lines: 12_431,
            total_bytes: 3_200_000,
        };
        let msg = format_with_locator("a\nb\n", &r);
        assert!(msg.starts_with("a\nb\n"));
        assert!(msg.contains("/tmp/s/003-shell.txt"));
        assert!(msg.contains("12431"));
        assert!(msg.contains("read_file"));
        assert!(msg.contains("grep"));
    }
}
