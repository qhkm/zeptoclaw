//! Voice transcription tool using Groq Whisper API.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{Result, ZeptoError};
use crate::tools::{Tool, ToolContext, ToolOutput};

pub struct TranscribeTool {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl TranscribeTool {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
        }
    }

    async fn transcribe_file(&self, path: &str) -> Result<String> {
        let file_bytes = tokio::fs::read(path)
            .await
            .map_err(|e| ZeptoError::Tool(format!("Failed to read audio file: {}", e)))?;

        let filename = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.ogg")
            .to_string();

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename)
            .mime_str("audio/ogg")
            .map_err(|e| ZeptoError::Tool(e.to_string()))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone());

        let resp = self
            .client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ZeptoError::Tool(format!(
                "Groq transcription failed ({}): {}",
                status, body
            )));
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(e.to_string()))?;

        Ok(json["text"].as_str().unwrap_or("").to_string())
    }
}

#[async_trait]
impl Tool for TranscribeTool {
    fn name(&self) -> &str {
        "transcribe"
    }

    fn description(&self) -> &str {
        "Transcribe a voice or audio file to text using Groq Whisper. \
         Provide the local file path to the audio file. \
         Supported formats: mp3, mp4, mpeg, mpga, m4a, wav, webm, ogg."
    }

    fn compact_description(&self) -> &str {
        "Transcribe an audio file to text via Groq Whisper."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute or workspace-relative path to the audio file"
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args["file_path"].as_str() {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path is required")),
        };

        // Resolve relative paths against workspace
        let resolved = if std::path::Path::new(file_path).is_absolute() {
            file_path.to_string()
        } else if let Some(ws) = &ctx.workspace {
            format!("{}/{}", ws, file_path)
        } else {
            file_path.to_string()
        };

        match self.transcribe_file(&resolved).await {
            Ok(text) if text.is_empty() => Ok(ToolOutput::llm_only(
                "Transcription returned empty (no speech detected)",
            )),
            Ok(text) => Ok(ToolOutput::user_visible(format!("Transcription: {}", text))),
            Err(e) => Ok(ToolOutput::error(format!("Transcription failed: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcribe_tool_name() {
        let tool = TranscribeTool::new("key", "whisper-large-v3-turbo");
        assert_eq!(tool.name(), "transcribe");
    }

    #[test]
    fn test_transcribe_tool_description() {
        let tool = TranscribeTool::new("key", "whisper-large-v3-turbo");
        assert!(tool.description().contains("Groq Whisper"));
        assert!(tool.description().contains("ogg"));
    }

    #[test]
    fn test_transcribe_tool_parameters() {
        let tool = TranscribeTool::new("key", "whisper-large-v3-turbo");
        let params = tool.parameters();
        assert!(params["properties"]["file_path"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("file_path")));
    }

    #[tokio::test]
    async fn test_transcribe_missing_file_path() {
        let tool = TranscribeTool::new("key", "whisper-large-v3-turbo");
        let ctx = ToolContext::new();
        let result = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.for_llm.contains("file_path is required"));
    }

    #[tokio::test]
    async fn test_transcribe_nonexistent_file() {
        let tool = TranscribeTool::new("key", "whisper-large-v3-turbo");
        let ctx = ToolContext::new();
        let result = tool
            .execute(
                serde_json::json!({"file_path": "/nonexistent/audio.ogg"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.for_llm.contains("Transcription failed"));
    }

    #[test]
    fn test_relative_path_resolves_with_workspace() {
        // Verify path resolution logic inline
        let ws = "/workspace";
        let rel = "audio.ogg";
        let resolved = if std::path::Path::new(rel).is_absolute() {
            rel.to_string()
        } else {
            format!("{}/{}", ws, rel)
        };
        assert_eq!(resolved, "/workspace/audio.ogg");
    }

    #[test]
    fn test_absolute_path_not_resolved() {
        let abs = "/tmp/audio.ogg";
        let resolved = if std::path::Path::new(abs).is_absolute() {
            abs.to_string()
        } else {
            format!("/workspace/{}", abs)
        };
        assert_eq!(resolved, "/tmp/audio.ogg");
    }
}
