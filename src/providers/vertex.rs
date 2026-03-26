//! Google Vertex AI provider.
//!
//! Uses the same `generateContent` request/response format as the Gemini
//! provider, but routes through the Vertex AI regional endpoint with
//! Application Default Credentials (ADC) for automatic token lifecycle.
//!
//! Auth priority:
//! 1. `VERTEX_ACCESS_TOKEN` env var (manual bearer token, no refresh)
//! 2. `GOOGLE_APPLICATION_CREDENTIALS` service account JSON (auto-refresh)
//! 3. `gcloud auth application-default login` ADC (auto-refresh)
//!
//! Endpoint:
//! `https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:generateContent`

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

use crate::error::{Result, ZeptoError};
use crate::session::{ContentPart, ImageSource, Message, Role};

use super::gemini::GeminiProvider;
use super::{parse_provider_error, ChatOptions, LLMProvider, LLMResponse, ToolDefinition};

/// Default model when none is configured or passed at call time.
const DEFAULT_VERTEX_MODEL: &str = "gemini-2.5-flash";

/// OAuth scope required for Vertex AI API access.
const VERTEX_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

// ── Auth ──────────────────────────────────────────────────────────────────────

/// Authentication method for Vertex AI.
enum VertexAuth {
    /// Static bearer token (from `VERTEX_ACCESS_TOKEN`). No auto-refresh.
    Static(String),
    /// ADC credential with automatic token refresh via `google-cloud-auth`.
    Adc(Arc<google_cloud_auth::credentials::AccessTokenCredentials>),
}

impl std::fmt::Debug for VertexAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(_) => f.write_str("VertexAuth::Static([REDACTED])"),
            Self::Adc(_) => f.write_str("VertexAuth::Adc"),
        }
    }
}

// ── Provider ──────────────────────────────────────────────────────────────────

/// Google Vertex AI provider that speaks the Gemini `generateContent` API
/// through the Vertex AI regional endpoint.
///
/// Supports both static bearer tokens and ADC with automatic refresh.
pub struct VertexProvider {
    project_id: String,
    location: String,
    auth: VertexAuth,
    model: String,
    client: Client,
}

impl std::fmt::Debug for VertexProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexProvider")
            .field("project_id", &self.project_id)
            .field("location", &self.location)
            .field("auth", &self.auth)
            .field("model", &self.model)
            .finish()
    }
}

