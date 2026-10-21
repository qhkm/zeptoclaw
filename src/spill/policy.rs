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

/// Opening text of the locator block appended by [`format_with_locator`].
///
/// Also the needle [`split_locator`] looks for, so the two cannot drift.
const LOCATOR_MARKER: &str = "... [output truncated. Full output: ";

/// Upper bound on what [`split_locator`] will treat as a locator. A locator
/// is one short block; anything larger is tool output that happens to
/// contain the marker.
const MAX_LOCATOR_BYTES: usize = 4_096;

/// Floor on the preview budget once the locator's bytes are reserved. The
/// locator is the whole point of spilling, so it is preferred over the
/// preview when the two cannot both fit.
const MIN_PREVIEW_BUDGET: usize = 256;

/// Preview + locator + retrieval guidance, as the model receives it.
pub fn format_with_locator(preview: &str, r: &SpillRef) -> String {
    let mut msg = String::from(preview);
    if !msg.ends_with('\n') {
        msg.push('\n');
    }
    msg.push_str(&format!(
        "{LOCATOR_MARKER}{} lines / {} bytes saved to {}\n\
         Read a slice: read_file(path, offset, limit) · Search: grep(pattern, path)]",
        r.total_lines,
        r.total_bytes,
        r.path.display()
    ));
    msg
}

/// Split a formatted result into `(preview, locator)`, or `None` when it
/// carries no locator.
///
/// Conservative on purpose: the marker must start a line, the block must be
/// the last thing in the string, it must still be intact (`]`-terminated),
/// and it must be short. Ordinary tool output that merely quotes the marker
/// is left alone.
pub fn split_locator(result: &str) -> Option<(&str, &str)> {
    let idx = result.rfind(LOCATOR_MARKER)?;
    if idx > 0 && !result[..idx].ends_with('\n') {
        return None;
    }
    let locator = &result[idx..];
    if locator.len() > MAX_LOCATOR_BYTES || !locator.ends_with(']') {
        return None;
    }
    Some((&result[..idx], locator))
}

/// Sanitize a tool result, guaranteeing that a spill locator survives.
///
/// [`crate::utils::sanitize::sanitize_tool_result`] keeps a 10 KB head and a
/// 2 KB tail. The tail budget is `max_bytes - head`, so once the dynamic
/// budget falls to 10 KB or below — which is what happens as the context
/// fills, the exact condition spill exists for — the tail is empty and the
/// locator, which sits at the *end* of [`format_with_locator`]'s output, is
/// cut entirely. The file is written, the path never reaches the model, and
/// retention later evicts an orphan nobody could read.
///
/// The locator is split off, the preview is sanitized against a budget that
/// reserves room for it, and the locator is reattached verbatim. Keeping it
/// out of `sanitize_tool_result` also means the base64/hex scrubbers can
/// never rewrite bytes of the path.
pub fn sanitize_preserving_locator(result: &str, max_bytes: usize) -> String {
    let Some((preview, locator)) = split_locator(result) else {
        return crate::utils::sanitize::sanitize_tool_result(result, max_bytes);
    };

    // +1 for the newline that joins preview and locator.
    let preview_budget = max_bytes
        .saturating_sub(locator.len() + 1)
        .max(MIN_PREVIEW_BUDGET);

    let mut out = crate::utils::sanitize::sanitize_tool_result(preview, preview_budget);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(locator);
    out
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

    fn big_spill_ref() -> SpillRef {
        SpillRef {
            path: PathBuf::from("/home/u/.zeptoclaw/sessions/cli%3Acli/spill/003-shell.txt"),
            total_lines: 12_431,
            total_bytes: 3_200_000,
        }
    }

    // CRITICAL #2 regression: `sanitize_tool_result` keeps a 10 KB head and
    // a 2 KB tail, and the tail budget is `max_bytes - head`. Once the
    // dynamic budget drops to 10 KB or below — what happens as the context
    // fills, i.e. exactly when spill matters — the tail is empty and the
    // locator, which sits at the END of the formatted message, is cut. The
    // file is written and the path never reaches the model.
    #[test]
    fn locator_survives_sanitization_at_the_minimum_budget() {
        let r = big_spill_ref();
        let body: String = (1..=4_000).map(|i| format!("line {i}\n")).collect();
        let msg = format_with_locator(&body, &r);

        // Demonstrate the bug this guards against.
        let naive = crate::utils::sanitize::sanitize_tool_result(
            &msg,
            crate::utils::sanitize::MIN_RESULT_BUDGET,
        );
        assert!(
            !naive.contains("003-shell.txt"),
            "plain sanitization is expected to drop the locator here"
        );

        let kept = sanitize_preserving_locator(&msg, crate::utils::sanitize::MIN_RESULT_BUDGET);
        assert!(
            kept.contains("003-shell.txt"),
            "locator must survive the minimum budget: {kept}"
        );
        assert!(kept.contains("read_file"));
        assert!(kept.contains("grep"));
        assert!(kept.starts_with("line 1\n"));
    }

    #[test]
    fn locator_survives_every_budget_between_min_and_max() {
        let r = big_spill_ref();
        let body: String = (1..=4_000).map(|i| format!("line {i}\n")).collect();
        let msg = format_with_locator(&body, &r);

        for budget in [
            crate::utils::sanitize::MIN_RESULT_BUDGET,
            2_048,
            4_000,
            10_240,
            12_288,
            crate::utils::sanitize::DEFAULT_MAX_RESULT_BYTES,
        ] {
            let kept = sanitize_preserving_locator(&msg, budget);
            assert!(
                kept.contains("003-shell.txt"),
                "locator dropped at budget {budget}"
            );
        }
    }

    #[test]
    fn results_without_a_locator_sanitize_unchanged() {
        let plain = "x".repeat(40_000);
        assert_eq!(
            sanitize_preserving_locator(&plain, 4_096),
            crate::utils::sanitize::sanitize_tool_result(&plain, 4_096)
        );
        assert_eq!(sanitize_preserving_locator("hello", 4_096), "hello");
    }

    #[test]
    fn split_locator_round_trips_a_formatted_message() {
        let r = big_spill_ref();
        let msg = format_with_locator("a\nb\n", &r);
        let (preview, locator) = split_locator(&msg).expect("locator present");
        assert_eq!(preview, "a\nb\n");
        assert!(locator.starts_with("... [output truncated. Full output: "));
        assert!(locator.ends_with(']'));
        assert_eq!(format!("{preview}{locator}"), msg);
    }

    #[test]
    fn split_locator_ignores_output_that_merely_quotes_the_marker() {
        // Mid-line, so not a locator block.
        assert!(split_locator("see ... [output truncated. Full output: x]").is_none());
        // Unterminated: an already-truncated locator must not be reattached.
        assert!(split_locator("a\n... [output truncated. Full output: 1 lines").is_none());
        // Oversized: real tool output that happens to contain the marker.
        let huge = format!(
            "a\n... [output truncated. Full output: {}]",
            "x".repeat(5_000)
        );
        assert!(split_locator(&huge).is_none());
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
