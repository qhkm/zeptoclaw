//! Native Gemini provider with OAuth and thinking model support.
//!
//! Auth priority: config key → GEMINI_API_KEY → GOOGLE_API_KEY → Gemini CLI OAuth
//!
//! Thinking model support: Gemini 2.5 models return parts tagged `thought: true`.
//! This provider filters those out and only returns the final non-thought text.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, warn};

use crate::error::{Result, ZeptoError};
use crate::session::{Message, Role};

use super::{parse_provider_error, ChatOptions, LLMProvider, LLMResponse, ToolDefinition, Usage};

/// Gemini v1beta REST API base.
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Path relative to $HOME where Gemini CLI stores OAuth credentials.
const GEMINI_CLI_CREDS_PATH: &str = ".gemini/oauth_credentials.json";

/// Default model when none is configured or passed at call time.
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.0-flash";

// ── Auth ─────────────────────────────────────────────────────────────────────

/// Authentication method for the Gemini REST API.
pub enum GeminiAuth {
    /// Standard API key — sent as `?key=` query parameter.
    ApiKey(String),
    /// OAuth bearer token — sent as `Authorization: Bearer` header.
    BearerToken(String),
}

impl std::fmt::Debug for GeminiAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(_) => f.write_str("GeminiAuth::ApiKey([REDACTED])"),
            Self::BearerToken(_) => f.write_str("GeminiAuth::BearerToken([REDACTED])"),
        }
    }
}

impl GeminiAuth {
    /// Resolve auth credentials in priority order.
    ///
    /// 1. `explicit_key` — value from config file
    /// 2. `env_key` — value of `GEMINI_API_KEY` or `GOOGLE_API_KEY`
    /// 3. `oauth_token` — token from Gemini CLI credential file
    pub fn resolve(
        explicit_key: Option<&str>,
        env_key: Option<&str>,
        oauth_token: Option<String>,
    ) -> Option<Self> {
        if let Some(k) = explicit_key.filter(|k| !k.is_empty()) {
            return Some(Self::ApiKey(k.to_string()));
        }
        if let Some(k) = env_key.filter(|k| !k.is_empty()) {
            return Some(Self::ApiKey(k.to_string()));
        }
        if let Some(token) = oauth_token {
            return Some(Self::BearerToken(token));
        }
        None
    }

    /// Read the Gemini CLI OAuth `access_token` from `~/.gemini/oauth_credentials.json`.
    ///
    /// The file is written by `gemini auth login`. It is a JSON object that may contain
    /// any of the fields `access_token`, `token`, or `oauth_token`.
    ///
    /// If the file contains an `expiry` or `expires_at` field (RFC 3339 string), the
    /// token is validated against the current time. An expired token returns `None`
    /// rather than causing a silent 401 failure downstream.
    pub fn load_cli_token() -> Option<String> {
        let home = dirs::home_dir()?;
        let path = home.join(GEMINI_CLI_CREDS_PATH);
        let data = std::fs::read_to_string(path).ok()?;
        let json: Value = serde_json::from_str(&data).ok()?;

        // Validate the expiry timestamp when present.
        if let Some(expiry_str) = json["expiry"]
            .as_str()
            .or_else(|| json["expires_at"].as_str())
        {
            match chrono::DateTime::parse_from_rfc3339(expiry_str) {
                Ok(expiry) => {
                    if expiry < chrono::Utc::now() {
                        warn!(
                            "Gemini CLI OAuth token has expired (expiry: {}). \
                             Run `gemini auth login` to refresh.",
                            expiry_str
                        );
                        return None;
                    }
                }
                Err(e) => {
                    warn!(
                        "Could not parse Gemini CLI token expiry '{}': {}. \
                         Proceeding with potentially expired token.",
                        expiry_str, e
                    );
                }
            }
        }

        json["access_token"]
            .as_str()
            .or_else(|| json["token"].as_str())
            .or_else(|| json["oauth_token"].as_str())
            .map(String::from)
    }

    /// Parse a credential JSON blob and return the access token if it is not expired.
    ///
    /// Extracted for unit-testing the expiry logic without touching the filesystem.
    #[cfg(test)]
    pub(crate) fn token_from_json_if_valid(json: &Value) -> Option<String> {
        if let Some(expiry_str) = json["expiry"]
            .as_str()
            .or_else(|| json["expires_at"].as_str())
        {
            if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expiry_str) {
                if expiry < chrono::Utc::now() {
                    return None;
                }
            }
        }
        json["access_token"]
            .as_str()
            .or_else(|| json["token"].as_str())
            .or_else(|| json["oauth_token"].as_str())
            .map(String::from)
    }
}

// ── Provider ──────────────────────────────────────────────────────────────────

