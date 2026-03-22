//! Unified diff parser and applier.
//!
//! Ported from pi-rs (pi-coding-agent). Applies standard unified diffs
//! with `@@ -old,count +new,count @@` hunk headers.

/// Summary of changes made when applying a diff.
#[derive(Debug, Default, PartialEq)]
pub struct DiffSummary {
    pub lines_added: usize,
    pub lines_removed: usize,
    pub hunks_applied: usize,
}

/// Parse and apply a unified diff to `original` text.
///
/// Supported format:
///   ```text
///   @@ -old_start,old_count +new_start,new_count @@
///    context line
///   -removed line
///   +added line
///   ```
///
/// Returns the modified text and a [`DiffSummary`] on success, or an error
/// message describing the mismatch on failure.
pub fn apply_unified_diff(original: &str, diff: &str) -> Result<(String, DiffSummary), String> {
    let orig_lines: Vec<&str> = split_lines_keep_terminator(original);
    let hunks = parse_hunks(diff)?;

    if hunks.is_empty() {
        return Err("diff contains no hunks".to_string());
    }

    let mut summary = DiffSummary::default();
    let mut out: Vec<String> = Vec::with_capacity(orig_lines.len());
    let mut orig_pos: usize = 0;

    for hunk in &hunks {
        let hunk_old_start = hunk.old_start;
        let copy_until = hunk_old_start.saturating_sub(1);
        while orig_pos < copy_until {
            if orig_pos >= orig_lines.len() {
                return Err(format!(
                    "hunk at old line {} references line {} which is past end of file ({} lines)",
                    hunk_old_start,
                    orig_pos + 1,
                    orig_lines.len()
                ));
            }
            out.push(orig_lines[orig_pos].to_string());
            orig_pos += 1;
        }

        for line in &hunk.lines {
            match line.kind {
                HunkLineKind::Context => {
                    if orig_pos >= orig_lines.len() {
                        return Err(format!(
                            "context mismatch: expected {:?} but reached end of file",
                            line.content
                        ));
                    }
                    let orig = strip_terminator(orig_lines[orig_pos]);
                    if orig != line.content {
                        return Err(format!(
                            "context mismatch at original line {}: expected {:?}, got {:?}",
                            orig_pos + 1,
                            line.content,
                            orig
                        ));
                    }
                    out.push(orig_lines[orig_pos].to_string());
                    orig_pos += 1;
                }
                HunkLineKind::Remove => {
                    if orig_pos >= orig_lines.len() {
                        return Err(format!(
                            "remove mismatch: expected {:?} but reached end of file",
                            line.content
                        ));
                    }
                    let orig = strip_terminator(orig_lines[orig_pos]);
                    if orig != line.content {
                        return Err(format!(
                            "remove mismatch at original line {}: expected {:?}, got {:?}",
                            orig_pos + 1,
                            line.content,
                            orig
                        ));
                    }
                    orig_pos += 1;
                    summary.lines_removed += 1;
                }
                HunkLineKind::Add => {
                    let mut s = line.content.clone();
                    if !s.ends_with('\n') && !s.ends_with('\r') {
                        s.push('\n');
                    }
                    out.push(s);
                    summary.lines_added += 1;
                }
            }
        }
        summary.hunks_applied += 1;
    }

    while orig_pos < orig_lines.len() {
        out.push(orig_lines[orig_pos].to_string());
        orig_pos += 1;
    }

    Ok((out.join(""), summary))
}

// --- Internal types ---

#[derive(Debug, Clone, Copy, PartialEq)]
enum HunkLineKind {
    Context,
    Remove,
    Add,
}

#[derive(Debug)]
struct HunkLine {
    kind: HunkLineKind,
    content: String,
}

#[derive(Debug)]
struct Hunk {
    old_start: usize,
    #[allow(dead_code)]
    old_count: usize,
    lines: Vec<HunkLine>,
}

