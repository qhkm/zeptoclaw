//! WhatsApp Cloud API tool.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

const WHATSAPP_API_BASE: &str = "https://graph.facebook.com/v18.0";

/// Tool for sending WhatsApp messages through Cloud API.
pub struct WhatsAppTool {
    phone_number_id: String,
    access_token: String,
    default_language: String,
    client: Client,
}

impl WhatsAppTool {
    /// Create a new WhatsApp tool.
    pub fn new(phone_number_id: &str, access_token: &str) -> Self {
        Self {
            phone_number_id: phone_number_id.to_string(),
            access_token: access_token.to_string(),
            default_language: "ms".to_string(),
            client: Client::new(),
        }
    }

    /// Create with explicit default template language.
    pub fn with_default_language(
        phone_number_id: &str,
        access_token: &str,
        default_language: &str,
    ) -> Self {
        Self {
            phone_number_id: phone_number_id.to_string(),
            access_token: access_token.to_string(),
            default_language: default_language.to_string(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl Tool for WhatsAppTool {
    fn name(&self) -> &str {
        "whatsapp_send"
    }

    fn description(&self) -> &str {
        "Send a WhatsApp message to a phone number using WhatsApp Cloud API."
    }

    fn compact_description(&self) -> &str {
        "Send WhatsApp"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Messaging
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient phone number with country code, digits only (e.g. 60123456789)."
                },
                "message": {
                    "type": "string",
                    "description": "Message text for plain text send. Required when template is not provided."
                },
                "template": {
                    "type": "string",
                    "description": "Optional template name for pre-approved messages."
                },
                "template_params": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional template body parameters."
                },
                "language": {
                    "type": "string",
                    "description": "Template language code override (defaults to configured language)."
                }
            },
            "required": ["to"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let to = args
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'to' phone number".to_string()))?;

        if !to.chars().all(|c| c.is_ascii_digit()) {
            return Err(ZeptoError::Tool(
                "Phone number must contain digits only (country code included)".to_string(),
            ));
        }

        let template = args.get("template").and_then(Value::as_str).map(str::trim);
        let message = args.get("message").and_then(Value::as_str).map(str::trim);

        if template.unwrap_or("").is_empty() && message.unwrap_or("").is_empty() {
            return Err(ZeptoError::Tool(
                "Missing 'message' when no template is provided".to_string(),
            ));
        }

        let payload = if let Some(template_name) = template.filter(|s| !s.is_empty()) {
            let params = args
                .get("template_params")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|text| json!({ "type": "text", "text": text }))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let language = args
                .get("language")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(self.default_language.as_str());

            let components = if params.is_empty() {
                None
            } else {
                Some(vec![json!({ "type": "body", "parameters": params })])
            };

            json!({
                "messaging_product": "whatsapp",
                "recipient_type": "individual",
                "to": to,
                "type": "template",
                "template": {
                    "name": template_name,
                    "language": { "code": language },
                    "components": components
                }
            })
        } else {
            json!({
                "messaging_product": "whatsapp",
                "recipient_type": "individual",
                "to": to,
                "type": "text",
                "text": {
                    "preview_url": false,
                    "body": message.unwrap_or("")
                }
            })
        };

        let endpoint = format!("{}/{}/messages", WHATSAPP_API_BASE, self.phone_number_id);
        let response = self
            .client
            .post(endpoint)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("WhatsApp request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid WhatsApp response payload: {}", e)))?;

        if !status.is_success() {
            let detail = body
                .get("error")
                .and_then(Value::as_object)
                .and_then(|err| err.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Unknown API error");
            return Err(ZeptoError::Tool(format!(
                "WhatsApp API error {}: {}",
                status, detail
            )));
        }

        let message_id = body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        Ok(ToolOutput::llm_only(format!(
            "WhatsApp message sent to {} (id: {})",
            to, message_id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whatsapp_tool_properties() {
        let tool = WhatsAppTool::new("123", "token");
        assert_eq!(tool.name(), "whatsapp_send");
        assert!(tool.description().contains("WhatsApp"));
    }

    #[test]
    fn test_whatsapp_tool_parameters() {
        let tool = WhatsAppTool::new("123", "token");
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["to"].is_object());
    }

    #[tokio::test]
    async fn test_whatsapp_tool_missing_to() {
        let tool = WhatsAppTool::new("123", "token");
        let result = tool
            .execute(json!({"message":"hi"}), &ToolContext::new())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_whatsapp_tool_invalid_phone() {
        let tool = WhatsAppTool::new("123", "token");
        let result = tool
            .execute(json!({"to":"+6012","message":"hi"}), &ToolContext::new())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_whatsapp_tool_missing_message_for_text() {
        let tool = WhatsAppTool::new("123", "token");
        let result = tool
            .execute(json!({"to":"6012"}), &ToolContext::new())
            .await;
        assert!(result.is_err());
    }
}
