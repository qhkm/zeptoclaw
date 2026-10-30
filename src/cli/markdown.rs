//! Lightweight markdown renderer for agent replies in the terminal.
//!
//! Renders one source line at a time, so the same code serves streamed and
//! complete replies. Supports headings, emphasis, inline code, links, bullet,
//! numbered and task lists, block quotes, fenced code blocks, rules and tables
//! (passed through). Every output line is indented and word-wrapped by display
//! width, and control characters from model output are stripped so a reply
//! cannot emit its own terminal escape sequences.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Left margin for every rendered line, matching tool/status lines.
pub const INDENT: &str = "  ";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    dim: bool,
    code: bool,
    accent: bool,
    link: bool,
}

impl Style {
    const PLAIN: Style = Style {
        bold: false,
        italic: false,
        dim: false,
        code: false,
        accent: false,
        link: false,
    };

    fn dim() -> Self {
        Style {
            dim: true,
            ..Self::PLAIN
        }
    }

    fn accent() -> Self {
        Style {
            accent: true,
            ..Self::PLAIN
        }
    }

    fn sgr(self) -> String {
        let mut codes: Vec<&str> = Vec::new();
        if self.bold {
            codes.push("1");
        }
        if self.dim {
            codes.push("2");
        }
        if self.italic {
            codes.push("3");
        }
        if self.link {
            codes.push("4");
        }
        if self.code {
            codes.push("38;5;180");
        } else if self.accent || self.link {
            codes.push("38;5;117");
        }
        if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }
}

#[derive(Debug, Clone)]
struct Span {
    text: String,
    style: Style,
}

impl Span {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

/// Renders markdown source lines into indented, wrapped terminal lines.
pub struct MarkdownRenderer {
    width: usize,
    color: bool,
    in_code: bool,
    pending_blank: bool,
    emitted: bool,
}

impl MarkdownRenderer {
    /// `width` is the maximum visible columns of an output line, indent included.
    pub fn new(width: usize, color: bool) -> Self {
        Self {
            width: width.max(INDENT.len() + 10),
            color,
            in_code: false,
            pending_blank: false,
            emitted: false,
        }
    }

    /// Render a complete reply.
    pub fn render(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in text.lines() {
            out.extend(self.render_line(line));
        }
        out.extend(self.finish());
        out
    }

    /// Close any open block (an unterminated code fence) at the end of a reply.
    pub fn finish(&mut self) -> Vec<String> {
        if self.in_code {
            self.in_code = false;
            return vec![self.paint(&[Span::new(format!("{INDENT}╰─"), Style::dim())])];
        }
        Vec::new()
    }

