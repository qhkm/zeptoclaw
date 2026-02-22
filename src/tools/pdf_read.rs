//! PDF text extraction tool.
//!
//! Always registered so the LLM knows it exists and can invoke it. Actual
//! extraction via lopdf requires `--features tool-pdf`. Without the feature
//! the tool returns a clear, actionable "requires tool-pdf feature" error
//! rather than silently failing, keeping the default binary at ~4MB.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::error::{Result, ZeptoError};
use crate::security::validate_path_in_workspace;

use super::{Tool, ToolContext, ToolOutput};

/// Maximum PDF file size accepted before extraction (50 MB).
const MAX_PDF_BYTES: u64 = 50 * 1024 * 1024;

/// Default output character limit.
const DEFAULT_MAX_CHARS: usize = 50_000;

/// Maximum allowed `max_chars` value from LLM args.
const HARD_MAX_CHARS: usize = 200_000;

/// Extract plain text from a PDF file in the workspace.
///
/// The tool is always registered; extraction requires `--features tool-pdf`.
pub struct PdfReadTool {
    workspace: String,
}

impl PdfReadTool {
    /// Create a new `PdfReadTool` bound to `workspace`.
    pub fn new(workspace: String) -> Self {
        Self { workspace }
    }

    /// Resolve and validate `path` to an absolute, workspace-bound PDF path.
    ///
    /// Returns an error if:
    /// - The path escapes the workspace (path traversal).
    /// - The file does not have a `.pdf` extension.
    /// - The file does not exist.
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let safe = validate_path_in_workspace(path, &self.workspace)?;
        if safe.as_path().extension().and_then(|e| e.to_str()) != Some("pdf") {
            return Err(ZeptoError::Tool(
                "Only .pdf files are supported".to_string(),
            ));
        }
        if !safe.as_path().exists() {
            return Err(ZeptoError::Tool(format!("File not found: {path}")));
        }
        Ok(safe.into_path_buf())
    }

    /// Truncate `text` to at most `max_chars` characters.
    ///
    /// Uses char-aware slicing to avoid panicking on multi-byte UTF-8
    /// (e.g., Arabic, CJK, emoji). Appends a `[TRUNCATED]` marker when
    /// truncation occurs.
    pub fn truncate_output(text: String, max_chars: usize) -> String {
        // Fast path: byte length ≤ max_chars guarantees char count ≤ max_chars,
        // because every char is at least 1 byte.
        if text.len() <= max_chars {
            return text;
        }

        let mut byte_end = text.len();
        let mut truncated = false;

        for (char_count, (byte_idx, _ch)) in text.char_indices().enumerate() {
            if char_count == max_chars {
                byte_end = byte_idx;
                truncated = true;
                break;
            }
        }

        if truncated {
            let mut s = text[..byte_end].to_string();
            s.push_str("\n[TRUNCATED] — output exceeded max_chars");
            s
        } else {
            text
        }
    }

    /// Extract text from the PDF at `path` using lopdf.
    ///
    /// Requires `--features tool-pdf`. Without the feature this returns a
    /// clear error telling the user how to rebuild.
    #[cfg(feature = "tool-pdf")]
    fn extract_text(path: &std::path::Path) -> Result<String> {
        use lopdf::Document;
        let doc = Document::load(path)
            .map_err(|e| ZeptoError::Tool(format!("Failed to load PDF: {e}")))?;
        let mut text = String::new();
        for page_id in doc.page_iter() {
            if let Ok(page_text) = doc.extract_text(&[page_id.0]) {
                text.push_str(&page_text);
                text.push('\n');
            }
        }
        Ok(text)
    }

    #[cfg(not(feature = "tool-pdf"))]
    fn extract_text(_path: &std::path::Path) -> Result<String> {
        Err(ZeptoError::Tool(
            "PDF extraction requires the 'tool-pdf' build feature. \
             Rebuild with: cargo build --features tool-pdf"
                .to_string(),
        ))
    }
}

#[async_trait]
impl Tool for PdfReadTool {
    fn name(&self) -> &str {
        "pdf_read"
    }

    fn description(&self) -> &str {
        "Extract plain text from a PDF file in the workspace. \
         Returns all readable text content. \
         Image-only or encrypted PDFs may return empty results."
    }