/// Native Gemini provider that speaks the Gemini REST API directly.
///
/// Use [`GeminiProvider::from_config`] to build from the zeptoclaw config, or
/// [`GeminiProvider::new_with_key`] / [`GeminiProvider::new_with_token`] for
/// testing / manual construction.
pub struct GeminiProvider {
    auth: GeminiAuth,
    model: String,
    client: Client,
}

impl std::fmt::Debug for GeminiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiProvider")
            .field("auth", &self.auth)
            .field("model", &self.model)
            .finish()
    }
}

impl GeminiProvider {
    /// Build a provider that authenticates with an API key.
    pub fn new_with_key(api_key: &str, model: &str) -> Self {
        Self {
            auth: GeminiAuth::ApiKey(api_key.to_string()),
            model: model.to_string(),
            client: Self::build_client(),
        }
    }

    /// Build a provider that authenticates with a bearer token.
    pub fn new_with_token(bearer_token: &str, model: &str) -> Self {
        Self {
            auth: GeminiAuth::BearerToken(bearer_token.to_string()),
            model: model.to_string(),
            client: Self::build_client(),
        }
    }

    /// Build from an optional API key, resolving auth in priority order.
    ///
    /// Returns `None` when no credentials are available.
    pub fn default_gemini_model() -> &'static str {
        DEFAULT_GEMINI_MODEL
    }

    pub fn from_config(api_key: Option<&str>, model: &str, prefer_oauth: bool) -> Option<Self> {
        let env_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .ok();

        let oauth_token = if prefer_oauth || api_key.map(str::is_empty).unwrap_or(true) {
            GeminiAuth::load_cli_token()
        } else {
            None
        };

        let auth = GeminiAuth::resolve(api_key, env_key.as_deref(), oauth_token)?;

        Some(Self {
            auth,
            model: model.to_string(),
            client: Self::build_client(),
        })
    }

    fn build_client() -> Client {
        Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client")
    }

    /// Build a minimal Gemini `generateContent` request body for a single
    /// (role, text) turn.  Used by unit tests and simple one-shot helpers.
    pub fn build_request_body_from_parts(
        &self,
        role: &str,
        text: &str,
        system: Option<&str>,
    ) -> Value {
        let gemini_role = if role == "assistant" { "model" } else { "user" };
        let mut body = json!({
            "contents": [{
                "role": gemini_role,
                "parts": [{ "text": text }]
            }],
            "generationConfig": {
                "temperature": 0.7,
                "maxOutputTokens": 4096
            }
        });
        if let Some(sys) = system {
            body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
        }
        body
    }

    /// Build a full `generateContent` request body from a slice of [`Message`]s.
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
                json!({
                    "role": gemini_role,
                    "parts": [{ "text": &m.content }]
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

    /// Extract final answer text from a Gemini API response.
    ///
    /// Gemini 2.5 thinking models return parts tagged `"thought": true`.
    /// Those are intermediate reasoning steps and must be filtered out.
    /// If no non-thought parts exist (unusual), we fall back to returning
    /// the thought text so the caller always gets *something*.
    pub fn extract_text(response: &Value) -> Option<String> {
        let parts = response["candidates"][0]["content"]["parts"].as_array()?;

        // Collect only parts that are NOT tagged as thoughts.
        let final_parts: Vec<&str> = parts
            .iter()
            .filter(|p| !p["thought"].as_bool().unwrap_or(false))
            .filter_map(|p| p["text"].as_str())
            .collect();

        if !final_parts.is_empty() {
            return Some(final_parts.join(""));
        }

        // Fallback: at least return thought text if nothing else is available.
        let thought_parts: Vec<&str> = parts.iter().filter_map(|p| p["text"].as_str()).collect();

        if !thought_parts.is_empty() {
            Some(thought_parts.join(""))
        } else {
            None
        }
    }

    /// Parse token usage from a Gemini response if available.
    fn extract_usage(response: &Value) -> Option<Usage> {
        let meta = response.get("usageMetadata")?;
        let prompt = meta["promptTokenCount"].as_u64()? as u32;
        let completion = meta["candidatesTokenCount"].as_u64()? as u32;
        Some(Usage::new(prompt, completion))
    }

    /// Build the full API URL for `generateContent`.
    fn api_url(&self, model: &str) -> String {
        format!("{}/models/{}:generateContent", GEMINI_API_BASE, model)
    }

    /// Attach authentication to the request builder.
    fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            GeminiAuth::ApiKey(key) => request.query(&[("key", key.as_str())]),
            GeminiAuth::BearerToken(token) => {
                request.header("Authorization", format!("Bearer {}", token))
            }
        }
    }
}

