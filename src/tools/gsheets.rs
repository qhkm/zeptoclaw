//! Google Sheets API tool.

use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde_json::{json, Value};

use crate::error::{Result, ZeptoError};

use super::{Tool, ToolCategory, ToolContext, ToolOutput};

const SHEETS_API_BASE: &str = "https://sheets.googleapis.com/v4/spreadsheets";

/// Tool for reading/writing Google Sheets ranges.
#[derive(Debug)]
pub struct GoogleSheetsTool {
    client: Client,
    access_token: String,
}

impl GoogleSheetsTool {
    /// Create with an OAuth access token.
    pub fn new(access_token: &str) -> Self {
        Self {
            client: Client::new(),
            access_token: access_token.to_string(),
        }
    }

    /// Parse a base64-encoded JSON payload to extract an `access_token`.
    ///
    /// This supports workflows where a short-lived token is provisioned in config.
    pub fn from_service_account(encoded_json: &str) -> Result<Self> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded_json)
            .map_err(|e| ZeptoError::Config(format!("Invalid base64 credentials: {}", e)))?;
        let payload: Value = serde_json::from_slice(&decoded)
            .map_err(|e| ZeptoError::Config(format!("Invalid credentials JSON: {}", e)))?;
        let token = payload
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| {
                ZeptoError::Config(
                    "Service account payload must include non-empty 'access_token'".to_string(),
                )
            })?;
        Ok(Self::new(token))
    }

    fn extract_values(args: &Value) -> Result<Vec<Vec<String>>> {
        let rows = args
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| ZeptoError::Tool("Missing 'values' for write operation".to_string()))?;

        let mut parsed = Vec::new();
        for row in rows {
            let row_values = row
                .as_array()
                .ok_or_else(|| ZeptoError::Tool("Each row must be an array".to_string()))?
                .iter()
                .map(|cell| {
                    cell.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| cell.to_string())
                })
                .collect::<Vec<_>>();
            parsed.push(row_values);
        }
        Ok(parsed)
    }

    async fn execute_read(&self, spreadsheet_id: &str, range: &str) -> Result<String> {
        let encoded_range = encode_range(range);
        let endpoint = format!(
            "{}/{}/values/{}",
            SHEETS_API_BASE, spreadsheet_id, encoded_range
        );
        let response = self
            .client
            .get(endpoint)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Google Sheets request failed: {}", e)))?;

        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Sheets response payload: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Google Sheets API error {}: {}",
                status, body
            )));
        }

        let values = body
            .get("values")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if values.is_empty() {
            return Ok("No data found in requested range.".to_string());
        }

        let mut lines = Vec::new();
        for (index, row) in values.iter().enumerate() {
            let cells = row
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|cell| cell.as_str().unwrap_or("").to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            lines.push(format!("Row {}: {}", index + 1, cells.join(" | ")));
        }

        Ok(lines.join("\n"))
    }

    async fn execute_append(
        &self,
        spreadsheet_id: &str,
        range: &str,
        values: Vec<Vec<String>>,
    ) -> Result<String> {
        let encoded_range = encode_range(range);
        let endpoint = format!(
            "{}/{}/values/{}:append?valueInputOption=USER_ENTERED&insertDataOption=INSERT_ROWS",
            SHEETS_API_BASE, spreadsheet_id, encoded_range
        );
        self.execute_write("POST", &endpoint, values).await
    }

    async fn execute_update(
        &self,
        spreadsheet_id: &str,
        range: &str,
        values: Vec<Vec<String>>,
    ) -> Result<String> {
        let encoded_range = encode_range(range);
        let endpoint = format!(
            "{}/{}/values/{}?valueInputOption=USER_ENTERED",
            SHEETS_API_BASE, spreadsheet_id, encoded_range
        );
        self.execute_write("PUT", &endpoint, values).await
    }

    async fn execute_write(
        &self,
        method: &str,
        endpoint: &str,
        values: Vec<Vec<String>>,
    ) -> Result<String> {
        let body = json!({ "values": values });
        let request = match method {
            "POST" => self.client.post(endpoint),
            "PUT" => self.client.put(endpoint),
            _ => {
                return Err(ZeptoError::Tool(format!(
                    "Unsupported HTTP method for Sheets write: {}",
                    method
                )))
            }
        };

        let response = request
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Google Sheets request failed: {}", e)))?;

        let status = response.status();
        let payload: Value = response
            .json()
            .await
            .map_err(|e| ZeptoError::Tool(format!("Invalid Sheets response payload: {}", e)))?;

        if !status.is_success() {
            return Err(ZeptoError::Tool(format!(
                "Google Sheets API error {}: {}",
                status, payload
            )));
        }

        let updated_rows = payload
            .get("updates")
            .and_then(|updates| updates.get("updatedRows"))
            .and_then(Value::as_u64)
            .unwrap_or(0);

        Ok(format!(
            "Google Sheets write successful (updated_rows={})",
            updated_rows
        ))
    }
}

