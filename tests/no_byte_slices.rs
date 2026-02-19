use anyhow::Context;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

// This test fails if source files contain literal-range string slices like
// `text[..50]` or `to_string()[..8]` which can panic on UTF-8 boundaries.
// It's intentionally conservative: it only looks for numeric literal ranges
// (e.g. `[..50]` or `[0..50]`) to avoid flagging slices where the end is a
// variable (e.g. `&buf[..n]`).

fn visit_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir failed: {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_rs_files(&path, out)?;
        } else if let Some(ext) = path.extension() {
            if ext == "rs" {
                out.push(path);
            }
        }
    }
    Ok(())
}

#[test]
fn no_literal_byte_index_string_slices() -> anyhow::Result<()> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = repo_root.join("src");

    // Patterns to detect: `[..123]`, `[123..]`, `.to_string()[..8]`, `.to_string()[123..]`
    let re_literal_range = Regex::new(r"\[\s*\.\.{2}\s*\d+\s*\]").unwrap();
    let re_leading_range = Regex::new(r"\[\s*\d+\s*\.\.{2}\s*\]").unwrap();
    let re_to_string_slice = Regex::new(r"to_string\(\)\s*\[\s*\.\.{2}\s*\d+\s*\]").unwrap();
    let mut failures: Vec<String> = Vec::new();

    let mut files: Vec<PathBuf> = Vec::new();
    visit_rs_files(&src_dir, &mut files)?;

    for path in files {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        for (i, line) in text.lines().enumerate() {
            if re_literal_range.is_match(line)
                || re_leading_range.is_match(line)
                || re_to_string_slice.is_match(line)
            {
                failures.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "Found unsafe literal-range slices in source files:\n{}",
            failures.join("\n")
        );
    }

    Ok(())
}