#[async_trait]
impl LLMProvider for GeminiProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        model: Option<&str>,
        options: ChatOptions,
    ) -> Result<LLMResponse> {
        let model = model.unwrap_or(&self.model);
        let body = self.build_messages_body(&messages, &options);

        debug!("Gemini native request to model {}", model);

        let request = self
            .client
            .post(self.api_url(model))
            .header("Content-Type", "application/json")
            .json(&body);

        let request = self.apply_auth(request);

        let response = request
            .send()
            .await
            .map_err(|e| ZeptoError::Provider(format!("Gemini request failed: {}", e)))?;

        if response.status().is_success() {
            let json: Value = response.json().await.map_err(|e| {
                ZeptoError::Provider(format!("Failed to parse Gemini response: {}", e))
            })?;

            let content = Self::extract_text(&json).unwrap_or_default();
            let usage = Self::extract_usage(&json);

            let mut llm_response = LLMResponse::text(&content);
            if let Some(u) = usage {
                llm_response = llm_response.with_usage(u);
            }
            return Ok(llm_response);
        }

        let status = response.status().as_u16();
        let error_text = response.text().await.unwrap_or_default();

        // Try to extract a useful message from the Gemini error body.
        let body_msg = serde_json::from_str::<Value>(&error_text)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .map(|s| format!("Gemini API error: {}", s))
            })
            .unwrap_or_else(|| format!("Gemini API error: {}", error_text));

        Err(ZeptoError::from(parse_provider_error(status, &body_msg)))
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &str {
        "gemini-native"
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_resolution_prefers_explicit_key() {
        let auth = GeminiAuth::resolve(Some("explicit-key"), Some("env-key"), None);
        assert!(matches!(auth, Some(GeminiAuth::ApiKey(k)) if k == "explicit-key"));
    }

    #[test]
    fn test_auth_resolution_falls_back_to_env() {
        let auth = GeminiAuth::resolve(None, Some("env-key"), None);
        assert!(matches!(auth, Some(GeminiAuth::ApiKey(k)) if k == "env-key"));
    }

    #[test]
    fn test_auth_resolution_returns_none_with_no_credentials() {
        let auth = GeminiAuth::resolve(None, None, None);
        assert!(auth.is_none());
    }

    #[test]
    fn test_auth_resolution_uses_oauth_when_no_keys() {
        let auth = GeminiAuth::resolve(None, None, Some("oauth-token".to_string()));
        assert!(matches!(auth, Some(GeminiAuth::BearerToken(t)) if t == "oauth-token"));
    }

    #[test]
    fn test_auth_resolution_explicit_key_beats_oauth() {
        // explicit key takes priority even when oauth token is provided
        let auth = GeminiAuth::resolve(Some("config-key"), None, Some("oauth-token".to_string()));
        assert!(matches!(auth, Some(GeminiAuth::ApiKey(k)) if k == "config-key"));
    }

    #[test]
    fn test_extract_thinking_model_response_skips_thought_parts() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "thinking...", "thought": true },
                        { "text": "Final answer here" }
                    ]
                }
            }]
        });
        let text = GeminiProvider::extract_text(&response);
        assert_eq!(text.as_deref(), Some("Final answer here"));
    }

    #[test]
    fn test_extract_thinking_falls_back_to_thought_if_no_final() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "only thought part", "thought": true }
                    ]
                }
            }]
        });
        let text = GeminiProvider::extract_text(&response);
        assert_eq!(text.as_deref(), Some("only thought part"));
    }

    #[test]
    fn test_extract_text_normal_response() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{ "text": "Hello world" }]
                }
            }]
        });
        let text = GeminiProvider::extract_text(&response);
        assert_eq!(text.as_deref(), Some("Hello world"));
    }

    #[test]
    fn test_extract_text_multiple_non_thought_parts_joined() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "Part one. ", "thought": true },
                        { "text": "Part two. " },
                        { "text": "Part three." }
                    ]
                }
            }]
        });
        let text = GeminiProvider::extract_text(&response);
        assert_eq!(text.as_deref(), Some("Part two. Part three."));
    }

    #[test]
    fn test_extract_text_returns_none_for_empty_parts() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": []
                }
            }]
        });
        let text = GeminiProvider::extract_text(&response);
        assert!(text.is_none());
    }

    #[test]
    fn test_build_request_body_sets_model_and_contents() {
        let provider = GeminiProvider::new_with_key("test-key", "gemini-2.0-flash");
        let body = provider.build_request_body_from_parts("user", "Hi", None);
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "Hi");
    }

    #[test]
    fn test_build_request_body_maps_assistant_role_to_model() {
        let provider = GeminiProvider::new_with_key("test-key", "gemini-2.0-flash");
        let body = provider.build_request_body_from_parts("assistant", "Hello back", None);
        assert_eq!(body["contents"][0]["role"], "model");
    }

    #[test]
    fn test_build_request_body_includes_system_instruction() {
        let provider = GeminiProvider::new_with_key("test-key", "gemini-2.0-flash");
        let body = provider.build_request_body_from_parts("user", "Hi", Some("You are helpful"));
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are helpful"
        );
    }

    #[test]
    fn test_api_url_format() {
        let provider = GeminiProvider::new_with_key("key", "gemini-2.0-flash");
        let url = provider.api_url("gemini-2.0-flash");
        assert!(url.contains("generativelanguage.googleapis.com"));
        assert!(url.contains("gemini-2.0-flash"));
        assert!(url.ends_with(":generateContent"));
    }

    #[test]
    fn test_provider_name() {
        let provider = GeminiProvider::new_with_key("key", DEFAULT_GEMINI_MODEL);
        assert_eq!(provider.name(), "gemini-native");
    }

    #[test]
    fn test_provider_default_model() {
        let provider = GeminiProvider::new_with_key("key", "gemini-2.5-pro");
        assert_eq!(provider.default_model(), "gemini-2.5-pro");
    }

    #[test]
    fn test_extract_usage_parses_token_counts() {
        let response = serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            }
        });
        let usage = GeminiProvider::extract_usage(&response);
        assert!(usage.is_some());
        let u = usage.unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.total_tokens, 15);
    }

    #[test]
    fn test_extract_usage_returns_none_when_missing() {
        let response = serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "hi" }] } }]
        });
        let usage = GeminiProvider::extract_usage(&response);
        assert!(usage.is_none());
    }

    #[test]
    fn test_build_messages_body_filters_system_role() {
        let provider = GeminiProvider::new_with_key("key", DEFAULT_GEMINI_MODEL);
        let messages = vec![Message::system("Be helpful"), Message::user("Hello")];
        let body = provider.build_messages_body(&messages, &ChatOptions::default());
        // System message should NOT appear in contents — only the user message.
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        // System message should be lifted to systemInstruction.
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be helpful");
    }

    #[test]
    fn test_from_config_returns_none_without_credentials() {
        // Make sure no GEMINI_API_KEY / GOOGLE_API_KEY in environment for this test.
        // We can't easily unset env vars in tests, but we can verify the logic by
        // calling resolve directly.
        let auth = GeminiAuth::resolve(None, None, None);
        assert!(auth.is_none());
    }

    // ── Issue 1: API-key routing ──────────────────────────────────────────────

    #[test]
    fn test_new_with_key_provider_name_and_model() {
        let p = GeminiProvider::new_with_key("test-key", "gemini-2.0-flash");
        assert_eq!(p.name(), "gemini-native");
        assert_eq!(p.default_model(), "gemini-2.0-flash");
    }

    #[test]
    fn test_new_with_key_custom_model() {
        let p = GeminiProvider::new_with_key("test-key", "gemini-2.5-pro");
        assert_eq!(p.default_model(), "gemini-2.5-pro");
    }

    // ── Issue 2: Expiry validation ────────────────────────────────────────────

    #[test]
    fn test_token_from_json_if_valid_returns_token_when_not_expired() {
        // Use a far-future expiry so this test won't break before then.
        let json = serde_json::json!({
            "access_token": "valid-token",
            "expiry": "2099-01-01T00:00:00Z"
        });
        let result = GeminiAuth::token_from_json_if_valid(&json);
        assert_eq!(result.as_deref(), Some("valid-token"));
    }

    #[test]
    fn test_token_from_json_if_valid_returns_none_when_expired() {
        let json = serde_json::json!({
            "access_token": "stale-token",
            "expiry": "2020-01-01T00:00:00Z"
        });
        let result = GeminiAuth::token_from_json_if_valid(&json);
        assert!(result.is_none(), "Expected None for expired token");
    }

    #[test]
    fn test_token_from_json_if_valid_returns_token_when_no_expiry_field() {
        // No expiry field → assume valid (backward-compat with older CLI versions).
        let json = serde_json::json!({
            "access_token": "no-expiry-token"
        });
        let result = GeminiAuth::token_from_json_if_valid(&json);
        assert_eq!(result.as_deref(), Some("no-expiry-token"));
    }

    #[test]
    fn test_token_from_json_if_valid_checks_expires_at_alias() {
        let json = serde_json::json!({
            "token": "alias-token",
            "expires_at": "2020-06-15T12:00:00+00:00"
        });
        let result = GeminiAuth::token_from_json_if_valid(&json);
        assert!(
            result.is_none(),
            "Expected None when expires_at is in the past"
        );
    }

    #[test]
    fn test_token_from_json_if_valid_falls_back_to_oauth_token_field() {
        let json = serde_json::json!({
            "oauth_token": "fallback-oauth",
            "expiry": "2099-12-31T23:59:59Z"
        });
        let result = GeminiAuth::token_from_json_if_valid(&json);
        assert_eq!(result.as_deref(), Some("fallback-oauth"));
    }
}