    /// Render one source line (without its trailing newline).
    pub fn render_line(&mut self, raw: &str) -> Vec<String> {
        let line = sanitize(raw);
        let trimmed = line.trim_start();

        if let Some(rest) = trimmed
            .strip_prefix("```")
            .or_else(|| trimmed.strip_prefix("~~~"))
        {
            return if self.in_code {
                self.in_code = false;
                self.block(vec![
                    self.paint(&[Span::new(format!("{INDENT}╰─"), Style::dim())])
                ])
            } else {
                self.in_code = true;
                let lang = rest.trim();
                let label = if lang.is_empty() {
                    format!("{INDENT}╭─")
                } else {
                    format!("{INDENT}╭─ {lang}")
                };
                self.block(vec![self.paint(&[Span::new(label, Style::dim())])])
            };
        }

        if self.in_code {
            return self.block(self.code_line(&line));
        }

        if trimmed.is_empty() {
            if self.emitted {
                self.pending_blank = true;
            }
            return Vec::new();
        }

        let leading = leading_columns(&line);
        let lines = if let Some((level, text)) = heading(trimmed) {
            let style = Style {
                bold: true,
                accent: level <= 2,
                ..Style::PLAIN
            };
            self.wrap(&parse_inline(text, style), INDENT, INDENT)
        } else if is_rule(trimmed) {
            let n = (self.width - INDENT.len()).min(40);
            vec![self.paint(&[
                Span::new(INDENT, Style::PLAIN),
                Span::new("─".repeat(n), Style::dim()),
            ])]
        } else if trimmed.starts_with('|') {
            let style = if is_table_rule(trimmed) {
                Style::dim()
            } else {
                Style::PLAIN
            };
            let mut spans = vec![Span::new(INDENT, Style::PLAIN)];
            spans.extend(parse_inline(trimmed, style));
            vec![self.paint(&spans)]
        } else if let Some(text) = trimmed.strip_prefix('>') {
            let style = Style {
                italic: true,
                ..Style::PLAIN
            };
            self.wrap_prefixed(
                &parse_inline(text.trim_start(), style),
                &[
                    Span::new(INDENT, Style::PLAIN),
                    Span::new("│ ", Style::dim()),
                ],
                &[
                    Span::new(INDENT, Style::PLAIN),
                    Span::new("│ ", Style::dim()),
                ],
            )
        } else if let Some((marker, text)) = list_item(trimmed) {
            let depth = " ".repeat((leading / 2).min(4) * 2);
            let first = vec![
                Span::new(format!("{INDENT}{depth}"), Style::PLAIN),
                Span::new(marker.clone(), Style::accent()),
                Span::new(" ", Style::PLAIN),
            ];
            let hang = format!(
                "{INDENT}{depth}{}",
                " ".repeat(UnicodeWidthStr::width(marker.as_str()) + 1)
            );
            self.wrap_prefixed(
                &parse_inline(text, Style::PLAIN),
                &first,
                &[Span::new(hang, Style::PLAIN)],
            )
        } else {
            let prefix = format!("{INDENT}{}", " ".repeat(leading.min(8)));
            self.wrap(&parse_inline(trimmed, Style::PLAIN), &prefix, &prefix)
        };
        self.block(lines)
    }

    /// Emit a deferred blank separator before real content.
    fn block(&mut self, lines: Vec<String>) -> Vec<String> {
        if lines.is_empty() {
            return lines;
        }
        let mut out = Vec::with_capacity(lines.len() + 1);
        if self.pending_blank {
            out.push(String::new());
            self.pending_blank = false;
        }
        out.extend(lines);
        self.emitted = true;
        out
    }

    fn code_line(&self, line: &str) -> Vec<String> {
        let text = line.replace('\t', "    ");
        let gutter = [
            Span::new(INDENT, Style::PLAIN),
            Span::new("│ ", Style::dim()),
        ];
        let avail = self.width.saturating_sub(INDENT.len() + 2).max(10);
        let code = Style {
            code: true,
            ..Style::PLAIN
        };
        split_by_width(&text, avail)
            .into_iter()
            .map(|chunk| {
                let mut spans = gutter.to_vec();
                spans.push(Span::new(chunk, code));
                self.paint(&spans)
            })
            .collect()
    }

    fn wrap(&self, spans: &[Span], first: &str, rest: &str) -> Vec<String> {
        self.wrap_prefixed(
            spans,
            &[Span::new(first, Style::PLAIN)],
            &[Span::new(rest, Style::PLAIN)],
        )
    }

    /// Word-wrap `spans` to `self.width`, starting lines with the given prefixes.
    fn wrap_prefixed(&self, spans: &[Span], first: &[Span], rest: &[Span]) -> Vec<String> {
        let prefix_width = |p: &[Span]| p.iter().map(|s| s.text.width()).sum::<usize>();
        let mut lines: Vec<Vec<Span>> = Vec::new();
        let mut current: Vec<Span> = first.to_vec();
        let mut used = prefix_width(first);
        let mut has_word = false;
        let mut pending_space = false;

        for (word, style) in words(spans) {
            if word.is_empty() {
                pending_space = has_word;
                continue;
            }
            let word_width = word.width();
            let space = usize::from(pending_space);
            if has_word && used + space + word_width > self.width {
                lines.push(std::mem::replace(&mut current, rest.to_vec()));
                used = prefix_width(rest);
            } else if pending_space {
                current.push(Span::new(" ", Style::PLAIN));
                used += 1;
            }
            pending_space = false;

            let avail = self.width.saturating_sub(used).max(1);
            if word_width <= avail {
                current.push(Span::new(word, style));
                used += word_width;
            } else {
                // A single word wider than a whole line: hard-split it.
                let line_avail = self.width.saturating_sub(prefix_width(rest)).max(1);
                let mut chunks = split_by_width(&word, avail).into_iter();
                if let Some(head) = chunks.next() {
                    let tail = &word[head.len()..];
                    used += head.width();
                    current.push(Span::new(head, style));
                    for chunk in split_by_width(tail, line_avail) {
                        if chunk.is_empty() {
                            continue;
                        }
                        lines.push(std::mem::replace(&mut current, rest.to_vec()));
                        used = prefix_width(rest) + chunk.width();
                        current.push(Span::new(chunk, style));
                    }
                }
            }
            has_word = true;
        }
        lines.push(current);
        lines.iter().map(|l| self.paint(l)).collect()
    }

