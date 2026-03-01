//! DOCX text extraction tool.
//!
//! Always registered so the LLM knows it exists and can invoke it. Actual
//! extraction via zip + quick-xml requires no optional feature — both crates
//! are unconditional dependencies as of the DOCX support addition.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::error::{Result, ZeptoError};
use crate::security::validate_path_in_workspace;

use super::{Tool, ToolContext, ToolOutput};

/// Maximum DOCX file size accepted before extraction (50 MB).
const MAX_DOCX_BYTES: u64 = 50 * 1024 * 1024;

/// Default output character limit.
const DEFAULT_MAX_CHARS: usize = 50_000;

/// Maximum allowed `max_chars` value from LLM args.
const HARD_MAX_CHARS: usize = 200_000;

/// Extract plain text from a DOCX file in the workspace.
///
/// The tool is always registered; extraction uses the `zip` and `quick-xml`
/// crates which are unconditional dependencies.
pub struct DocxReadTool {
    workspace: String,
}

impl DocxReadTool {
    /// Create a new `DocxReadTool` bound to `workspace`.
    pub fn new(workspace: String) -> Self {
        Self { workspace }
    }

    /// Resolve and validate `path` to an absolute, workspace-bound DOCX path.
    ///
    /// Returns an error if:
    /// - The path escapes the workspace (path traversal).
    /// - The file does not have a `.docx` extension.
    /// - The file does not exist.
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let safe = validate_path_in_workspace(path, &self.workspace)?;
        if safe.as_path().extension().and_then(|e| e.to_str()) != Some("docx") {
            return Err(ZeptoError::Tool(
                "Only .docx files are supported".to_string(),
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

    /// Extract text from DOCX bytes.
    ///
    /// Opens the byte slice as a ZIP archive, reads `word/document.xml`, and
    /// walks the XML events to collect human-readable text:
    /// - `<w:t>` element text is appended as-is.
    /// - `</w:p>` (end of paragraph) inserts a newline.
    /// - `<w:tab/>` inserts a tab character.
    /// - `<w:br/>` inserts a newline.
    pub fn extract_text_from_bytes(bytes: &[u8]) -> Result<String> {
        use std::io::{Cursor, Read};
        use zip::ZipArchive;

        let cursor = Cursor::new(bytes);
        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| ZeptoError::Tool(format!("Failed to open DOCX as ZIP: {e}")))?;

        let mut xml_content = String::new();
        {
            let mut entry = archive.by_name("word/document.xml").map_err(|e| {
                ZeptoError::Tool(format!("word/document.xml not found in DOCX: {e}"))
            })?;
            entry
                .read_to_string(&mut xml_content)
                .map_err(|e| ZeptoError::Tool(format!("Failed to read word/document.xml: {e}")))?;
        }

        use quick_xml::events::Event;
        use quick_xml::Reader;

        let mut reader = Reader::from_str(&xml_content);
        reader.config_mut().trim_text(false);

        let mut output = String::new();
        let mut in_t = false;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    if e.local_name().as_ref() == b"t" {
                        in_t = true;
                    }
                }
                Ok(Event::Empty(ref e)) => match e.local_name().as_ref() {
                    b"tab" => output.push('\t'),
                    b"br" => output.push('\n'),
                    _ => {}
                },
                Ok(Event::End(ref e)) => {
                    if e.local_name().as_ref() == b"t" {
                        in_t = false;
                    } else if e.local_name().as_ref() == b"p" {
                        output.push('\n');
                    }
                }
                Ok(Event::Text(ref e)) => {
                    if in_t {
                        let text = e
                            .unescape()
                            .map_err(|e| ZeptoError::Tool(format!("XML unescape error: {e}")))?;
                        output.push_str(&text);
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(ZeptoError::Tool(format!("XML parse error: {e}")));
                }
                _ => {}
            }
            buf.clear();
        }

        Ok(output)
    }

    /// Extract text from the DOCX at `path`.
    ///
    /// Reads the file bytes with `std::fs::read()` and delegates to
    /// [`DocxReadTool::extract_text_from_bytes`].
    pub fn extract_text(path: &std::path::Path) -> Result<String> {
        let bytes = std::fs::read(path)
            .map_err(|e| ZeptoError::Tool(format!("Failed to read file: {e}")))?;
        Self::extract_text_from_bytes(&bytes)
    }
}

#[async_trait]
impl Tool for DocxReadTool {
    fn name(&self) -> &str {
        "docx_read"
    }

    fn description(&self) -> &str {
        "Extract plain text from a DOCX (Microsoft Word) file in the workspace. \
         Returns all readable text content including paragraphs and tables."
    }