fn split_lines_keep_terminator(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            lines.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn strip_terminator(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Hunk> = None;

    for raw_line in diff.lines() {
        if raw_line.starts_with("@@") {
            if let Some(h) = current.take() {
                validate_hunk(&h)?;
                hunks.push(h);
            }
            let (old_start, old_count) = parse_hunk_header(raw_line)?;
            current = Some(Hunk {
                old_start,
                old_count,
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current {
            let kind = if raw_line.starts_with('+') {
                HunkLineKind::Add
            } else if raw_line.starts_with('-') {
                HunkLineKind::Remove
            } else if raw_line.starts_with(' ') {
                HunkLineKind::Context
            } else {
                continue;
            };
            let content = raw_line[1..].to_string();
            hunk.lines.push(HunkLine { kind, content });
        }
    }

    if let Some(h) = current.take() {
        validate_hunk(&h)?;
        hunks.push(h);
    }

    Ok(hunks)
}

fn parse_hunk_header(line: &str) -> Result<(usize, usize), String> {
    let err = || format!("malformed hunk header: {:?}", line);
    let inner = line.trim_start_matches('@').trim_start_matches(' ');
    let old_part = inner
        .split_whitespace()
        .find(|s| s.starts_with('-'))
        .ok_or_else(err)?;
    let old_str = old_part.trim_start_matches('-');
    if old_str.contains(',') {
        let mut it = old_str.splitn(2, ',');
        let start: usize = it.next().unwrap().parse().map_err(|_| err())?;
        let count: usize = it.next().unwrap().parse().map_err(|_| err())?;
        Ok((start, count))
    } else {
        let start: usize = old_str.parse().map_err(|_| err())?;
        Ok((start, 1))
    }
}

fn validate_hunk(hunk: &Hunk) -> Result<(), String> {
    let actual_old = hunk
        .lines
        .iter()
        .filter(|l| matches!(l.kind, HunkLineKind::Context | HunkLineKind::Remove))
        .count();
    if hunk.old_count > 0 && actual_old != hunk.old_count {
        return Err(format!(
            "hunk at old line {} declares old_count={} but has {} context/remove lines",
            hunk.old_start, hunk.old_count, actual_old
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diff(hunks: &[&str]) -> String {
        hunks.join("\n")
    }

    #[test]
    fn simple_add_inserts_line() {
        let original = "line one\nline two\nline three\n";
        let diff = make_diff(&[
            "@@ -1,3 +1,4 @@",
            " line one",
            "+inserted line",
            " line two",
            " line three",
        ]);
        let (result, summary) = apply_unified_diff(original, &diff).unwrap();
        assert_eq!(result, "line one\ninserted line\nline two\nline three\n");
        assert_eq!(summary.lines_added, 1);
        assert_eq!(summary.lines_removed, 0);
        assert_eq!(summary.hunks_applied, 1);
    }

    #[test]
    fn simple_remove_deletes_line() {
        let original = "alpha\nbeta\ngamma\n";
        let diff = make_diff(&["@@ -1,3 +1,2 @@", " alpha", "-beta", " gamma"]);
        let (result, summary) = apply_unified_diff(original, &diff).unwrap();
        assert_eq!(result, "alpha\ngamma\n");
        assert_eq!(summary.lines_removed, 1);
    }

    #[test]
    fn replace_substitutes_line() {
        let original = "fn old_name() {\n    // body\n}\n";
        let diff = make_diff(&[
            "@@ -1,3 +1,3 @@",
            "-fn old_name() {",
            "+fn new_name() {",
            "     // body",
            " }",
        ]);
        let (result, _) = apply_unified_diff(original, &diff).unwrap();
        assert_eq!(result, "fn new_name() {\n    // body\n}\n");
    }

    #[test]
    fn multi_hunk_applies_both() {
        let original = (1..=10)
            .map(|i| format!("line {}\n", i))
            .collect::<String>();
        let diff = make_diff(&[
            "@@ -2,1 +2,1 @@",
            "-line 2",
            "+LINE TWO",
            "@@ -8,2 +8,3 @@",
            " line 8",
            "+extra line",
            " line 9",
        ]);
        let (result, summary) = apply_unified_diff(&original, &diff).unwrap();
        assert!(result.contains("LINE TWO"));
        assert!(result.contains("extra line"));
        assert_eq!(summary.hunks_applied, 2);
    }

    #[test]
    fn context_mismatch_returns_error() {
        let original = "foo\nbar\nbaz\n";
        let diff = make_diff(&["@@ -1,3 +1,3 @@", " foo", " WRONG", " baz"]);
        let err = apply_unified_diff(original, &diff).unwrap_err();
        assert!(err.contains("context mismatch"));
    }

    #[test]
    fn remove_mismatch_returns_error() {
        let original = "hello\nworld\n";
        let diff = make_diff(&["@@ -1,2 +1,1 @@", "-NONEXISTENT", " world"]);
        let err = apply_unified_diff(original, &diff).unwrap_err();
        assert!(err.contains("remove mismatch"));
    }

    #[test]
    fn empty_diff_returns_error() {
        let original = "some content\n";
        let diff = "--- a/file\n+++ b/file\n";
        let err = apply_unified_diff(original, diff).unwrap_err();
        assert!(err.contains("no hunks"));
    }

    #[test]
    fn pure_insertion_at_start() {
        let original = "existing\n";
        let diff = "@@ -0,0 +1,2 @@\n+first\n+second\n";
        let (result, summary) = apply_unified_diff(original, diff).unwrap();
        assert_eq!(result, "first\nsecond\nexisting\n");
        assert_eq!(summary.lines_added, 2);
    }

    #[test]
    fn diff_with_file_headers_ignored() {
        let original = "x\ny\n";
        let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n-x\n+X\n y\n";
        let (result, _) = apply_unified_diff(original, diff).unwrap();
        assert_eq!(result, "X\ny\n");
    }

    #[test]
    fn diff_on_file_without_trailing_newline() {
        let original = "a\nb\nc";
        let diff = make_diff(&["@@ -2,1 +2,1 @@", "-b", "+B"]);
        let (result, _) = apply_unified_diff(original, &diff).unwrap();
        assert!(result.contains("B\n"));
        assert!(result.contains('c'));
    }
}