    /// Turn spans into a string, with ANSI styling when color is enabled.
    fn paint(&self, spans: &[Span]) -> String {
        let mut out = String::new();
        let mut open: Option<Style> = None;
        for span in spans {
            if span.text.is_empty() {
                continue;
            }
            if self.color && open != Some(span.style) {
                if open.is_some_and(|s| s != Style::PLAIN) {
                    out.push_str("\x1b[0m");
                }
                out.push_str(&span.style.sgr());
                open = Some(span.style);
            }
            out.push_str(&span.text);
        }
        if self.color && open.is_some_and(|s| s != Style::PLAIN) {
            out.push_str("\x1b[0m");
        }
        out.trim_end().to_string()
    }
}

/// Turns a stream of text deltas into rendered lines as each line completes.
pub struct MarkdownStream {
    renderer: MarkdownRenderer,
    buf: String,
}

impl MarkdownStream {
    pub fn new(renderer: MarkdownRenderer) -> Self {
        Self {
            renderer,
            buf: String::new(),
        }
    }

    /// Add a delta; returns the rendered lines for every line it completed.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            out.extend(
                self.renderer
                    .render_line(line.trim_end_matches(['\n', '\r'])),
            );
        }
        out
    }

    /// Flush the final partial line and close open blocks.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            out.extend(self.renderer.render_line(&line));
        }
        out.extend(self.renderer.finish());
        out
    }
}

/// Remove control characters (including ESC) except tabs.
fn sanitize(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect()
}

fn leading_columns(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn heading(trimmed: &str) -> Option<(usize, &str)> {
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&level) {
        let rest = &trimmed[level..];
        if rest.starts_with(' ') {
            return Some((level, rest.trim()));
        }
    }
    None
}

fn is_rule(trimmed: &str) -> bool {
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|m| compact.chars().all(|c| c == *m))
}

fn is_table_rule(trimmed: &str) -> bool {
    trimmed.contains('-')
        && trimmed
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '+'))
}

/// Returns the display marker and item text for bullet, task and numbered items.
fn list_item(trimmed: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(bullet) {
            let text = text.trim_start();
            for (task, mark) in [("[ ] ", "☐"), ("[x] ", "☑"), ("[X] ", "☑")] {
                if let Some(rest) = text.strip_prefix(task) {
                    return Some((mark.to_string(), rest));
                }
            }
            return Some(("•".to_string(), text));
        }
    }
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let rest = &trimmed[digits..];
        for sep in [". ", ") "] {
            if let Some(text) = rest.strip_prefix(sep) {
                let marker = format!("{}{}", &trimmed[..digits], &sep[..1]);
                return Some((marker, text.trim_start()));
            }
        }
    }
    None
}