impl VertexProvider {
    /// Build a provider with a static bearer token.
    pub fn new(project_id: &str, location: &str, bearer_token: &str, model: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            location: location.to_string(),
            auth: VertexAuth::Static(bearer_token.to_string()),
            model: model.to_string(),
            client: Self::build_client(),
        }
    }

    /// Build a provider with ADC (Application Default Credentials).
    ///
    /// The token is automatically refreshed before expiry by `google-cloud-auth`.
    pub fn with_adc(project_id: &str, location: &str, model: &str) -> Result<Self> {
        let cred = google_cloud_auth::credentials::Builder::default()
            .with_scopes([VERTEX_SCOPE])
            .build_access_token_credentials()
            .map_err(|e| ZeptoError::Provider(format!("Vertex AI ADC init failed: {e}")))?;

        Ok(Self {
            project_id: project_id.to_string(),
            location: location.to_string(),
            auth: VertexAuth::Adc(Arc::new(cred)),
            model: model.to_string(),
            client: Self::build_client(),
        })
    }

    /// Build from config and environment variables.
    ///
    /// Resolution priority:
    /// - **Project ID**: `api_key` config → `GOOGLE_CLOUD_PROJECT` → `VERTEX_PROJECT_ID`
    /// - **Location**: `api_base` config → `GOOGLE_CLOUD_LOCATION` → `VERTEX_LOCATION` → `us-central1`
    /// - **Auth**: `VERTEX_ACCESS_TOKEN` (static) → ADC (auto-refresh)
    /// - **Model**: config model → `gemini-2.5-flash`
    pub async fn from_config(
        api_key: Option<&str>,
        api_base: Option<&str>,
        model: &str,
    ) -> Option<Self> {
        // Resolve project ID
        let project_id = api_key
            .filter(|k| !k.is_empty())
            .map(String::from)
            .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok())
            .or_else(|| std::env::var("VERTEX_PROJECT_ID").ok())?;

        // Resolve location
        let location = api_base
            .filter(|b| !b.is_empty())
            .map(String::from)
            .or_else(|| std::env::var("GOOGLE_CLOUD_LOCATION").ok())
            .or_else(|| std::env::var("VERTEX_LOCATION").ok())
            .unwrap_or_else(|| "us-central1".to_string());

        let model = if model.is_empty() {
            DEFAULT_VERTEX_MODEL
        } else {
            model
        };

        // Try static token first (backward compat), then ADC
        if let Some(token) = std::env::var("VERTEX_ACCESS_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
        {
            return Some(Self::new(&project_id, &location, &token, model));
        }

        // Fall back to ADC (service account JSON or gcloud default credentials)
        match Self::with_adc(&project_id, &location, model) {
            Ok(provider) => Some(provider),
            Err(e) => {
                tracing::warn!("Vertex AI ADC not available: {e}");
                None
            }
        }
    }

    fn build_client() -> Client {
        Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client")
    }

    /// Get a valid bearer token, refreshing if needed for ADC.
    async fn get_token(&self) -> Result<String> {
        match &self.auth {
            VertexAuth::Static(token) => Ok(token.clone()),
            VertexAuth::Adc(cred) => {
                let token = cred
                    .access_token()
                    .await
                    .map_err(|e| ZeptoError::Provider(format!("Vertex AI token refresh: {e}")))?;
                Ok(token.token)
            }
        }
    }

    /// Build the full Vertex AI `generateContent` URL.
    fn api_url(&self, model: &str) -> String {
        format!(
            "https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:generateContent",
            location = self.location,
            project = self.project_id,
            model = model,
        )
    }

    /// Build a `generateContent` request body from a slice of [`Message`]s.
    ///
    /// The format is identical to the Gemini API: `contents` array with
    /// `role`/`parts`, optional `systemInstruction`, and `generationConfig`.
    fn build_messages_body(&self, messages: &[Message], options: &ChatOptions) -> Value {
        // Separate the system prompt (first System message) from the conversation.
        let system_prompt = messages
            .iter()
            .find(|m| m.role == Role::System)
            .map(|m| m.content.as_str());

        let contents: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| {
                let gemini_role = match m.role {
                    Role::Assistant => "model",
                    _ => "user",
                };
                let parts: Vec<Value> = if m.has_images() {
                    m.content_parts
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => Some(json!({ "text": text })),
                            ContentPart::Image { source, media_type } => {
                                if let ImageSource::Base64 { data } = source {
                                    Some(json!({
                                        "inlineData": {
                                            "mimeType": media_type,
                                            "data": data
                                        }
                                    }))
                                } else {
                                    None
                                }
                            }
                        })
                        .collect()
                } else {
                    vec![json!({ "text": &m.content })]
                };
                json!({
                    "role": gemini_role,
                    "parts": parts
                })
            })
            .collect();

        let mut generation_config = json!({});
        if let Some(max_tokens) = options.max_tokens {
            generation_config["maxOutputTokens"] = json!(max_tokens);
        }
        if let Some(temp) = options.temperature {
            generation_config["temperature"] = json!(temp);
        }
        if let Some(top_p) = options.top_p {
            generation_config["topP"] = json!(top_p);
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config
        });

        if let Some(sys) = system_prompt {
            body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
        }

        body
    }
}