/// Validate that a spreadsheet ID is safe to interpolate into a URL path.
///
/// Google Sheet IDs are base64-like strings containing alphanumeric characters,
/// hyphens, and underscores. Reject anything else to prevent path injection.
fn validate_spreadsheet_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(ZeptoError::Tool(
            "Spreadsheet ID must not be empty".to_string(),
        ));
    }

    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ZeptoError::Tool(format!(
            "Invalid spreadsheet ID '{}': only alphanumeric characters, hyphens, and underscores are allowed",
            id
        )));
    }

    Ok(())
}

/// Encode a Google Sheets A1 notation range for use in a URL path.
///
/// Handles the special characters that can appear in sheet names and range
/// notation (e.g., "Sheet 1!A1:B2").
fn encode_range(range: &str) -> String {
    range
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '!' => "%21".to_string(),
            '#' => "%23".to_string(),
            '$' => "%24".to_string(),
            '+' => "%2B".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

#[async_trait]
impl Tool for GoogleSheetsTool {
    fn name(&self) -> &str {
        "google_sheets"
    }

    fn description(&self) -> &str {
        "Read and write Google Sheets data ranges."
    }

    fn compact_description(&self) -> &str {
        "Google Sheets"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::NetworkWrite
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID from the sheet URL."
                },
                "action": {
                    "type": "string",
                    "enum": ["read", "append", "update"],
                    "description": "Operation to perform."
                },
                "range": {
                    "type": "string",
                    "description": "A1 notation range, e.g. Orders!A:F."
                },
                "values": {
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": {"type": "string"}
                    },
                    "description": "Rows to append/update for write actions."
                }
            },
            "required": ["spreadsheet_id", "action", "range"]
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let spreadsheet_id = args
            .get("spreadsheet_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'spreadsheet_id'".to_string()))?;
        validate_spreadsheet_id(spreadsheet_id)?;
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'action'".to_string()))?;
        let range = args
            .get("range")
            .and_then(Value::as_str)
            .ok_or_else(|| ZeptoError::Tool("Missing 'range'".to_string()))?;

        let s = match action {
            "read" => self.execute_read(spreadsheet_id, range).await?,
            "append" => {
                let values = Self::extract_values(&args)?;
                self.execute_append(spreadsheet_id, range, values).await?
            }
            "update" => {
                let values = Self::extract_values(&args)?;
                self.execute_update(spreadsheet_id, range, values).await?
            }
            other => return Err(ZeptoError::Tool(format!("Unknown action '{}'", other))),
        };
        Ok(ToolOutput::llm_only(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_google_sheets_tool_properties() {
        let tool = GoogleSheetsTool::new("token");
        assert_eq!(tool.name(), "google_sheets");
        assert!(tool.description().contains("Google Sheets"));
    }

    #[test]
    fn test_google_sheets_tool_parameters() {
        let tool = GoogleSheetsTool::new("token");
        let params = tool.parameters();
        assert!(params["properties"]["spreadsheet_id"].is_object());
        assert!(params["properties"]["action"].is_object());
        assert!(params["properties"]["range"].is_object());
    }

    #[test]
    fn test_extract_values() {
        let values = GoogleSheetsTool::extract_values(&json!({
            "values": [["A", "B"], ["1", "2"]]
        }))
        .unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn test_from_service_account_requires_access_token() {
        let payload = base64::engine::general_purpose::STANDARD.encode(r#"{"foo":"bar"}"#);
        let result = GoogleSheetsTool::from_service_account(&payload);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_google_sheets_missing_args() {
        let tool = GoogleSheetsTool::new("token");
        let result = tool
            .execute(json!({"action":"read"}), &ToolContext::new())
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_spreadsheet_id_valid() {
        assert!(validate_spreadsheet_id("1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgVE2upms").is_ok());
        assert!(validate_spreadsheet_id("abc-def_123").is_ok());
        assert!(validate_spreadsheet_id("a").is_ok());
    }

    #[test]
    fn test_validate_spreadsheet_id_invalid() {
        // Path traversal
        assert!(validate_spreadsheet_id("../etc/passwd").is_err());
        // Slash injection
        assert!(validate_spreadsheet_id("id/values/Sheet1").is_err());
        // Query string injection
        assert!(validate_spreadsheet_id("id?key=evil").is_err());
        // Empty
        assert!(validate_spreadsheet_id("").is_err());
        // Spaces
        assert!(validate_spreadsheet_id("has space").is_err());
    }

    #[test]
    fn test_encode_range_special_chars() {
        // Space encoding
        assert_eq!(encode_range("Sheet 1"), "Sheet%201");
        // Exclamation mark (sheet separator)
        assert_eq!(encode_range("Sheet1!A1:B2"), "Sheet1%21A1:B2");
        // Combined: space and exclamation
        assert_eq!(encode_range("My Sheet!A:F"), "My%20Sheet%21A:F");
        // Dollar sign (absolute reference)
        assert_eq!(encode_range("Sheet1!$A$1"), "Sheet1%21%24A%241");
        // Hash
        assert_eq!(encode_range("Sheet#1"), "Sheet%231");
        // Plus sign
        assert_eq!(encode_range("a+b"), "a%2Bb");
        // Plain range (no encoding needed)
        assert_eq!(encode_range("A1:B2"), "A1:B2");
    }

    // ==================== ADDITIONAL ERROR/SECURITY TESTS ====================

    #[tokio::test]
    async fn test_unknown_action_rejected() {
        let tool = GoogleSheetsTool::new("token");
        let ctx = ToolContext::new();

        let result = tool
            .execute(
                json!({
                    "spreadsheet_id": "abc123",
                    "action": "delete",
                    "range": "A1:B2"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unknown action 'delete'"),
            "Expected unknown action error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_missing_spreadsheet_id() {
        let tool = GoogleSheetsTool::new("token");
        let ctx = ToolContext::new();

        let result = tool
            .execute(
                json!({
                    "action": "read",
                    "range": "A1:B2"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Missing 'spreadsheet_id'"),
            "Expected missing spreadsheet_id error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_missing_range() {
        let tool = GoogleSheetsTool::new("token");
        let ctx = ToolContext::new();

        let result = tool
            .execute(
                json!({
                    "spreadsheet_id": "abc123",
                    "action": "read"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Missing 'range'"),
            "Expected missing range error, got: {}",
            err
        );
    }

    #[test]
    fn test_extract_values_malformed_row() {
        // Each row must be an array; a string should be rejected
        let result = GoogleSheetsTool::extract_values(&json!({
            "values": ["not-an-array", "also-not"]
        }));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Each row must be an array"),
            "Expected row format error, got: {}",
            err
        );
    }

    #[test]
    fn test_extract_values_missing_values_key() {
        let result = GoogleSheetsTool::extract_values(&json!({"data": [["A"]]}));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Missing 'values'"),
            "Expected missing values error, got: {}",
            err
        );
    }

    #[test]
    fn test_from_service_account_invalid_base64() {
        let result = GoogleSheetsTool::from_service_account("not-valid-base64!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid base64"),
            "Expected base64 error, got: {}",
            err
        );
    }

    #[test]
    fn test_from_service_account_empty_access_token() {
        // access_token present but empty string
        let payload = base64::engine::general_purpose::STANDARD.encode(r#"{"access_token": "  "}"#);
        let result = GoogleSheetsTool::from_service_account(&payload);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("non-empty 'access_token'"),
            "Expected empty token error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_spreadsheet_id_path_injection_with_colon() {
        // Colons, percent-encoding, and other URL-significant characters
        assert!(validate_spreadsheet_id("id:values").is_err());
        assert!(validate_spreadsheet_id("id%2Fvalues").is_err());
        assert!(validate_spreadsheet_id("id\ninjection").is_err());
    }
}