/// Parse inline markdown (code, bold, italic, links, escapes) into styled spans.
fn parse_inline(text: &str, base: Style) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut spans = Vec::new();
    let mut cur = String::new();
    let mut style = base;
    let mut bold_ch: Option<char> = None;
    let mut italic_ch: Option<char> = None;
    let mut i = 0;

    let flush = |cur: &mut String, spans: &mut Vec<Span>, style: Style| {
        if !cur.is_empty() {
            spans.push(Span::new(std::mem::take(cur), style));
        }
    };

    while i < n {
        let c = chars[i];

        if c == '\\' && i + 1 < n && chars[i + 1].is_ascii_punctuation() {
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }

        if c == '`' {
            if let Some(end) = (i + 1..n).find(|&j| chars[j] == '`') {
                flush(&mut cur, &mut spans, style);
                let code: String = chars[i + 1..end].iter().collect();
                spans.push(Span::new(
                    code,
                    Style {
                        code: true,
                        ..style
                    },
                ));
                i = end + 1;
                continue;
            }
        }

        if (c == '*' || c == '_') && i + 1 < n && chars[i + 1] == c {
            if bold_ch == Some(c) {
                flush(&mut cur, &mut spans, style);
                style.bold = base.bold;
                bold_ch = None;
                i += 2;
                continue;
            }
            let opens = bold_ch.is_none()
                && i + 2 < n
                && !chars[i + 2].is_whitespace()
                && (i + 2..n.saturating_sub(1)).any(|j| chars[j] == c && chars[j + 1] == c);
            if opens {
                flush(&mut cur, &mut spans, style);
                style.bold = true;
                bold_ch = Some(c);
                i += 2;
                continue;
            }
        }

        if c == '*' || c == '_' {
            let word_before = i > 0 && chars[i - 1].is_alphanumeric();
            let word_after = i + 1 < n && chars[i + 1].is_alphanumeric();
            if italic_ch == Some(c) && !(c == '_' && word_after) {
                flush(&mut cur, &mut spans, style);
                style.italic = base.italic;
                italic_ch = None;
                i += 1;
                continue;
            }
            let opens = italic_ch.is_none()
                && !(c == '_' && word_before)
                && i + 1 < n
                && !chars[i + 1].is_whitespace()
                && (i + 2..n).any(|j| {
                    chars[j] == c
                        && !chars[j - 1].is_whitespace()
                        && !(c == '_' && j + 1 < n && chars[j + 1].is_alphanumeric())
                });
            if opens {
                flush(&mut cur, &mut spans, style);
                style.italic = true;
                italic_ch = Some(c);
                i += 1;
                continue;
            }
        }

        if c == '[' {
            if let Some((label, url, end)) = parse_link(&chars, i) {
                flush(&mut cur, &mut spans, style);
                let link_style = Style {
                    link: true,
                    ..style
                };
                spans.push(Span::new(label.clone(), link_style));
                if url != label {
                    spans.push(Span::new(format!(" ({url})"), Style::dim()));
                }
                i = end;
                continue;
            }
        }

        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut spans, style);
    spans
}

/// Parse `[label](url)` starting at `start`; returns label, url and end index.
fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = (start + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    let label: String = chars[start + 1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    if label.is_empty() || url.is_empty() || url.contains(' ') {
        return None;
    }
    Some((label, url, end + 1))
}

/// Split spans into words; an empty word marks whitespace between words.
///
/// Adjacent chunks with different styles and no whitespace between them (as in
/// `**bold**,`) stay separate words with no space marker, so they render touching.
fn words(spans: &[Span]) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = Vec::new();
    for span in spans {
        let mut word = String::new();
        for ch in span.text.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    out.push((std::mem::take(&mut word), span.style));
                }
                if out.last().is_none_or(|(w, _)| !w.is_empty()) {
                    out.push((String::new(), Style::PLAIN));
                }
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            out.push((word, span.style));
        }
    }
    out
}