    fn compact_description(&self) -> &str {
        "Extract plain text from a workspace DOCX file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the DOCX file within the workspace"
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
        if meta.len() > MAX_DOCX_BYTES {
            return Err(ZeptoError::Tool(format!(
                "DOCX too large: {} bytes (max {}MB)",
                meta.len(),
                MAX_DOCX_BYTES / 1024 / 1024
            )));
        }

        // Offload blocking zip+xml I/O off the async thread.
        let text = tokio::task::spawn_blocking(move || Self::extract_text(&resolved))
            .await
            .map_err(|e| ZeptoError::Tool(format!("Task panicked: {e}")))??;

        if text.trim().is_empty() {
            return Ok(ToolOutput::llm_only(
                "No text content found. The DOCX may be empty or image-only.",
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

    fn tool(workspace: &str) -> DocxReadTool {
        DocxReadTool::new(workspace.to_string())
    }

    // ---------------------------------------------------------------------------
    // Helper: build a minimal DOCX (ZIP) with given word/document.xml content.
    // ---------------------------------------------------------------------------
    fn build_test_docx(xml_content: &str) -> Vec<u8> {
        use std::io::Cursor;
        let buf = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        archive.start_file("word/document.xml", options).unwrap();
        std::io::Write::write_all(&mut archive, xml_content.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    // ---------------------------------------------------------------------------
    // Path validation tests (Task 2)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_rejects_path_outside_workspace() {
        let tmp = TempDir::new().unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("../../../etc/passwd");
        assert!(result.is_err(), "expected error for path traversal");
    }

    #[test]
    fn test_rejects_non_docx_extension() {
        let tmp = TempDir::new().unwrap();
        // Create the file so it exists; extension check should still fire.
        let txt_path = tmp.path().join("document.txt");
        std::fs::File::create(&txt_path).unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("document.txt");
        assert!(result.is_err(), "expected error for non-docx extension");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains(".docx"), "error should mention .docx: {msg}");
    }

    #[test]
    fn test_rejects_missing_file() {
        let tmp = TempDir::new().unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("missing.docx");
        assert!(result.is_err(), "expected error for missing file");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("not found") || msg.contains("missing"),
            "error should mention missing file: {msg}"
        );
    }

    #[test]
    fn test_accepts_valid_docx_path() {
        let tmp = TempDir::new().unwrap();
        let docx_path = tmp.path().join("report.docx");
        std::fs::File::create(&docx_path)
            .unwrap()
            .write_all(b"PK\x03\x04")
            .unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let result = t.resolve_path("report.docx");
        assert!(
            result.is_ok(),
            "expected Ok for valid docx path: {:?}",
            result
        );
    }

    // ---------------------------------------------------------------------------
    // Extraction tests (Task 3)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_extract_text_basic() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Hello World</w:t></w:r></w:p>
    <w:p><w:r><w:t>Second paragraph</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let bytes = build_test_docx(xml);
        let text = DocxReadTool::extract_text_from_bytes(&bytes).unwrap();
        assert!(
            text.contains("Hello World"),
            "should contain first paragraph text"
        );
        assert!(
            text.contains("Second paragraph"),
            "should contain second paragraph text"
        );
        // The two paragraphs must be separated by at least one newline.
        let hello_pos = text.find("Hello World").unwrap();
        let second_pos = text.find("Second paragraph").unwrap();
        let between = &text[hello_pos + "Hello World".len()..second_pos];
        assert!(
            between.contains('\n'),
            "paragraphs should be separated by newline, got: {:?}",
            between
        );
    }

    #[test]
    fn test_extract_text_empty_document() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
  </w:body>
</w:document>"#;

        let bytes = build_test_docx(xml);
        let text = DocxReadTool::extract_text_from_bytes(&bytes).unwrap();
        assert!(
            text.trim().is_empty(),
            "empty body should produce empty trimmed output, got: {:?}",
            text
        );
    }

    #[test]
    fn test_extract_text_with_tabs() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>Column A</w:t></w:r>
      <w:r><w:tab/></w:r>
      <w:r><w:t>Column B</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let bytes = build_test_docx(xml);
        let text = DocxReadTool::extract_text_from_bytes(&bytes).unwrap();
        assert!(
            text.contains('\t'),
            "tab element should produce \\t character"
        );
        assert!(text.contains("Column A"), "should contain Column A");
        assert!(text.contains("Column B"), "should contain Column B");
    }