#[async_trait]
impl LLMProvider for VertexProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        let model = model.unwrap_or(&self.model);
        let body = self.build_messages_body(&messages, &options);
        let token = self.get_token().await?;

        debug!("Vertex AI request to model {} in {}", model, self.location);

        let response = self
            .client
            .post(self.api_url(model))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
            .map_err(|e| ZeptoError::Provider(format!("Vertex AI request failed: {}", e)))?;

        if response.status().is_success() {
            let json: Value = response.json().await.map_err(|e| {
                ZeptoError::Provider(format!("Failed to parse Vertex AI response: {}", e))
            })?;

            // Reuse Gemini's response parsing — the format is identical.
            let content = GeminiProvider::extract_text(&json).unwrap_or_default();
            let usage = GeminiProvider::extract_usage(&json);

            let mut llm_response = LLMResponse::text(&content);
            if let Some(u) = usage {
                llm_response = llm_response.with_usage(u);
            }
            return Ok(llm_response);
        }

        let status = response.status().as_u16();
        let error_text = response.text().await.unwrap_or_default();

        // Try to extract a useful message from the Vertex AI error body.
        let body_msg = serde_json::from_str::<Value>(&error_text)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .map(|s| format!("Vertex AI API error: {}", s))
            })
            .unwrap_or_else(|| format!("Vertex AI API error: {}", error_text));

        Err(ZeptoError::from(parse_provider_error(status, &body_msg)))
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        "vertex"
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Message;

    #[test]
    fn test_constructor_sets_fields() {
        let provider =
            VertexProvider::new("my-project", "europe-west1", "token123", "gemini-2.5-pro");
        assert_eq!(provider.project_id, "my-project");
        assert_eq!(provider.location, "europe-west1");
        assert!(matches!(provider.auth, VertexAuth::Static(ref t) if t == "token123"));
        assert_eq!(provider.model, "gemini-2.5-pro");
    }

    #[test]
    fn test_provider_name() {
        let provider = VertexProvider::new("p", "us-central1", "t", DEFAULT_VERTEX_MODEL);
        assert_eq!(provider.name(), "vertex");
    }

    #[test]
    fn test_default_model() {
        let provider = VertexProvider::new("p", "us-central1", "t", "gemini-2.5-pro");
        assert_eq!(provider.default_model(), "gemini-2.5-pro");
    }

    #[test]
    fn test_api_url_construction() {
        let provider = VertexProvider::new("my-project", "us-central1", "t", "gemini-2.5-flash");
        let url = provider.api_url("gemini-2.5-flash");
        assert_eq!(
            url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn test_api_url_with_different_location() {
        let provider = VertexProvider::new("proj-123", "europe-west4", "t", "gemini-2.0-flash");
        let url = provider.api_url("gemini-2.0-flash");
        assert!(url.starts_with("https://europe-west4-aiplatform.googleapis.com/"));
        assert!(url.contains("projects/proj-123"));
        assert!(url.contains("locations/europe-west4"));
        assert!(url.ends_with(":generateContent"));
    }

    #[test]
    fn test_build_messages_body_basic_text() {
        let provider = VertexProvider::new("p", "us-central1", "t", DEFAULT_VERTEX_MODEL);
        let messages = vec![Message::user("Hello")];
        let body = provider.build_messages_body(&messages, &ChatOptions::default());

        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello");
    }

    #[test]
    fn test_build_messages_body_filters_system_prompt() {
        let provider = VertexProvider::new("p", "us-central1", "t", DEFAULT_VERTEX_MODEL);
        let messages = vec![Message::system("Be helpful"), Message::user("Hi")];
        let body = provider.build_messages_body(&messages, &ChatOptions::default());

        // System message should NOT appear in contents.
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");

        // System message should be lifted to systemInstruction.
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be helpful");
    }

    #[test]
    fn test_build_messages_body_maps_assistant_to_model() {
        let provider = VertexProvider::new("p", "us-central1", "t", DEFAULT_VERTEX_MODEL);
        let messages = vec![Message::user("Hi"), Message::assistant("Hello!")];
        let body = provider.build_messages_body(&messages, &ChatOptions::default());

        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
    }

    #[test]
    fn test_build_messages_body_generation_config() {
        let provider = VertexProvider::new("p", "us-central1", "t", DEFAULT_VERTEX_MODEL);
        let options = ChatOptions::new()
            .with_max_tokens(1024)
            .with_temperature(0.5)
            .with_top_p(0.75);
        let body = provider.build_messages_body(&[Message::user("Hi")], &options);

        let gen_config = &body["generationConfig"];
        assert_eq!(gen_config["maxOutputTokens"], 1024);
        assert_eq!(gen_config["temperature"], 0.5);
        assert_eq!(gen_config["topP"], 0.75);
    }

    #[test]
    fn test_extract_text_reuses_gemini_parser() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "thinking...", "thought": true },
                        { "text": "Final answer" }
                    ]
                }
            }]
        });
        let text = GeminiProvider::extract_text(&response);
        assert_eq!(text.as_deref(), Some("Final answer"));
    }

    #[test]
    fn test_extract_usage_reuses_gemini_parser() {
        let response = serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }],
            "usageMetadata": {
                "promptTokenCount": 42,
                "candidatesTokenCount": 10,
                "totalTokenCount": 52
            }
        });
        let usage = GeminiProvider::extract_usage(&response);
        assert!(usage.is_some());
        let u = usage.unwrap();
        assert_eq!(u.prompt_tokens, 42);
        assert_eq!(u.completion_tokens, 10);
        assert_eq!(u.total_tokens, 52);
    }

    #[test]
    fn test_no_system_instruction_when_no_system_message() {
        let provider = VertexProvider::new("p", "us-central1", "t", DEFAULT_VERTEX_MODEL);
        let messages = vec![Message::user("Hello")];
        let body = provider.build_messages_body(&messages, &ChatOptions::default());
        assert!(body.get("systemInstruction").is_none());
    }

    #[tokio::test]
    async fn test_get_token_static() {
        let provider = VertexProvider::new("p", "us-central1", "my-token", "m");
        let token = provider.get_token().await.unwrap();
        assert_eq!(token, "my-token");
    }

    #[test]
    fn test_static_auth_debug_redacted() {
        let auth = VertexAuth::Static("secret".to_string());
        let debug = format!("{:?}", auth);
        assert!(!debug.contains("secret"));
        assert!(debug.contains("REDACTED"));
    }
}