/// Split `text` into chunks no wider than `width` display columns.
fn split_by_width(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(ch);
        used += w;
    }
    if !cur.is_empty() || chunks.is_empty() {
        chunks.push(cur);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(width: usize, text: &str) -> Vec<String> {
        MarkdownRenderer::new(width, false).render(text)
    }

    #[test]
    fn paragraphs_are_indented_and_wrapped() {
        let out = plain(20, "the quick brown fox jumps over the lazy dog");
        assert_eq!(
            out,
            vec!["  the quick brown", "  fox jumps over the", "  lazy dog"]
        );
        assert!(out.iter().all(|l| l.width() <= 20));
    }

    #[test]
    fn blank_lines_collapse_and_do_not_lead_or_trail() {
        let out = plain(40, "\n\nfirst\n\n\n\nsecond\n\n");
        assert_eq!(out, vec!["  first", "", "  second"]);
    }

    #[test]
    fn inline_markup_is_removed_in_plain_mode() {
        let out = plain(
            80,
            "use **bold**, *italic*, `code` and [docs](https://x.dev)",
        );
        assert_eq!(
            out,
            vec!["  use bold, italic, code and docs (https://x.dev)"]
        );
    }

    #[test]
    fn snake_case_and_lone_markers_are_left_alone() {
        let out = plain(80, "call my_func_name with 2 * 3 and a_b");
        assert_eq!(out, vec!["  call my_func_name with 2 * 3 and a_b"]);
    }

    #[test]
    fn lists_get_markers_and_hanging_indent() {
        let out = plain(
            24,
            "- first item that wraps around\n  - nested\n1. numbered\n- [x] done",
        );
        assert_eq!(
            out,
            vec![
                "  • first item that",
                "    wraps around",
                "    • nested",
                "  1. numbered",
                "  ☑ done",
            ]
        );
    }

    #[test]
    fn headings_quotes_and_rules() {
        let out = plain(40, "## Title\n> quoted text\n---");
        assert_eq!(out[0], "  Title");
        assert_eq!(out[1], "  │ quoted text");
        assert!(out[2].starts_with("  ───"));
    }

    #[test]
    fn code_blocks_are_boxed_and_not_reflowed() {
        let out = plain(40, "```rust\nfn main() {\n\tlet a = 1;\n}\n```");
        assert_eq!(
            out,
            vec![
                "  ╭─ rust",
                "  │ fn main() {",
                "  │     let a = 1;",
                "  │ }",
                "  ╰─",
            ]
        );
    }

    #[test]
    fn markdown_inside_code_blocks_is_literal() {
        let out = plain(40, "```\n# not a heading **x**\n```");
        assert_eq!(out[1], "  │ # not a heading **x**");
    }

    #[test]
    fn unterminated_code_block_is_closed() {
        let out = plain(40, "```\nlet x = 1;");
        assert_eq!(out.last().unwrap(), "  ╰─");
    }

    #[test]
    fn long_words_are_hard_split() {
        let out = plain(12, "abcdefghijklmnopqrstuvwxyz");
        assert!(out.len() > 1);
        assert!(out.iter().all(|l| l.width() <= 12), "{out:?}");
        let joined: String = out.iter().map(|l| l.trim()).collect();
        assert_eq!(joined, "abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn wide_characters_wrap_by_display_width() {
        let out = plain(12, "你好世界 你好世界 你好世界");
        assert!(out.iter().all(|l| l.width() <= 12), "{out:?}");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn control_characters_are_stripped() {
        let out = plain(80, "safe \x1b[31mred\x1b[0m text\x07");
        assert_eq!(out, vec!["  safe [31mred[0m text"]);
        assert!(!out[0].contains('\x1b'));
    }

    #[test]
    fn tables_pass_through_unwrapped() {
        let out = plain(10, "| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(out, vec!["  | a | b |", "  |---|---|", "  | 1 | 2 |"]);
    }

    #[test]
    fn color_mode_emits_and_resets_styles() {
        let out = MarkdownRenderer::new(80, true).render("a **b** `c`");
        let line = &out[0];
        assert!(line.contains("\x1b[1mb"));
        assert!(line.contains("\x1b[38;5;180mc"));
        assert!(line.ends_with("\x1b[0m"));
    }

    #[test]
    fn stream_renders_complete_lines_only() {
        let mut stream = MarkdownStream::new(MarkdownRenderer::new(40, false));
        assert!(stream.push("Hello wor").is_empty());
        assert_eq!(stream.push("ld\nsecond "), vec!["  Hello world"]);
        assert_eq!(stream.push("line"), Vec::<String>::new());
        assert_eq!(stream.finish(), vec!["  second line"]);
    }

    #[test]
    fn stream_matches_full_render() {
        let text = "# Plan\n\n- one\n- two\n\n```sh\nls -la\n```\nDone **now**.";
        let full = MarkdownRenderer::new(30, false).render(text);
        let mut stream = MarkdownStream::new(MarkdownRenderer::new(30, false));
        let mut streamed = Vec::new();
        for chunk in text.as_bytes().chunks(3) {
            streamed.extend(stream.push(std::str::from_utf8(chunk).unwrap()));
        }
        streamed.extend(stream.finish());
        assert_eq!(streamed, full);
    }
}