    #[test]
    fn test_extract_text_multiple_runs() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>foo</w:t></w:r>
      <w:r><w:t>bar</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let bytes = build_test_docx(xml);
        let text = DocxReadTool::extract_text_from_bytes(&bytes).unwrap();
        // Both runs are in the same paragraph — text must be concatenated (no
        // separator between runs, only the paragraph-end newline after).
        assert!(
            text.contains("foobar"),
            "runs in same paragraph should be concatenated, got: {:?}",
            text
        );
    }

    #[tokio::test]
    async fn test_execute_extracts_text() {
        let tmp = TempDir::new().unwrap();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Integration test content</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let docx_bytes = build_test_docx(xml);
        let docx_path = tmp.path().join("test.docx");
        std::fs::write(&docx_path, &docx_bytes).unwrap();

        let t = tool(tmp.path().to_str().unwrap());
        let ctx = ToolContext::default();
        let result = t
            .execute(serde_json::json!({"path": "test.docx"}), &ctx)
            .await
            .unwrap();
        assert!(
            result.for_llm.contains("Integration test content"),
            "expected extracted text in for_llm, got: {:?}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_execute_missing_path_arg() {
        let tmp = TempDir::new().unwrap();
        let t = tool(tmp.path().to_str().unwrap());
        let ctx = ToolContext::default();
        let result = t.execute(serde_json::json!({}), &ctx).await;
        assert!(result.is_err(), "expected error when path arg is missing");
    }

    // ---------------------------------------------------------------------------
    // New gap-filling tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_extract_text_line_break() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>Line one</w:t></w:r>
      <w:r><w:br/></w:r>
      <w:r><w:t>Line two</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let bytes = build_test_docx(xml);
        let text = DocxReadTool::extract_text_from_bytes(&bytes).unwrap();
        assert!(
            text.contains("Line one\nLine two"),
            "<w:br/> should produce a newline between runs, got: {:?}",
            text
        );
    }

    #[test]
    fn test_truncate_output_long() {
        // 200 000 'a' chars — well above the 50 000 default limit.
        let long_text = "a".repeat(200_000);
        let result = DocxReadTool::truncate_output(long_text, DEFAULT_MAX_CHARS);
        assert!(
            result.contains("[TRUNCATED]"),
            "long output should be marked [TRUNCATED], got length {}",
            result.len()
        );
        // The result must be substantially smaller than the original 200 K.
        assert!(
            result.len() < 100_000,
            "truncated output should be well below 100 K chars, got {}",
            result.len()
        );
    }

    #[test]
    fn test_truncate_output_short() {
        let short_text = "hello world".to_string();
        let result = DocxReadTool::truncate_output(short_text.clone(), DEFAULT_MAX_CHARS);
        assert_eq!(
            result, short_text,
            "short text should pass through unchanged"
        );
        assert!(
            !result.contains("[TRUNCATED]"),
            "short text must not be marked [TRUNCATED]"
        );
    }

    #[test]
    fn test_truncate_output_multibyte() {
        // Each '日' is 3 bytes in UTF-8; 100 000 repetitions = 300 000 bytes.
        let cjk_text = "日".repeat(100_000);
        let result = DocxReadTool::truncate_output(cjk_text, DEFAULT_MAX_CHARS);
        assert!(
            result.contains("[TRUNCATED]"),
            "CJK text exceeding max_chars should be marked [TRUNCATED]"
        );
        // The body before the marker must be exactly DEFAULT_MAX_CHARS chars.
        let marker = "\n[TRUNCATED]";
        let body_end = result.find(marker).expect("[TRUNCATED] marker not found");
        let body = &result[..body_end];
        let char_count = body.chars().count();
        assert_eq!(
            char_count, DEFAULT_MAX_CHARS,
            "body before [TRUNCATED] must be exactly {DEFAULT_MAX_CHARS} chars, got {char_count}"
        );
    }

    #[test]
    fn test_extract_text_invalid_zip() {
        let not_a_zip = b"not a zip file";
        let result = DocxReadTool::extract_text_from_bytes(not_a_zip);
        assert!(result.is_err(), "non-ZIP bytes should return an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("zip"),
            "error message should mention ZIP, got: {msg}"
        );
    }

    #[test]
    fn test_extract_text_xml_entities() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Tom &amp; Jerry &lt;3</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        let bytes = build_test_docx(xml);
        let text = DocxReadTool::extract_text_from_bytes(&bytes).unwrap();
        assert!(
            text.contains("Tom & Jerry <3"),
            "&amp; and &lt; should be unescaped to & and <, got: {:?}",
            text
        );
    }

    #[tokio::test]
    async fn test_execute_empty_docx_returns_message() {
        let tmp = TempDir::new().unwrap();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
  </w:body>
</w:document>"#;
        let docx_bytes = build_test_docx(xml);
        let docx_path = tmp.path().join("empty.docx");
        std::fs::write(&docx_path, &docx_bytes).unwrap();

        let t = tool(tmp.path().to_str().unwrap());
        let ctx = ToolContext::default();
        let result = t
            .execute(serde_json::json!({"path": "empty.docx"}), &ctx)
            .await;
        assert!(result.is_ok(), "empty DOCX should not return an Err");
        let output = result.unwrap();
        assert!(
            output.for_llm.contains("No text content"),
            "empty DOCX should report 'No text content', got: {:?}",
            output.for_llm
        );
    }
}