    fn compact_description(&self) -> &str {
        "Extract plain text from a workspace PDF file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the PDF file within the workspace"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters to return (default: 50000, max: 200000)",
                    "default": DEFAULT_MAX_CHARS
                }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let path_str = args["path"].as_str().unwrap_or("");
        if path_str.is_empty() {
            return Err(ZeptoError::Tool(
                "Missing required argument: path".to_string(),
            ));
        }

        let max_chars = args["max_chars"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_CHARS)
            .min(HARD_MAX_CHARS);

        let resolved = self.resolve_path(path_str)?;

        // Size guard before we do any I/O-heavy work.
        let meta = tokio::fs::metadata(&resolved)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Cannot stat file: {e}")))?;
        if meta.len() > MAX_PDF_BYTES {
            return Err(ZeptoError::Tool(format!(
                "PDF too large: {} bytes (max {}MB)",
                meta.len(),
                MAX_PDF_BYTES / 1024 / 1024
            )));
        }

        // Offload blocking lopdf I/O off the async thread.
        let text = tokio::task::spawn_blocking(move || Self::extract_text(&resolved))
            .await
            .map_err(|e| ZeptoError::Tool(format!("Task panicked: {e}")))??;

        if text.trim().is_empty() {
            return Ok(ToolOutput::llm_only(
                "No text content found. The PDF may be image-only or encrypted.",
            ));
        }

        Ok(ToolOutput::llm_only(Self::truncate_output(text, max_chars)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn tool(workspace: &str) -> PdfReadTool {
        PdfReadTool::new(workspace.to_string())
    }

    #[test]
    fn test_rejects_path_outside_workspace() {
        let tmp = TempDir::new().unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("../../../etc/passwd");
        assert!(result.is_err(), "expected error for path traversal");
    }

    #[test]
    fn test_rejects_non_pdf_extension() {
        let tmp = TempDir::new().unwrap();
        // Create the file so it exists; extension check should still fire.
        let txt_path = tmp.path().join("document.txt");
        std::fs::File::create(&txt_path).unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("document.txt");
        assert!(result.is_err(), "expected error for non-pdf extension");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains(".pdf"), "error should mention .pdf: {msg}");
    }

    #[test]
    fn test_rejects_missing_file() {
        let tmp = TempDir::new().unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("missing.pdf");
        assert!(result.is_err(), "expected error for missing file");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("not found") || msg.contains("missing"),
            "error should mention missing file: {msg}"
        );
    }

    #[test]
    fn test_accepts_valid_pdf_path() {
        let tmp = TempDir::new().unwrap();
        let pdf_path = tmp.path().join("invoice.pdf");
        std::fs::File::create(&pdf_path)
            .unwrap()
            .write_all(b"%PDF-1.4")
            .unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("invoice.pdf");
        assert!(
            result.is_ok(),
            "expected Ok for valid pdf path: {:?}",
            result
        );
    }

    #[test]
    fn test_truncate_output() {
        let long = "a".repeat(200_000);
        let result = PdfReadTool::truncate_output(long, 50_000);
        // Result must be longer than 50_000 (due to the truncation marker) but
        // no more than 50_100 characters of meaningful overhead.
        assert!(
            result.len() <= 50_100,
            "truncated output too long: {}",
            result.len()
        );
        assert!(
            result.contains("[TRUNCATED]"),
            "truncated output missing marker"
        );
    }

    #[test]
    fn test_truncate_output_short() {
        let short = "hello world".to_string();
        let result = PdfReadTool::truncate_output(short.clone(), 50_000);
        assert_eq!(result, short, "short strings should be returned unchanged");
    }

    #[test]
    fn test_truncate_output_multibyte() {
        // Each '日' is 3 bytes. Slicing by byte index would panic at the char boundary.
        let long = "日".repeat(100_000);
        let result = PdfReadTool::truncate_output(long, 50_000);
        assert!(
            result.contains("[TRUNCATED]"),
            "should contain TRUNCATED marker"
        );
        // The body before the marker must be exactly 50_000 chars.
        let marker_pos = result
            .find('\n')
            .expect("should have newline before marker");
        let body = &result[..marker_pos];
        assert_eq!(
            body.chars().count(),
            50_000,
            "body should be exactly max_chars wide"
        );
    }
}
