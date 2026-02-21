//! Provider-agnostic audio transcription service.
//!
//! Tries each configured OpenAI-compatible provider in order until one succeeds.
//! Falls back to `[Voice Message]` if all fail or none are configured.

use reqwest::multipart;
use tracing::{debug, warn};

use crate::config::Config;
use crate::providers::{provider_config_by_name, PROVIDER_REGISTRY};

/// A single transcription endpoint candidate.
#[derive(Debug, Clone)]
pub struct TranscriptionCandidate {
    pub provider_name: String,
    pub api_key: String,
    pub api_base: String,
}

/// Service that transcribes audio bytes using configured OpenAI-compatible providers.
///
/// Built from config at startup. Tries providers in registry order, skipping
/// Anthropic (which has no audio API). Falls back to `[Voice Message]` on total failure.
#[derive(Debug, Clone)]
pub struct TranscriberService {
    candidates: Vec<TranscriptionCandidate>,
    model: String,
    client: reqwest::Client,
}

impl TranscriberService {
    /// Build from config. Skips providers with `backend == "anthropic"`.
    /// Returns `None` if transcription is disabled or no eligible providers are configured.
    pub fn from_config(config: &Config) -> Option<Self> {
        if !config.transcription.enabled {
            return None;
        }

        let candidates: Vec<TranscriptionCandidate> = PROVIDER_REGISTRY
            .iter()
            .filter(|spec| spec.backend == "openai")
            .filter_map(|spec| {
                let pc = provider_config_by_name(config, spec.name)?;
                let api_key = pc.api_key.clone()?;
                let api_base = pc
                    .api_base
                    .clone()
                    .or_else(|| spec.default_base_url.map(|s| s.to_string()))
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                Some(TranscriptionCandidate {
                    provider_name: spec.name.to_string(),
                    api_key,
                    api_base,
                })
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }

        Some(Self {
            candidates,
            model: config.transcription.model.clone(),
            client: reqwest::Client::new(),
        })
    }

    /// Transcribe raw audio bytes. Returns transcript or `"[Voice Message]"` on total failure.
    pub async fn transcribe(&self, audio: Vec<u8>, content_type: &str) -> String {
        for candidate in &self.candidates {
            match self
                .try_transcribe(candidate, audio.clone(), content_type)
                .await
            {
                Ok(text) => {
                    debug!(provider = %candidate.provider_name, "Transcription succeeded");
                    return text;
                }
                Err(e) => {
                    warn!(
                        provider = %candidate.provider_name,
                        error = %e,
                        "Transcription failed, trying next provider"
                    );
                }
            }
        }
        "[Voice Message]".to_string()
    }

    async fn try_transcribe(
        &self,
        candidate: &TranscriptionCandidate,
        audio: Vec<u8>,
        content_type: &str,
    ) -> Result<String, String> {
        let file_part = multipart::Part::bytes(audio)
            .file_name("voice.ogg")
            .mime_str(content_type)
            .map_err(|e| e.to_string())?;

        let form = multipart::Form::new()
            .part("file", file_part)
            .text("model", self.model.clone());

        let url = format!("{}/audio/transcriptions", candidate.api_base);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&candidate.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let body = resp.text().await.map_err(|e| e.to_string())?;
        let trimmed = body.trim();

        // Some providers return JSON {"text": "..."}, others return plain text.
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(text) = json.get("text").and_then(|v| v.as_str()) {
                return Ok(text.to_string());
            }
        }

        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    fn make_config_with_provider(name: &str, api_key: &str, api_base: &str) -> Config {
        let mut config = Config::default();
        let pc = ProviderConfig {
            api_key: Some(api_key.to_string()),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        match name {
            "openai" => config.providers.openai = Some(pc),
            "groq" => config.providers.groq = Some(pc),
            "anthropic" => config.providers.anthropic = Some(pc),
            _ => {}
        }
        config
    }

    #[test]
    fn test_from_config_openai_included() {
        let config = make_config_with_provider("openai", "sk-test", "https://api.openai.com/v1");
        let svc = TranscriberService::from_config(&config).unwrap();
        assert_eq!(svc.candidates.len(), 1);
        assert_eq!(svc.candidates[0].provider_name, "openai");
    }

    #[test]
    fn test_from_config_groq_included() {
        let config =
            make_config_with_provider("groq", "gsk_test", "https://api.groq.com/openai/v1");
        let svc = TranscriberService::from_config(&config).unwrap();
        assert_eq!(svc.candidates[0].provider_name, "groq");
    }

    #[test]
    fn test_from_config_anthropic_excluded() {
        let config =
            make_config_with_provider("anthropic", "sk-ant-test", "https://api.anthropic.com");
        let svc = TranscriberService::from_config(&config);
        assert!(
            svc.is_none(),
            "Anthropic should be excluded from transcription"
        );
    }

    #[test]
    fn test_from_config_disabled() {
        let mut config =
            make_config_with_provider("openai", "sk-test", "https://api.openai.com/v1");
        config.transcription.enabled = false;
        assert!(TranscriberService::from_config(&config).is_none());
    }

    #[test]
    fn test_from_config_no_providers() {
        let config = Config::default();
        assert!(TranscriberService::from_config(&config).is_none());
    }

    #[test]
    fn test_from_config_uses_configured_model() {
        let mut config =
            make_config_with_provider("openai", "sk-test", "https://api.openai.com/v1");
        config.transcription.model = "whisper-large-v3".to_string();
        let svc = TranscriberService::from_config(&config).unwrap();
        assert_eq!(svc.model, "whisper-large-v3");
    }

    #[tokio::test]
    async fn test_transcribe_empty_candidates_returns_fallback() {
        let svc = TranscriberService {
            candidates: vec![],
            model: "whisper-1".to_string(),
            client: reqwest::Client::new(),
        };
        let result = svc.transcribe(vec![1, 2, 3], "audio/ogg").await;
        assert_eq!(result, "[Voice Message]");
    }
}
